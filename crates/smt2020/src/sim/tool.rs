//! Tools: jobs on a phase timeline (setup, load, processing, unload), outages that pause them, and
//! time accounting per state.

use des_core::Time;

use super::fab::LotId;
use crate::data::{Pm, PmTrigger, SetupId, ToolGroupId};

/// Tool state. Where jobs of a cascading tool overlap, the earlier variant counts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum State {
    Down,
    Pm,
    Setup,
    Process,
    Load,
    Unload,
    Idle,
}

pub(super) const STATES: usize = 7;

/// Durations of a job about to start.
pub(super) struct Work {
    pub setup: Time,
    pub load: Time,
    /// Processing time per unit (lot, wafer or batch).
    pub process: Time,
    pub units: u32,
    /// Cascading tools: time a unit occupies the first slot.
    pub cascade: Option<Time>,
    pub unload: Time,
}

#[derive(Default)]
pub(super) struct Job {
    pub lots: Vec<LotId>,
    pub active: bool,
    /// Phase ends; between `load_end` and `process_start` the first unit waits for the first slot.
    setup_end: Time,
    load_end: Time,
    process_start: Time,
    process_end: Time,
    pub end: Time,
}

impl Job {
    /// Phase at `t` (`None` while waiting or done) and when it ends.
    fn phase(&self, t: Time) -> (Option<State>, Time) {
        if t < self.setup_end {
            (Some(State::Setup), self.setup_end)
        } else if t < self.load_end {
            (Some(State::Load), self.load_end)
        } else if t < self.process_start {
            (None, self.process_start)
        } else if t < self.process_end {
            (Some(State::Process), self.process_end)
        } else if t < self.end {
            (Some(State::Unload), self.end)
        } else {
            (None, Time::MAX)
        }
    }

    fn times(&mut self) -> [&mut Time; 5] {
        [
            &mut self.setup_end,
            &mut self.load_end,
            &mut self.process_start,
            &mut self.process_end,
            &mut self.end,
        ]
    }
}

pub(super) struct Tool {
    pub group: ToolGroupId,
    pub cascading: bool,
    pub setup: Option<SetupId>,
    /// Lots still due in the current setup run (`rule_LSSU`).
    pub run_left: u32,
    /// One job, or two on a cascading tool.
    pub jobs: [Job; 2],
    /// Cascading tools: when the first and the second slot become free.
    slots_free: [Time; 2],
    pub breakdowns: u32,
    /// Breakdowns that fell due during the running PM; they begin when it ends.
    pub deferred: Vec<usize>,
    pub pm: Option<usize>,
    pub pm_pending: Vec<usize>,
    /// Per PM of the group (wafer-count PMs only): wafers since the last PM and the trigger.
    pm_wafers: Vec<u32>,
    pm_threshold: Vec<u32>,
    paused_at: Option<Time>,
    /// Bumped when jobs pause, invalidating their scheduled ends.
    pub epoch: u32,
    /// Listed in the group's ready queue.
    pub ready: bool,
    /// Kept free for a super hot lot.
    pub held: bool,
    accounted: Time,
    pub time: [Time; STATES],
}

impl Tool {
    /// The `position`-th (1-based) of `count` tools; first PMs are staggered at `position/count`.
    pub(super) fn new(
        group: ToolGroupId,
        cascading: bool,
        pms: &[Pm],
        position: u32,
        count: u32,
    ) -> Self {
        let pm_threshold = pms
            .iter()
            .map(|pm| match pm.trigger {
                PmTrigger::Wafers { first, .. } => {
                    (u64::from(first) * u64::from(position) / u64::from(count)) as u32
                }
                PmTrigger::Calendar { .. } => 0,
            })
            .collect();
        Self {
            group,
            cascading,
            setup: None,
            run_left: 0,
            jobs: Default::default(),
            slots_free: [0; 2],
            breakdowns: 0,
            deferred: Vec::new(),
            pm: None,
            pm_pending: Vec::new(),
            pm_wafers: vec![0; pms.len()],
            pm_threshold,
            paused_at: None,
            epoch: 0,
            ready: false,
            held: false,
            accounted: 0,
            time: [0; STATES],
        }
    }

    pub(super) fn busy(&self) -> usize {
        self.jobs.iter().filter(|job| job.active).count()
    }

    /// Up, no PM due, and room for a job (a cascading tool holds two lots).
    pub(super) fn available(&self) -> bool {
        self.breakdowns == 0
            && self.pm.is_none()
            && self.pm_pending.is_empty()
            && self.busy() < if self.cascading { 2 } else { 1 }
    }

    /// Starts a job now; returns its slot and end.
    pub(super) fn start(&mut self, lots: Vec<LotId>, now: Time, work: Work) -> (usize, Time) {
        let slot = self
            .jobs
            .iter()
            .position(|job| !job.active)
            .expect("tool has room");
        let setup_end = now + work.setup;
        let load_end = setup_end + work.load;
        let (process_start, process_end) = match work.cascade {
            None => (load_end, load_end + work.process * Time::from(work.units)),
            Some(interval) => {
                // Two serial slots: a unit spends `interval` in the first slot, waits there while
                // the second is busy, then spends the rest of its processing in the second.
                let [mut first, mut second] = self.slots_free;
                let start = load_end.max(first);
                first = start;
                for _ in 0..work.units {
                    first = (first + interval).max(second);
                    second = first + work.process - interval;
                }
                self.slots_free = [first, second];
                (start, second)
            }
        };
        let end = process_end + work.unload;
        self.jobs[slot] = Job {
            lots,
            active: true,
            setup_end,
            load_end,
            process_start,
            process_end,
            end,
        };
        (slot, end)
    }

