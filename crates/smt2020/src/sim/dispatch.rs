//! Lot selection: which ready tool picks first, which lots it may take, their ranking and batch
//! formation. Dispatching runs on events only (arrival, job end, repair, PM end, reservation
//! release, stopping limit release) and reads the queue entries fixed on arrival.

use std::cmp::Ordering;
use std::mem;

use des_core::{Scheduler, Time};

use super::fab::{Event, Fab, LotId, LotState, ToolId, Urgency, Waiting};
use crate::data::{Dist, PartId, Rank, Rule, SetupId, StepIndex, StepSetup, ToolGroupId};

/// Ranking key compared lexicographically, smallest first.
pub(super) type Key = [f64; 7];

/// What the keys of one selection share: the group's ranks, the tool's setup and the instant.
struct Ranking<'a> {
    ranks: &'a [Rank],
    /// CAtE/CoT on the steppers: whether engineering lots are preferred.
    prefer_engineering: Option<bool>,
    setup: Option<SetupId>,
    now: Time,
}

fn compare(a: &Key, b: &Key) -> Ordering {
    a.iter()
        .zip(b)
        .map(|(x, y)| x.total_cmp(y))
        .find(|order| order.is_ne())
        .unwrap_or(Ordering::Equal)
}

impl Fab {
    /// Lets the group's ready tools pick lots, longest available first; for a lot arriving at a
    /// `wake_LeastSetupTime` group, the tools needing the least setup for it first.
    pub(super) fn dispatch(
        &mut self,
        group: ToolGroupId,
        arrival: Option<LotId>,
        sched: &mut Scheduler<Event>,
    ) {
        if self.groups[group].queue.is_empty() || self.groups[group].ready.is_empty() {
            return;
        }
        if self.mark_held(group) && !self.stopped.contains(&group) {
            self.stopped.push(group);
        }
        let mut order = mem::take(&mut self.order);
        order.clear();
        order.extend(&self.groups[group].ready);
        if let Some(lot) = arrival
            && self.data.tool_groups[group].wake_least_setup
        {
            let setup = self.groups[group]
                .queue
                .iter()
                .rev()
                .find(|waiting| waiting.lot == lot)
                .expect("arrived lot")
                .setup;
            order.sort_by(|&a, &b| {
                self.wake_setup_time(setup, self.tools[a].setup)
                    .total_cmp(&self.wake_setup_time(setup, self.tools[b].setup))
            });
        }
        for &tool in &order {
            while self.tools[tool].ready && self.select(tool, sched.now()) {
                self.start_job(tool, sched);
            }
            if self.groups[group].queue.is_empty() {
                break;
            }
        }
        self.order = order;
    }

    /// Stopping ([P2] §3.2): marks the lots about to enter a CQT segment while a limit of a tool
    /// group of the segment is reached; true if any is held. The marks hold for the whole
    /// dispatch, as job starts leave the stopping counts unchanged.
    fn mark_held(&mut self, group: ToolGroupId) -> bool {
        let Some(stopping) = &self.strategy.stopping else {
            return false;
        };
        let mut held = false;
        for waiting in &mut self.groups[group].queue {
            waiting.held = waiting.entering
                && self.routes.info[waiting.route].segment_groups[waiting.step]
                    .iter()
                    .any(|&later| stopping.reached(later));
            held |= waiting.held;
        }
        held
    }

    /// Puts the best lot, or the best batch, for `tool` into `selected`.
    fn select(&mut self, tool: ToolId, now: Time) -> bool {
        let group = self.tools[tool].group;
        // CoT: a campaign starts once enough engineering lots wait.
        if let Some(trigger) = self.strategy.campaign_trigger()
            && self.strategy.steppers[group]
            && self.groups[group].campaign == 0
        {
            let waiting = self.groups[group]
                .queue
                .iter()
                .filter(|waiting| waiting.kind.engineering());
            if waiting.count() >= trigger as usize {
                self.groups[group].campaign = trigger;
            }
        }
        // rule_LSSU: an unfinished setup run, hot lots included, waits for lots keeping the setup
        // while any can come (AutoSched documentation: a minimum number of lots is ensured).
        let state = &self.tools[tool];
        let current = state.setup;
        let run_holds = matches!(self.data.tool_groups[group].rule, Rule::SetupRun(_))
            && state.run_left > 0
            && current.is_some_and(|setup| {
                self.can_still_come(&self.routes.setup_members[&(group, setup)])
            });
        // Held lots, lots dedicated to another tool, and setup changes during a held run wait.
        let eligible = |waiting: &Waiting| {
            !waiting.held
                && waiting.dedicated.is_none_or(|dedicated| dedicated == tool)
                && !(run_holds
                    && waiting
                        .setup
                        .is_some_and(|setup| current != Some(setup.setup)))
        };
        let spec = &self.data.tool_groups[group];
        let ranking = Ranking {
            ranks: &spec.ranks,
            prefer_engineering: self.strategy.prefer_engineering(
                group,
                now,
                self.groups[group].campaign,
            ),
            setup: current,
            now,
        };
        if spec.batching.is_some() {
            let mut candidates = mem::take(&mut self.candidates);
            candidates.clear();
            for (index, waiting) in self.groups[group].queue.iter().enumerate() {
                if eligible(waiting) {
                    candidates.push((self.key(waiting, &ranking), index));
                }
            }
            let found = self.form_batch(group, &mut candidates);
            self.candidates = candidates;
            return found;
        }
        let mut best: Option<(Key, LotId)> = None;
        for waiting in &self.groups[group].queue {
            if eligible(waiting) {
                let key = self.key(waiting, &ranking);
                if best
                    .as_ref()
                    .is_none_or(|(best, _)| compare(&key, best).is_lt())
                {
                    best = Some((key, waiting.lot));
                }
            }
        }
        let Some((_, lot)) = best else {
            return false;
        };
        self.selected.clear();
        self.selected.push(lot);
        true
    }