    /// Ends the job in `slot`, returning its lots.
    pub(super) fn finish(&mut self, slot: usize) -> Vec<LotId> {
        self.jobs[slot].active = false;
        std::mem::take(&mut self.jobs[slot].lots)
    }

    /// Counts processed wafers; requests wafer-count PMs that are due.
    pub(super) fn add_wafers(&mut self, wafers: u32, pms: &[Pm]) {
        for (index, pm) in pms.iter().enumerate() {
            if let PmTrigger::Wafers { .. } = pm.trigger {
                self.pm_wafers[index] += wafers;
                if self.pm_wafers[index] >= self.pm_threshold[index] {
                    self.request_pm(index);
                }
            }
        }
    }

    pub(super) fn request_pm(&mut self, pm: usize) {
        if self.pm != Some(pm) && !self.pm_pending.contains(&pm) {
            self.pm_pending.push(pm);
        }
    }

    /// Starts the next due PM; a wafer-count PM restarts its count.
    pub(super) fn start_pm(&mut self, pms: &[Pm]) -> usize {
        let pm = self.pm_pending.remove(0);
        if let PmTrigger::Wafers { interval, .. } = pms[pm].trigger {
            self.pm_wafers[pm] = 0;
            self.pm_threshold[pm] = interval;
        }
        self.pm = Some(pm);
        pm
    }

    /// Adds the time since the last accounting to the states held meanwhile.
    pub(super) fn account(&mut self, until: Time) {
        while self.accounted < until {
            let (state, change) = self.state_at(self.accounted);
            let end = change.min(until);
            self.time[state as usize] += end - self.accounted;
            self.accounted = end;
        }
    }

    fn state_at(&self, t: Time) -> (State, Time) {
        if self.breakdowns > 0 {
            return (State::Down, Time::MAX);
        }
        if self.pm.is_some() {
            return (State::Pm, Time::MAX);
        }
        let mut state = State::Idle;
        let mut change = Time::MAX;
        for job in self.jobs.iter().filter(|job| job.active) {
            let (phase, end) = job.phase(t);
            if let Some(phase) = phase {
                state = state.min(phase);
            }
            change = change.min(end);
        }
        (state, change)
    }

    /// Freezes running jobs (breakdown).
    pub(super) fn pause(&mut self, now: Time) {
        if self.paused_at.is_none() && self.busy() > 0 {
            self.paused_at = Some(now);
            self.epoch += 1;
        }
    }

    /// Shifts paused jobs by the downtime; true if jobs must be rescheduled.
    pub(super) fn resume(&mut self, now: Time) -> bool {
        let Some(paused) = self.paused_at.take() else {
            return false;
        };
        let shift = now - paused;
        let jobs = self.jobs.iter_mut().filter(|job| job.active);
        for time in jobs.flat_map(Job::times).chain(&mut self.slots_free) {
            if *time >= paused {
                *time += shift;
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cascading() -> Tool {
        Tool::new(0, true, &[], 1, 1)
    }

    fn work(process: Time, units: u32, cascade: Option<Time>) -> Work {
        Work {
            setup: 0,
            load: 1,
            process,
            units,
            cascade,
            unload: 1,
        }
    }

    #[test]
    fn wafers_cascade_through_two_slots() {
        let mut tool = cascading();
        // 25 wafers of 10 with a 7 interval: first wafer 10, then one every 7.
        let (_, end) = tool.start(vec![0], 0, work(10, 25, Some(7)));
        assert_eq!(end, 1 + 10 + 24 * 7 + 1);
        // A second lot loads meanwhile and follows the last wafer through the first slot.
        let (_, end) = tool.start(vec![1], 5, work(10, 1, Some(7)));
        assert_eq!(end, 1 + 25 * 7 + 10 + 1);
    }

    #[test]
    fn a_unit_waits_in_the_first_slot_while_the_second_is_busy() {
        let mut tool = cascading();
        // Long unit: 2 in the first slot, 98 in the second.
        tool.start(vec![0], 0, work(100, 1, Some(2)));
        // The next unit finishes its interval at 4 but leaves the first slot only at 101.
        let (_, end) = tool.start(vec![1], 0, work(10, 1, Some(2)));
        assert_eq!(end, 101 + 8 + 1);
    }

    #[test]
    fn accounting_and_pause() {
        let mut tool = Tool::new(0, false, &[], 1, 1);
        let (slot, end) = tool.start(
            vec![0],
            0,
            Work {
                setup: 2,
                load: 1,
                process: 3,
                units: 2,
                cascade: None,
                unload: 1,
            },
        );
        assert_eq!(end, 2 + 1 + 6 + 1);
        tool.account(4);
        tool.breakdowns = 1;
        tool.pause(4);
        tool.account(14);
        tool.breakdowns = 0;
        assert!(tool.resume(14));
        tool.account(30);
        assert_eq!(tool.jobs[slot].end, end + 10);
        let time = tool.time;
        assert_eq!(
            [
                time[State::Setup as usize],
                time[State::Load as usize],
                time[State::Process as usize]
            ],
            [2, 1, 6]
        );
        assert_eq!(
            [time[State::Down as usize], time[State::Unload as usize]],
            [10, 1]
        );
        assert_eq!(time[State::Idle as usize], 30 - 20);
    }
}