    /// `rank_RSETUP`: mean setup time from the setup change table for a lot on a tool with
    /// `current` setup. Setup times given only in the route rank as none, as in the AutoSched
    /// reference runs.
    fn ranked_setup_time(&self, setup: Option<StepSetup>, current: Option<SetupId>) -> f64 {
        needed_setup(setup, current)
            .and_then(|setup| self.setup_dist(current, setup.setup))
            .map_or(0.0, |dist| dist.mean() as f64)
    }

    /// `wake_LeastSetupTime`: mean time of the setup a lot needs on a tool with `current` setup.
    fn wake_setup_time(&self, setup: Option<StepSetup>, current: Option<SetupId>) -> f64 {
        needed_setup(setup, current).map_or(0.0, |setup| {
            setup
                .time
                .or_else(|| self.setup_dist(current, setup.setup).map(Dist::mean))
                .unwrap_or(0) as f64
        })
    }

    /// Lexicographic key: CAtE/CoT class preference, then the group's ranks with the queue-time
    /// urgency right before FIFO/CR, then release order.
    fn key(&self, waiting: &Waiting, ranking: &Ranking) -> Key {
        let now = ranking.now;
        let mut key = [0.0; 7];
        let mut len = 0;
        let mut push = |value: f64| {
            key[len] = value;
            len += 1;
        };
        if let Some(prefer) = ranking.prefer_engineering {
            push(if waiting.kind.engineering() == prefer {
                0.0
            } else {
                1.0
            });
        }
        for &rank in ranking.ranks {
            match rank {
                Rank::Priority => push(-f64::from(waiting.priority)),
                Rank::LeastSetup => push(self.ranked_setup_time(waiting.setup, ranking.setup)),
                Rank::Fifo | Rank::CriticalRatio => {
                    match waiting.urgency {
                        Urgency::None => {}
                        Urgency::Outside => push(f64::INFINITY),
                        Urgency::Qtcr { deadline, work } => push(qtcr(deadline, work, now)),
                        Urgency::Qts(deadline) => push(deadline),
                    }
                    push(match rank {
                        Rank::Fifo => waiting.queued_at as f64,
                        // Time to the due date over the expected remaining work.
                        _ => (waiting.due - now) as f64 / waiting.remaining,
                    });
                }
            }
        }
        push(waiting.serial as f64);
        key
    }

    /// [P2] §4.1: the best-ranked lot opens a batch filled with compatible lots in rank order up to
    /// the maximum. It starts at the minimum, or below it once no compatible lot can still come;
    /// otherwise the next-ranked lot of another batch kind opens one.
    fn form_batch(&mut self, group: ToolGroupId, candidates: &mut [(Key, usize)]) -> bool {
        candidates.sort_by(|a, b| compare(&a.0, &b.0));
        let queue = &self.groups[group].queue;
        let mut tried = Vec::new();
        for first in 0..candidates.len() {
            let head = &queue[candidates[first].1];
            let key = head.batch.expect("batch step");
            if tried.contains(&key) {
                continue;
            }
            tried.push(key);
            let size = self.data.routes[head.route].steps[head.step]
                .batch
                .expect("batch size");
            self.selected.clear();
            let mut wafers = 0;
            for &(_, index) in &candidates[first..] {
                let waiting = &queue[index];
                if waiting.batch == Some(key) && wafers + waiting.wafers <= size.max {
                    self.selected.push(waiting.lot);
                    wafers += waiting.wafers;
                }
            }
            if wafers >= size.min || !self.can_still_come(&self.routes.batch_members[key]) {
                return true;
            }
        }
        false
    }

    /// A lot can still reach one of these (part, step) pairs: the part has releases left, or a lot
    /// is before the step or moving to it. Such steps are never skipped or reworked over (checked
    /// by the loader), so these lots arrive and the arrival dispatches the waiting tools again.
    fn can_still_come(&self, members: &[(PartId, StepIndex)]) -> bool {
        members
            .iter()
            .any(|&(part, _)| self.plan.remaining[part] > 0)
            || self.lots.iter().any(|lot| {
                lot.alive
                    && members.iter().any(|&(part, step)| {
                        lot.part == part
                            && (lot.step < step
                                || (lot.step == step && lot.state == LotState::Moving))
                    })
            })
    }
}

/// Setup a lot needs on a tool with `current` setup, if any.
fn needed_setup(setup: Option<StepSetup>, current: Option<SetupId>) -> Option<StepSetup> {
    setup.filter(|setup| setup.always || current != Some(setup.setup))
}

/// [P2] eq. (1): QTCR index from the segment end date and the expected work to the exit step.
fn qtcr(deadline: Time, work: f64, now: Time) -> f64 {
    let slack = (deadline - now) as f64;
    if slack >= 0.0 {
        slack / work
    } else {
        slack * work
    }
}
