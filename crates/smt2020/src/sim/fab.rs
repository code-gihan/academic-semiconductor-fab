//! The fab as a `des_core` model: releases, lot moves, tool jobs, outages and period reports.

use std::collections::VecDeque;
use std::mem;
use std::sync::Arc;

use des_core::{DAY, Model, Scheduler, Time};

use super::code::{Code, CqtView, GroupCount, Hooks, LotView};
use super::dispatch::Key;
use super::logistics::Logistics;
use super::plan::{Plan, releases_before};
use super::record::{Entry, EventKind, Recorder, Recording, Records};
use super::routes::Routes;
use super::stats::{CqtReport, Days, LotKind, PeriodReport, Results, Stats, Visit};
use super::strategy::{SegmentCounts, Strategy};
use super::tool::{STATES, Tool, ToolState, Work};
use super::{Config, DRAIN_LIMIT, Error};
use crate::data::{
    Dataset, Dist, LocationId, LotRelease, PartId, PmTrigger, RouteId, Rule, SetupId, StepIndex,
    StepSetup, ToolGroupId, Unit,
};
use crate::rng::{Purpose, Streams};

pub(super) type LotId = usize;
pub(super) type ToolId = usize;

pub(super) enum Event {
    /// Next release of a periodic stream.
    Stream(usize),
    /// Next listed lot.
    Listed,
    Arrive(LotId),
    JobDone {
        tool: ToolId,
        slot: usize,
        epoch: u32,
    },
    Fail {
        tool: ToolId,
        breakdown: usize,
    },
    Repair {
        tool: ToolId,
        breakdown: usize,
    },
    PmDue {
        tool: ToolId,
        pm: usize,
    },
    PmDone(ToolId),
    /// The group dispatches again: a batch below its minimum size or a lot held by strategy code
    /// may go now ([`Config::batch_start_within`], [`Code`]).
    Wake(ToolGroupId),
    /// End of the reporting period with this index.
    PeriodEnd(usize),
    /// A vehicle's next instant to act ([`Amhs`](super::amhs)); stale if its epoch moved on.
    Vehicle {
        vehicle: u32,
        epoch: u32,
    },
    /// [`DRAIN_LIMIT`] after the horizon: the run stops unfinished.
    Deadline,
}

named_enum! {
    /// Where a lot is at its current step.
    pub enum LotState {
        /// On the way to the step's tool group.
        Moving = "moving",
        /// In the tool group's queue (with an AMHS also while assigned to a tool and on its way
        /// there).
        Queued = "queued",
        /// In a job on a tool.
        Processing = "processing",
        /// Past its last step, on its way out of the fab (AMHS).
        Leaving = "leaving",
    }
}

/// Open CQT segment `id`: from the end of step `entry` to the start of step `exit`.
pub(super) struct Segment {
    pub id: usize,
    pub entry: StepIndex,
    pub exit: StepIndex,
    pub entered: Time,
    pub limit: Time,
}

pub(super) struct Lot {
    pub alive: bool,
    pub part: PartId,
    pub route: RouteId,
    pub kind: LotKind,
    pub priority: u32,
    pub wafers: u32,
    pub release: Time,
    pub due: Time,
    /// Release order, the final tie-breaker.
    pub serial: u64,
    pub step: StepIndex,
    pub state: LotState,
    /// End of the previous processed step (release for the first).
    pub last_done: Time,
    pub location: Option<LocationId>,
    /// Lot-to-lens dedications: (step, tool).
    pub dedicated: Vec<(StepIndex, ToolId)>,
    pub segment: Option<Segment>,
    /// Reserves a tool at its next step (super hot lots).
    pub reserve: bool,
    /// Tool group holding a reservation for this lot.
    pub reservation: Option<ToolGroupId>,
}

/// When a lot arrived at its step's queue and started its job, and the steps of its open CQT
/// segment so far (the wait's parts). Apart from [`Lot`], whose scans stay compact.
#[derive(Default)]
pub(super) struct LotTimes {
    pub arrived: Time,
    pub started: Time,
    pub visits: Vec<Visit>,
    /// AMHS: time of the transports of the present step so far (request to drop-off), and when
    /// the running one was requested.
    pub moved: Time,
    pub requested: Time,
}

pub(super) struct Reservation {
    pub lot: LotId,
    pub step: StepIndex,
    pub tool: Option<ToolId>,
}

/// A lot queued at a tool group with everything its ranking reads, fixed on arrival: dispatching
/// scans these entries instead of lots and route data.
#[derive(Clone)]
pub(super) struct Waiting {
    pub lot: LotId,
    pub route: RouteId,
    pub step: StepIndex,
    pub kind: LotKind,
    pub priority: u32,
    pub wafers: u32,
    pub serial: u64,
    pub queued_at: Time,
    pub due: Time,
    /// Expected remaining work from this step (critical ratio, least remaining).
    pub remaining: f64,
    /// Expected duration of this step (shortest step).
    pub step_time: f64,
    pub setup: Option<StepSetup>,
    /// Lot-to-lens dedication: the only tool that may process the lot here.
    pub dedicated: Option<ToolId>,
    /// Batch compatibility key (batch steps).
    pub batch: Option<usize>,
    /// Queue-time inputs of a lot in a CQT segment.
    pub cqt: Option<WaitingCqt>,
    /// The strategy code's priority, at a group that ranks by it (0 elsewhere).
    pub code: f64,
    /// The lot enters a CQT segment here, so stopping limits and admission code apply.
    pub entering: bool,
    /// Held by stopping or by strategy code, as of the group's current dispatch.
    pub held: bool,
}

/// Queue-time inputs of a waiting lot in a CQT segment.
#[derive(Clone, Copy)]
pub(super) struct WaitingCqt {
    /// End of the segment: the exit step must start by then.
    pub deadline: Time,
    /// Expected work from this step through the exit step (QTCR).
    pub work: f64,
    /// Expected work from this step until the exit step starts (queue-time slack).
    pub before_exit: f64,
    /// QTS latest start of this step; 0 without QTS flow factors (no QTS ranking then).
    pub latest: f64,
}

impl WaitingCqt {
    /// Queue-time slack at `now`: segment end − now − expected work before the exit step.
    pub(super) fn slack(&self, now: Time) -> f64 {
        (self.deadline - now) as f64 - self.before_exit
    }

    /// First time the slack is at most `within`.
    pub(super) fn within_from(&self, within: Time) -> Time {
        ((self.deadline - within) as f64 - self.before_exit).ceil() as Time
    }
}

#[derive(Default)]
pub(super) struct Group {
    pub queue: Vec<Waiting>,
    /// Available tools, longest available first.
    pub ready: VecDeque<ToolId>,
    pub reservation: Option<Reservation>,
    /// CoT: engineering lots still to start in the current campaign.
    pub campaign: u32,
    /// Earliest time a batch or lot held by the current dispatch may go: by queue-time slack, or
    /// as strategy code asked.
    pub due: Option<Time>,
    /// Earliest pending [`Event::Wake`].
    pub wake: Option<Time>,
    /// Tool groups whose counts the lots held by admission code wait on, as of the group's
    /// current dispatch.
    pub held_on: Vec<ToolGroupId>,
    /// Lots queued, integrated over time since the start, up to `queue_since` (records).
    pub queue_area: f64,
    pub queue_since: Time,
}

impl Group {
    /// Integrates the queue length up to `now`, before it changes or is read.
    pub(super) fn integrate_queue(&mut self, now: Time) {
        self.queue_area += self.queue.len() as f64 * (now - self.queue_since) as f64;
        self.queue_since = now;
    }
}

struct Period {
    name: String,
    end: Time,
    report: bool,
    reset: bool,
}

pub(super) struct Fab {
    pub data: Arc<Dataset>,
    pub routes: Routes,
    pub strategy: Strategy,
    pub plan: Plan,
    horizon: Time,
    periods: Vec<Period>,
    reserve_super_hot: bool,
    pub rng: Streams,
    pub lots: Vec<Lot>,
    /// Per lot, as `lots`.
    pub times: Vec<LotTimes>,
    free: Vec<LotId>,
    serial: u64,
    pub tools: Vec<Tool>,
    pub groups: Vec<Group>,
    /// Setup durations per target setup: (from setup, `None` = any, time).
    setup_times: Vec<Vec<(Option<SetupId>, Dist)>>,
    /// Transport time per (from, to) location.
    transports: Vec<Option<Dist>>,
    stats: Stats,
    reports: Vec<PeriodReport>,
    days: Days,
    step_flow_factors: Vec<Vec<Option<f64>>>,
    seed: u64,
    replication: u32,
    released: u64,
    completed: u64,
    wip: usize,
    /// Releases continue until the horizon.
    releasing: bool,
    finished: Option<Time>,
    /// Constrained lots per tool group, with stopping or admission code.
    pub counts: Option<SegmentCounts>,
    /// Groups with lots held by stopping or admission code, dispatched again when a stopping
    /// limit is released or, for the code, when a count it waits on goes down.
    pub stopped: Vec<ToolGroupId>,
    /// Groups whose counts went down in the current event.
    fewer: Vec<ToolGroupId>,
    /// Strategy code (configured run only), the hooks it defines and its first error.
    pub code: Option<Box<dyn Code>>,
    pub hooks: Hooks,
    pub code_failure: Option<String>,
    /// Counts of a segment's tool groups, for admission code.
    pub group_counts: Vec<GroupCount>,
    /// Scratch buffers of dispatching; candidates index the group queue.
    pub selected: Vec<LotId>,
    pub candidates: Vec<(Key, usize)>,
    pub order: Vec<ToolId>,
    recorder: Option<Recorder>,
    records: Records,
    /// Ports, lots' FOUPs and the AMHS of a dataset with a layout.
    pub logistics: Option<Logistics>,
}

impl Fab {
    /// The fab of `config` at time 0 with QTS `flow_factors`, recording what `recording` asks,
    /// with the strategy `code`, which it takes unless this fails.
    pub(super) fn new(
        data: Arc<Dataset>,
        config: &Config,
        flow_factors: Option<&[Vec<Option<f64>>]>,
        recording: &Recording,
        code: &mut Option<Box<dyn Code>>,
    ) -> Result<Self, Error> {
        if config.horizon <= 0 {
            return Err(Error("the horizon must be positive".into()));
        }
        let strategy = Strategy::new(&data, config, flow_factors)?;
        let mut logistics = match (&data.layout, &config.amhs) {
            (None, None) => None,
            (None, Some(_)) => return Err(Error("the dataset has no AMHS layout".into())),
            (Some(_), amhs) => Some(Logistics::new(&data, &amhs.clone().unwrap_or_default())?),
        };
        if let (Some(window), Some(logistics)) = (&recording.replay, &mut logistics) {
            logistics.record(window);
        }
        let routes = Routes::new(
            &data,
            &strategy.steppers,
            logistics
                .as_ref()
                .map(|logistics| logistics.skipped.as_slice()),
        );
        let plan = Plan::new(&data, config.horizon, config.load)?;
        let recorder = Recorder::new(&data, recording)?;

        let mut periods = Vec::new();
        match config.warm_up {
            None => {
                for (index, period) in data.periods.iter().enumerate() {
                    let next = data
                        .periods
                        .get(index + 1)
                        .map_or(Time::MAX, |next| next.start);
                    if period.start >= config.horizon {
                        break;
                    }
                    let end = next.min(config.horizon);
                    // The window ending at the horizon always closes; the drain gets its own.
                    periods.push(Period {
                        name: period.name.clone(),
                        end,
                        report: period.report,
                        reset: period.reset || end == config.horizon,
                    });
                }
            }
            Some(warm_up) if 0 < warm_up && warm_up < config.horizon => {
                for (name, end) in [("WarmUp", warm_up), ("Period_1", config.horizon)] {
                    periods.push(Period {
                        name: name.into(),
                        end,
                        report: true,
                        reset: true,
                    });
                }
            }
            Some(_) => {
                return Err(Error(
                    "the warm-up must lie between 0 and the horizon".into(),
                ));
            }
        }
        if periods
            .last()
            .is_none_or(|period| period.end != config.horizon)
        {
            return Err(Error("no reporting period covers the horizon".into()));
        }

        let mut setup_times = vec![Vec::new(); data.setups.len()];
        for change in &data.setup_changes {
            setup_times[change.to].push((change.from, change.time));
        }
        let locations = data.locations.len();
        let mut transports = vec![None; locations * locations];
        for transport in &data.transports {
            transports[transport.from * locations + transport.to] = Some(transport.time);
        }

        let mut tools = Vec::new();
        let mut groups: Vec<Group> = data.tool_groups.iter().map(|_| Group::default()).collect();
        for (group_id, group) in data.tool_groups.iter().enumerate() {
            for position in 1..=group.tools {
                let mut tool =
                    Tool::new(group_id, group.cascading, &group.pms, position, group.tools);
                tool.ready = true;
                groups[group_id].ready.push_back(tools.len());
                tools.push(tool);
            }
        }

        let hooks = code.as_ref().map_or(Hooks::default(), |code| code.hooks());
        let counts = (strategy.stopping.is_some() || hooks.admit)
            .then(|| SegmentCounts::new(data.tool_groups.len()));
        Ok(Self {
            stats: Stats::new(&data, &routes),
            data,
            routes,
            strategy,
            plan,
            horizon: config.horizon,
            periods,
            reserve_super_hot: config.reserve_super_hot,
            rng: Streams::new(config.seed, config.replication),
            lots: Vec::new(),
            times: Vec::new(),
            free: Vec::new(),
            serial: 0,
            tools,
            groups,
            setup_times,
            transports,
            reports: Vec::new(),
            days: Days::new(),
            step_flow_factors: Vec::new(),
            seed: config.seed,
            replication: config.replication,
            released: 0,
            completed: 0,
            wip: 0,
            releasing: true,
            finished: None,
            counts,
            stopped: Vec::new(),
            fewer: Vec::new(),
            code: code.take(),
            hooks,
            code_failure: None,
            group_counts: Vec::new(),
            selected: Vec::new(),
            candidates: Vec::new(),
            order: Vec::new(),
            recorder,
            records: Records::default(),
            logistics,
        })
    }

    /// The tables recorded so far.
    pub(super) fn records(&self) -> &Records {
        &self.records
    }

    /// Every CQT segment completion so far.
    pub(super) fn cqt_total(&self) -> &CqtReport {
        &self.stats.cqt_total
    }

    /// Every completion of each CQT segment so far, in dataset order.
    pub(super) fn segment_totals(&self) -> &[CqtReport] {
        &self.stats.segment_totals
    }

    /// The first error of the strategy code, which stops the run.
    pub(super) fn code_failure(&self) -> Option<&str> {
        self.code_failure.as_deref()
    }

    /// Records the first error of the strategy code; the event ends without further calls, then
    /// the run stops.
    pub(super) fn record_failure(failure: &mut Option<String>, message: String) {
        failure.get_or_insert(message);
    }

    /// `waiting` as strategy code sees it at `now`.
    pub(super) fn lot_view(&self, waiting: &Waiting, now: Time) -> LotView {
        let lot = &self.lots[waiting.lot];
        LotView {
            id: waiting.serial,
            part: lot.part,
            kind: waiting.kind,
            priority: waiting.priority,
            wafers: waiting.wafers,
            release: lot.release,
            due: waiting.due,
            route: waiting.route,
            step: waiting.step,
            tool_group: self.data.routes[waiting.route].steps[waiting.step].tool_group,
            remaining: waiting.remaining,
            step_time: waiting.step_time,
            cqt: lot
                .segment
                .as_ref()
                .zip(waiting.cqt)
                .map(|(segment, cqt)| CqtView {
                    segment: segment.id,
                    limit: segment.limit,
                    entered: segment.entered,
                    deadline: cqt.deadline,
                    exit: segment.exit,
                    before_exit: cqt.before_exit,
                    slack: cqt.slack(now),
                }),
        }
    }

    /// Records the event `entry` describes now, if recording events. The entry is made only
    /// then: unrecorded runs pay one check per event.
    #[inline]
    pub(super) fn log(&mut self, now: Time, entry: impl FnOnce(&Self) -> Entry) {
        if let Some(recorder) = &self.recorder {
            let entry = entry(self);
            recorder.event(&mut self.records, now, entry);
        }
    }

    /// Records an outage event of `tool` now.
    fn log_tool(&mut self, now: Time, kind: EventKind, tool: ToolId) {
        self.log(now, |fab| Entry {
            kind,
            lot: None,
            part: None,
            tool: Some(tool),
            tool_group: Some(fab.tools[tool].group),
            step: None,
        });
    }

    pub(super) fn finished(&self) -> bool {
        self.finished.is_some()
    }

    pub(super) fn released(&self) -> u64 {
        self.released
    }

    pub(super) fn completed(&self) -> u64 {
        self.completed
    }

    pub(super) fn wip(&self) -> usize {
        self.wip
    }

    /// QTS flow factors measured up to the horizon; empty before it.
    pub(super) fn step_flow_factors(&self) -> &[Vec<Option<f64>>] {
        &self.step_flow_factors
    }

    pub(super) fn results(&self, events: u64) -> Results {
        Results {
            seed: self.seed,
            replication: self.replication,
            periods: self.reports.clone(),
            days: self.days.reports.clone(),
            released: self.released,
            completed: self.completed,
            end: self.finished.unwrap_or_default(),
            events,
            step_flow_factors: self.step_flow_factors.clone(),
        }
    }

    /// Setup duration from `from` to `to`: a specific change, else any-to-`to`.
    pub(super) fn setup_dist(&self, from: Option<SetupId>, to: SetupId) -> Option<Dist> {
        let changes = &self.setup_times[to];
        changes
            .iter()
            .find(|change| change.0.is_some() && change.0 == from)
            .or_else(|| changes.iter().find(|change| change.0.is_none()))
            .map(|change| change.1)
    }

    pub(super) fn group_of(&self, lot: LotId) -> ToolGroupId {
        let lot = &self.lots[lot];
        self.data.routes[lot.route].steps[lot.step].tool_group
    }

    // ---- releases and lot moves ----

    fn release_stream(&mut self, index: usize, sched: &mut Scheduler<Event>) {
        let stream = self.plan.streams[index];
        let now = sched.now();
        for _ in 0..stream.lots {
            let lot = LotRelease {
                part: stream.part,
                priority: stream.priority,
                wafers: stream.wafers,
                start: now,
                due: now + stream.due_offset,
                reserve: stream.reserve,
                step: None,
            };
            self.release(lot, sched);
        }
        let done = self.plan.stream_next[index] + 1;
        self.plan.stream_next[index] = done;
        if done < releases_before(&stream, self.horizon) {
            sched.schedule_at(
                stream.start + Time::from(done) * stream.interval,
                Event::Stream(index),
            );
        }
    }

    fn release_listed(&mut self, sched: &mut Scheduler<Event>) {
        let lot = self.plan.listed[self.plan.listed_next];
        self.plan.listed_next += 1;
        self.release(lot, sched);
        if let Some(next) = self.plan.listed.get(self.plan.listed_next) {
            sched.schedule_at(next.start, Event::Listed);
        }
    }

    fn release(&mut self, spec: LotRelease, sched: &mut Scheduler<Event>) {
        let now = sched.now();
        let part = &self.data.parts[spec.part];
        let kind = LotKind::of(part.engineering, spec.priority).expect("checked by the plan");
        let lot = Lot {
            alive: true,
            part: spec.part,
            route: part.route,
            kind,
            priority: self.strategy.priority(kind, spec.priority),
            wafers: spec.wafers,
            release: now,
            due: spec.due,
            serial: self.serial,
            step: spec.step.unwrap_or(0),
            state: LotState::Moving,
            last_done: now,
            location: None,
            dedicated: Vec::new(),
            segment: None,
            reserve: spec.reserve || (self.reserve_super_hot && kind == LotKind::Shl),
            reservation: None,
        };
        let id = match self.free.pop() {
            Some(id) => {
                // The freed lot's buffers serve the new one.
                let mut dedicated = mem::take(&mut self.lots[id].dedicated);
                dedicated.clear();
                self.lots[id] = Lot { dedicated, ..lot };
                self.times[id].visits.clear();
                id
            }
            None => {
                self.lots.push(lot);
                self.times.push(LotTimes::default());
                self.lots.len() - 1
            }
        };
        self.log(now, |fab| Entry {
            kind: EventKind::Release,
            lot: Some(fab.serial),
            part: Some(spec.part),
            tool: None,
            tool_group: None,
            step: spec.step,
        });
        self.serial += 1;
        self.plan.remaining[spec.part] -= 1;
        self.released += 1;
        self.stats.started(spec.part, kind);
        self.stats.wip(now, self.wip);
        self.days.wip(now, self.wip);
        self.wip += 1;
        let skipped = match &self.logistics {
            Some(logistics) => {
                let skipped = logistics.skipped[self.group_of(id)];
                self.place_released(id, spec.step.is_some(), now);
                skipped
            }
            None => false,
        };
        if spec.step.is_some() && !skipped {
            // Initial WIP waits at its current step.
            sched.schedule_in(0, Event::Arrive(id));
        } else {
            self.advance(id, sched);
        }
    }

    /// Sends a lot to its next processed step (metrology sampling may skip steps), or completes it.
    fn advance(&mut self, id: LotId, sched: &mut Scheduler<Event>) {
        let steps = &self.data.routes[self.lots[id].route].steps;
        let mut step = self.lots[id].step;
        let skipped = self
            .logistics
            .as_ref()
            .map(|logistics| logistics.skipped.as_slice());
        while step < steps.len()
            && (skipped.is_some_and(|skipped| skipped[steps[step].tool_group])
                || !self.rng.chance(Purpose::Sampling, steps[step].sampling))
        {
            step += 1;
        }
        // Tool group of the next processed step; none past the last step.
        let next = steps.get(step).map(|next| next.tool_group);
        self.lots[id].step = step;
        if let Some(group) = self.lots[id].reservation
            && self.groups[group]
                .reservation
                .as_ref()
                .is_some_and(|reservation| reservation.step != step)
        {
            self.release_reservation(group, sched);
        }
        let Some(group) = next else {
            if self.logistics.is_some() {
                // Out through a complete station; the status shows the last step.
                self.lots[id].step = self.data.routes[self.lots[id].route].steps.len() - 1;
                self.leave(id, sched);
            } else {
                self.complete(id, sched);
            }
            return;
        };
        let to = self.data.tool_groups[group].location;
        let transport = self.lots[id]
            .location
            .and_then(|from| self.transports[from * self.data.locations.len() + to])
            .filter(|_| self.logistics.is_none());
        let delay = transport.map_or(0, |time| self.rng.sample(Purpose::Transport, time));
        self.lots[id].state = LotState::Moving;
        self.count_segment(id, 1);
        sched.schedule_in(delay, Event::Arrive(id));
    }

    fn arrive(&mut self, id: LotId, sched: &mut Scheduler<Event>) {
        let now = sched.now();
        self.count_segment(id, -1);
        self.lots[id].state = LotState::Queued;
        self.times[id].arrived = now;
        self.times[id].moved = 0;
        self.count_segment(id, 1);
        let group = self.group_of(id);
        self.log(now, |fab| Entry {
            kind: EventKind::Arrive,
            lot: Some(fab.lots[id].serial),
            part: Some(fab.lots[id].part),
            tool: None,
            tool_group: Some(group),
            step: Some(fab.lots[id].step),
        });
        let mut waiting = self.waiting(id, now);
        if self.strategy.code_ranked[group] && self.code_failure.is_none() {
            let lot = self.lot_view(&waiting, now);
            let code = self.code.as_deref_mut().expect("code with priority");
            waiting.code = match code.priority(&lot, now) {
                Ok(value) if !value.is_nan() => value,
                Ok(_) => {
                    Self::record_failure(&mut self.code_failure, "priority returned NaN".into());
                    0.0
                }
                Err(message) => {
                    Self::record_failure(&mut self.code_failure, format!("priority: {message}"));
                    0.0
                }
            };
        }
        self.groups[group].integrate_queue(now);
        self.groups[group].queue.push(waiting);
        // AMHS: a tool of the group takes the lot at its port without a free one.
        if self.logistics.is_some()
            && let Some(tool) = self.standing_at(id)
            && self.tools[tool].group == group
        {
            self.refresh(tool, sched);
        }
        if let Some(reservation) = &self.groups[group].reservation
            && reservation.lot == id
            && let Some(tool) = reservation.tool
        {
            self.selected.clear();
            self.selected.push(id);
            if self.logistics.is_some() {
                self.assign(tool, sched);
            } else {
                self.start_job(tool, sched);
            }
            return;
        }
        self.dispatch(group, Some(id), sched);
        if self.logistics.is_some() {
            self.settle(id, sched);
        }
    }

    /// Queue entry of a lot arriving at its step now.
    fn waiting(&self, id: LotId, now: Time) -> Waiting {
        let lot = &self.lots[id];
        let step = &self.data.routes[lot.route].steps[lot.step];
        let info = &self.routes.info[lot.route];
        let remaining = |step: StepIndex| info.remaining[step].at(lot.wafers);
        let cqt = lot.segment.as_ref().map(|segment| WaitingCqt {
            deadline: segment.entered + segment.limit,
            work: remaining(lot.step) - remaining(segment.exit + 1),
            before_exit: remaining(lot.step) - remaining(segment.exit),
            latest: self
                .strategy
                .flow_factors
                .as_ref()
                .map_or(0.0, |flow_factors| {
                    self.qts_deadline(lot, segment, flow_factors)
                }),
        });
        Waiting {
            lot: id,
            route: lot.route,
            step: lot.step,
            kind: lot.kind,
            priority: lot.priority,
            wafers: lot.wafers,
            serial: lot.serial,
            queued_at: now,
            due: lot.due,
            remaining: remaining(lot.step),
            step_time: info.step[lot.step].at(lot.wafers),
            setup: step.setup,
            dedicated: lot
                .dedicated
                .iter()
                .find(|&&(step, _)| step == lot.step)
                .map(|&(_, tool)| tool),
            batch: self.routes.batch_key[lot.part][lot.step],
            cqt,
            code: 0.0,
            // A lot leaving its previous segment here is exempt.
            entering: self.counts.is_some()
                && step.cqt.is_some()
                && lot
                    .segment
                    .as_ref()
                    .is_none_or(|segment| segment.exit != lot.step),
            held: false,
        }
    }

    /// [P2] eq. (2)–(6): latest start of the lot's current step in its segment, with p_k the
    /// sampling-weighted expected step durations.
    fn qts_deadline(&self, lot: &Lot, segment: &Segment, flow_factors: &[Vec<f64>]) -> f64 {
        let deadline = (segment.entered + segment.limit) as f64;
        if lot.step >= segment.exit {
            return deadline;
        }
        let info = &self.routes.info[lot.route];
        let p = |k: usize| info.sampling[k] * info.step[k].at(lot.wafers);
        let ff = |k: usize| flow_factors[lot.route][k];
        let span = (segment.entry + 1..segment.exit)
            .map(|k| ff(k) * p(k))
            .sum::<f64>()
            + (ff(segment.exit) - 1.0) * p(segment.exit);
        let allocated: f64 = (segment.entry + 1..=lot.step).map(|k| ff(k) * p(k)).sum();
        let share = if span > 0.0 { allocated / span } else { 0.0 };
        segment.entered as f64 + segment.limit as f64 * share - p(lot.step)
    }

    pub(super) fn complete(&mut self, id: LotId, sched: &mut Scheduler<Event>) {
        let now = sched.now();
        let lot = &mut self.lots[id];
        let cycle_time = now - lot.release;
        let rpt = self.routes.info[lot.route].rpt.at(lot.wafers);
        self.stats.completed(
            lot.part,
            lot.kind,
            cycle_time,
            cycle_time as f64 / rpt,
            now <= lot.due,
        );
        lot.alive = false;
        self.free.push(id);
        self.stats.wip(now, self.wip);
        self.days.wip(now, self.wip);
        self.wip -= 1;
        self.completed += 1;
        self.log(now, |fab| Entry {
            kind: EventKind::Complete,
            lot: Some(fab.lots[id].serial),
            part: Some(fab.lots[id].part),
            tool: None,
            tool_group: None,
            step: None,
        });
        if !self.releasing && self.wip == 0 {
            self.finish(sched);
        }
    }

    // ---- jobs ----

    /// Starts the lots in `selected` (one lot or a batch) on `tool`.
    pub(super) fn start_job(&mut self, tool_id: ToolId, sched: &mut Scheduler<Event>) {
        let now = sched.now();
        let lots = mem::take(&mut self.selected);
        let group_id = self.group_of(lots[0]);
        self.groups[group_id].integrate_queue(now);
        self.groups[group_id]
            .queue
            .retain(|waiting| !lots.contains(&waiting.lot));
        self.begin_job(tool_id, lots, true, sched);
    }

    /// Starts `lots` (one lot or a batch, out of the queue) on `tool`: selected just now
    /// (`dispatched`), or assigned to the tool earlier and come to its ports (AMHS), whose
    /// projected setup, run and campaign count were taken then.
    pub(super) fn begin_job(
        &mut self,
        tool_id: ToolId,
        lots: Vec<LotId>,
        dispatched: bool,
        sched: &mut Scheduler<Event>,
    ) {
        let now = sched.now();
        let head = &self.lots[lots[0]];
        let (route, step_index) = (head.route, head.step);
        let step = &self.data.routes[route].steps[step_index];
        let group_id = step.tool_group;
        let group = &self.data.tool_groups[group_id];
        let rule = group.rule;

        self.tools[tool_id].account(now);
        let current = self.tools[tool_id].setup;
        let (next, run_left, sets_up) = after_job(
            &self.data,
            rule,
            current,
            self.tools[tool_id].run_left,
            step.setup,
        );
        let mut setup = 0;
        if sets_up {
            let needed = step.setup.expect("a setup");
            let dist = needed
                .time
                .map(Dist::Constant)
                .or_else(|| self.setup_dist(current, needed.setup));
            setup = dist.map_or(0, |dist| self.rng.sample(Purpose::Setup, dist));
        }
        let tool = &mut self.tools[tool_id];
        tool.setup = next;
        tool.run_left = run_left;
        if dispatched {
            tool.next_setup = next;
            tool.next_run_left = run_left;
        }
        let units = match step.unit {
            Unit::Wafer => lots.iter().map(|&id| self.lots[id].wafers).sum(),
            Unit::Lot | Unit::Batch => 1,
        };
        let work = Work {
            setup,
            load: group.load,
            process: self.rng.sample(Purpose::Process, step.time),
            units,
            cascade: step.cascade_interval,
            unload: group.unload,
        };
        let dedicate_to = step.dedicate_to;
        let moved = self.logistics.is_some();

        for &id in &lots {
            let lot = &mut self.lots[id];
            let times = &mut self.times[id];
            lot.state = LotState::Processing;
            times.started = now;
            if let Some(segment) = &lot.segment
                && segment.exit == step_index
            {
                // The wait ends: the exit step's arrival and queue complete its visits.
                let (transport, queue) = split_wait(times, lot.last_done, now, moved);
                times.visits.push(Visit {
                    step: step_index,
                    transport,
                    queue,
                    process: 0,
                });
                let violated = self.stats.cqt(
                    segment.id,
                    segment.entry,
                    now - segment.entered,
                    segment.limit,
                    &times.visits,
                );
                times.visits.clear();
                if violated && self.recorder.as_ref().is_some_and(Recorder::violations) {
                    let violations = &mut self.records.violations;
                    violations.lot.push(lot.serial);
                    violations.part.push(lot.part);
                    violations.kind.push(lot.kind);
                    violations.segment.push(segment.id);
                    violations.release.push(lot.release);
                    violations.entered.push(segment.entered);
                    violations.arrived.push(times.arrived);
                    violations.exit.push(now);
                }
            }
            if let Some(target) = dedicate_to {
                lot.dedicated.retain(|&(step, _)| step != target);
                lot.dedicated.push((target, tool_id));
            }
            if dispatched
                && lot.kind.engineering()
                && self.groups[group_id].campaign > 0
                && self.strategy.steppers[group_id]
            {
                self.groups[group_id].campaign -= 1;
            }
            self.log(now, |fab| Entry {
                kind: EventKind::Start,
                lot: Some(fab.lots[id].serial),
                part: Some(fab.lots[id].part),
                tool: Some(tool_id),
                tool_group: Some(group_id),
                step: Some(step_index),
            });
        }

        let tool = &mut self.tools[tool_id];
        let epoch = tool.epoch;
        let (slot, end) = tool.start(lots, now, work);
        sched.schedule_at(
            end,
            Event::JobDone {
                tool: tool_id,
                slot,
                epoch,
            },
        );
        self.log_tool_states(tool_id, now);
        if !dispatched {
            // The setups and runs after the assignments left, from the tool's new ones.
            self.reproject(tool_id);
        }
        // A tool with room left goes to the back of the ready queue.
        self.set_unready(tool_id);
        self.refresh(tool_id, sched);

        for index in 0..self.tools[tool_id].jobs[slot].lots.len() {
            let id = self.tools[tool_id].jobs[slot].lots[index];
            if self.lots[id].reservation == Some(group_id) {
                self.release_reservation(group_id, sched);
            }
            if self.lots[id].reserve && rule == Rule::HotLotFirst {
                self.reserve_next(id, sched);
            }
        }
    }

    fn job_done(&mut self, tool_id: ToolId, slot: usize, epoch: u32, sched: &mut Scheduler<Event>) {
        let now = sched.now();
        let tool = &mut self.tools[tool_id];
        if tool.epoch != epoch || !tool.jobs[slot].active {
            return;
        }
        tool.account(now);
        let lots = tool.finish(slot);
        let wafers = lots.iter().map(|&id| self.lots[id].wafers).sum();
        let group = self.tools[tool_id].group;
        let pms = &self.data.tool_groups[group].pms;
        self.tools[tool_id].add_wafers(wafers, pms);
        for &id in &lots {
            self.log(now, |fab| Entry {
                kind: EventKind::End,
                lot: Some(fab.lots[id].serial),
                part: Some(fab.lots[id].part),
                tool: Some(tool_id),
                tool_group: Some(group),
                step: Some(fab.lots[id].step),
            });
            self.finish_step(id, sched);
        }
        if self.logistics.is_some() {
            if self.data.tool_groups[group].batching.is_some() {
                self.batch_done(tool_id, &lots, sched);
            }
            // A wafer-count PM fell due.
            if !self.tools[tool_id].pm_pending.is_empty() {
                self.release_assignments(tool_id, sched);
            }
        }
        self.selected = lots;
        self.selected.clear();
        self.tool_changed(tool_id, sched);
        self.log_tool_states(tool_id, now);
    }

    fn finish_step(&mut self, id: LotId, sched: &mut Scheduler<Event>) {
        let now = sched.now();
        self.count_segment(id, -1);
        let lot = &mut self.lots[id];
        let times = &mut self.times[id];
        let step = &self.data.routes[lot.route].steps[lot.step];
        let info = &self.routes.info[lot.route];
        self.stats.step(
            lot.route,
            lot.step,
            (now - lot.last_done) as f64 / info.step[lot.step].at(lot.wafers),
        );
        let moved = self.logistics.is_some();
        match &lot.segment {
            Some(segment) if segment.exit == lot.step => lot.segment = None,
            Some(_) => {
                let (transport, queue) = split_wait(times, lot.last_done, times.started, moved);
                times.visits.push(Visit {
                    step: lot.step,
                    transport,
                    queue,
                    process: now - times.started,
                });
            }
            None => {}
        }
        lot.last_done = now;
        lot.location = Some(self.data.tool_groups[step.tool_group].location);
        if let Some(cqt) = step.cqt {
            times.visits.clear();
            lot.segment = Some(Segment {
                id: self.routes.segment_at[lot.route][lot.step].expect("CQT step"),
                entry: lot.step,
                exit: cqt.until,
                entered: now,
                limit: cqt.limit,
            });
        }
        lot.step = match step.rework {
            Some(rework) if self.rng.chance(Purpose::Rework, rework.probability) => rework.to,
            _ => lot.step + 1,
        };
        self.advance(id, sched);
    }

    // ---- tool availability ----

    /// After a tool's jobs, outages or PMs change: starts a due PM on an empty tool that is up,
    /// updates readiness, dispatches.
    fn tool_changed(&mut self, tool_id: ToolId, sched: &mut Scheduler<Event>) {
        let tool = &self.tools[tool_id];
        if tool.pm.is_none()
            && !tool.pm_pending.is_empty()
            && tool.breakdowns == 0
            && tool.busy() == 0
        {
            self.start_pm(tool_id, sched);
        }
        if self.logistics.is_some() {
            self.try_start(tool_id, sched);
        }
        self.refresh(tool_id, sched);
        if self.tools[tool_id].ready {
            self.dispatch(self.tools[tool_id].group, None, sched);
        }
    }

    /// Lists an available tool in its group's ready queue (or holds it for a reservation), and
    /// unlists an unavailable one.
    pub(super) fn refresh(&mut self, tool_id: ToolId, sched: &mut Scheduler<Event>) {
        let tool = &self.tools[tool_id];
        let available = if self.logistics.is_some() {
            self.assignable(tool_id)
        } else {
            tool.available()
        };
        if !available || tool.held {
            self.set_unready(tool_id);
            return;
        }
        if tool.ready {
            return;
        }
        let group = tool.group;
        if self.groups[group]
            .reservation
            .as_ref()
            .is_some_and(|reservation| reservation.tool.is_none())
        {
            self.hold(group, tool_id, sched);
        } else {
            self.tools[tool_id].ready = true;
            self.groups[group].ready.push_back(tool_id);
        }
    }

    pub(super) fn set_unready(&mut self, tool_id: ToolId) {
        let tool = &mut self.tools[tool_id];
        if tool.ready {
            tool.ready = false;
            let ready = &mut self.groups[tool.group].ready;
            let position = ready
                .iter()
                .position(|&id| id == tool_id)
                .expect("listed tool");
            ready.remove(position);
        }
    }

    fn fail(&mut self, tool_id: ToolId, breakdown: usize, sched: &mut Scheduler<Event>) {
        let now = sched.now();
        let tool = &mut self.tools[tool_id];
        // Outages never overlap: a breakdown falling due during a PM begins when the PM ends.
        if tool.pm.is_some() {
            tool.deferred.push(breakdown);
            return;
        }
        tool.account(now);
        tool.breakdowns += 1;
        tool.pause(now);
        let ttr = self.data.tool_groups[tool.group].breakdowns[breakdown].ttr;
        sched.schedule_in(
            self.rng.sample(Purpose::Breakdown, ttr),
            Event::Repair {
                tool: tool_id,
                breakdown,
            },
        );
        self.log_tool_states(tool_id, now);
        self.log_tool(now, EventKind::Down, tool_id);
        self.unhold(tool_id);
        self.refresh(tool_id, sched);
        if self.logistics.is_some() {
            self.release_assignments(tool_id, sched);
        }
    }

    fn repair(&mut self, tool_id: ToolId, breakdown: usize, sched: &mut Scheduler<Event>) {
        let now = sched.now();
        self.log_tool(now, EventKind::Up, tool_id);
        let tool = &mut self.tools[tool_id];
        tool.account(now);
        tool.breakdowns -= 1;
        let ttf = self.data.tool_groups[tool.group].breakdowns[breakdown].ttf;
        sched.schedule_in(
            self.rng.sample(Purpose::Breakdown, ttf),
            Event::Fail {
                tool: tool_id,
                breakdown,
            },
        );
        if tool.breakdowns == 0 && tool.resume(now) {
            for (slot, job) in tool.jobs.iter().enumerate().filter(|(_, job)| job.active) {
                sched.schedule_at(
                    job.end,
                    Event::JobDone {
                        tool: tool_id,
                        slot,
                        epoch: tool.epoch,
                    },
                );
            }
        }
        self.tool_changed(tool_id, sched);
        self.log_tool_states(tool_id, now);
    }

    fn pm_due(&mut self, tool_id: ToolId, pm: usize, sched: &mut Scheduler<Event>) {
        let group = &self.data.tool_groups[self.tools[tool_id].group];
        if let PmTrigger::Calendar { interval, .. } = group.pms[pm].trigger {
            sched.schedule_in(interval, Event::PmDue { tool: tool_id, pm });
        }
        self.tools[tool_id].request_pm(pm);
        if self.logistics.is_some() {
            self.release_assignments(tool_id, sched);
        }
        self.tool_changed(tool_id, sched);
    }

    fn start_pm(&mut self, tool_id: ToolId, sched: &mut Scheduler<Event>) {
        let pms = &self.data.tool_groups[self.tools[tool_id].group].pms;
        let tool = &mut self.tools[tool_id];
        tool.account(sched.now());
        let pm = tool.start_pm(pms);
        sched.schedule_in(
            self.rng.sample(Purpose::Pm, pms[pm].duration),
            Event::PmDone(tool_id),
        );
        self.log_tool(sched.now(), EventKind::PmStart, tool_id);
        self.log_tool_states(tool_id, sched.now());
        self.unhold(tool_id);
    }

    fn pm_done(&mut self, tool_id: ToolId, sched: &mut Scheduler<Event>) {
        self.log_tool(sched.now(), EventKind::PmEnd, tool_id);
        let tool = &mut self.tools[tool_id];
        tool.account(sched.now());
        tool.pm = None;
        for breakdown in mem::take(&mut tool.deferred) {
            self.fail(tool_id, breakdown, sched);
        }
        self.tool_changed(tool_id, sched);
        self.log_tool_states(tool_id, sched.now());
    }

    // ---- super hot lot reservations ----

    /// Reserves a tool of the next step's group for the lot (`rule_HotLotFIRST` groups only).
    fn reserve_next(&mut self, id: LotId, sched: &mut Scheduler<Event>) {
        let lot = &self.lots[id];
        let step = lot.step + 1;
        let Some(next) = self.data.routes[lot.route].steps.get(step) else {
            return;
        };
        let group = next.tool_group;
        if self.data.tool_groups[group].rule != Rule::HotLotFirst
            || self.groups[group].reservation.is_some()
        {
            return;
        }
        self.groups[group].reservation = Some(Reservation {
            lot: id,
            step,
            tool: None,
        });
        self.lots[id].reservation = Some(group);
        if let Some(&tool) = self.groups[group].ready.front() {
            self.set_unready(tool);
            self.hold(group, tool, sched);
        }
    }

    /// Keeps `tool` free for the group's reservation; starts the lot if it already waits.
    fn hold(&mut self, group: ToolGroupId, tool: ToolId, sched: &mut Scheduler<Event>) {
        self.tools[tool].held = true;
        let reservation = self.groups[group]
            .reservation
            .as_mut()
            .expect("reservation");
        reservation.tool = Some(tool);
        let lot = reservation.lot;
        if self.lots[lot].state == LotState::Queued
            && self.group_of(lot) == group
            && self.assigned_tool(lot).is_none()
        {
            self.selected.clear();
            self.selected.push(lot);
            if self.logistics.is_some() {
                self.assign(tool, sched);
            } else {
                self.start_job(tool, sched);
            }
        }
    }

    /// A held tool that goes down or into PM stops waiting; the reservation waits for another.
    pub(super) fn unhold(&mut self, tool: ToolId) {
        if mem::take(&mut self.tools[tool].held) {
            let group = &mut self.groups[self.tools[tool].group];
            group.reservation.as_mut().expect("reservation").tool = None;
        }
    }

    fn release_reservation(&mut self, group: ToolGroupId, sched: &mut Scheduler<Event>) {
        let Some(reservation) = self.groups[group].reservation.take() else {
            return;
        };
        self.lots[reservation.lot].reservation = None;
        if let Some(tool) = reservation.tool {
            self.tools[tool].held = false;
            self.tool_changed(tool, sched);
        }
    }

    // ---- CQT segment counts for stopping and admission code ----

    /// Adds (`delta` = 1) or removes (−1) a constrained lot's contribution to the segment counts:
    /// in front of the group it is queued or processing at, upstream of every other group it still
    /// reaches in its segment (a moving lot also of the group it moves to); once per group.
    fn count_segment(&mut self, id: LotId, delta: i64) {
        let Some(counts) = &mut self.counts else {
            return;
        };
        let lot = &self.lots[id];
        let Some(segment) = &lot.segment else {
            return;
        };
        let steps = &self.data.routes[lot.route].steps;
        let at = (lot.state != LotState::Moving).then(|| steps[lot.step].tool_group);
        if let Some(group) = at {
            counts.count(group, true, delta);
        }
        let ahead = &steps[lot.step..=segment.exit];
        for (index, step) in ahead.iter().enumerate() {
            let group = step.tool_group;
            if Some(group) != at
                && ahead[..index]
                    .iter()
                    .all(|earlier| earlier.tool_group != group)
            {
                counts.count(group, false, delta);
            }
        }
    }

    // ---- periods ----

    fn period_end(&mut self, index: usize, sched: &mut Scheduler<Event>) {
        let now = sched.now();
        let period = &self.periods[index];
        let (name, report, reset) = (period.name.clone(), period.report, period.reset);
        if now == self.horizon {
            self.step_flow_factors = self.stats.step_flow_factors();
            self.releasing = false;
        }
        self.snapshot(name, now, report, reset);
        if !self.releasing && self.wip == 0 {
            self.finish(sched);
        }
    }

    /// The last lot is complete: reports the drain and the last day and ends the run.
    fn finish(&mut self, sched: &mut Scheduler<Event>) {
        let now = sched.now();
        self.snapshot("Drain".into(), now, true, false);
        self.close_day(now);
        self.finished = Some(now);
        sched.stop();
    }

    /// Closes the open day at `at`; with tool groups recorded, records their day.
    fn close_day(&mut self, at: Time) {
        let day = self.days.reports.len();
        let length = at - (self.days.end - DAY);
        self.days.close(
            at,
            self.wip,
            self.released,
            self.completed,
            &self.stats.cqt_total,
        );
        let Some(recorder) = &mut self.recorder else {
            return;
        };
        if !recorder.tool_groups() {
            return;
        }
        let mut totals = vec![[0; STATES]; self.groups.len()];
        for tool in &mut self.tools {
            tool.account(at);
            for (total, time) in totals[tool.group].iter_mut().zip(tool.total) {
                *total += time;
            }
        }
        let rows = &mut self.records.tool_groups;
        for (index, (group, total)) in self.groups.iter_mut().zip(totals).enumerate() {
            group.integrate_queue(at);
            let (before, area_before) = recorder.day_start[index];
            let time = |state: ToolState| total[state as usize] - before[state as usize];
            rows.day.push(day);
            rows.tool_group.push(index);
            rows.queue.push(if length > 0 {
                (group.queue_area - area_before) / length as f64
            } else {
                0.0
            });
            rows.down.push(time(ToolState::Down));
            rows.pm.push(time(ToolState::Pm));
            rows.setup.push(time(ToolState::Setup));
            rows.process.push(time(ToolState::Process));
            rows.load.push(time(ToolState::Load));
            rows.unload.push(time(ToolState::Unload));
            rows.idle.push(time(ToolState::Idle));
            recorder.day_start[index] = (total, group.queue_area);
        }
    }

    /// Closes the statistics window at `now`: reports it (REPORT = yes) and restarts it
    /// (RESET = yes).
    fn snapshot(&mut self, name: String, now: Time, report: bool, reset: bool) {
        for tool in &mut self.tools {
            tool.account(now);
        }
        self.stats.wip(now, self.wip);
        let amhs = self
            .logistics
            .as_mut()
            .map(|logistics| logistics.system.report(now));
        if report {
            self.reports.push(self.stats.report(
                name,
                now,
                &self.data,
                &self.routes,
                &self.tools,
                amhs,
            ));
        }
        if reset {
            if let Some(logistics) = &mut self.logistics {
                logistics.system.reset();
            }
            self.stats.reset(now);
            for tool in &mut self.tools {
                tool.time = [0; STATES];
            }
        }
    }
}

impl Model for Fab {
    type Event = Event;

    fn init(&mut self, sched: &mut Scheduler<Event>) {
        for (index, stream) in self.plan.streams.iter().enumerate() {
            if releases_before(stream, self.horizon) > 0 {
                sched.schedule_at(stream.start, Event::Stream(index));
            }
        }
        if let Some(first) = self.plan.listed.first() {
            sched.schedule_at(first.start, Event::Listed);
        }
        let mut position = 0;
        for (tool_id, tool) in self.tools.iter().enumerate() {
            let group = &self.data.tool_groups[tool.group];
            // Tools are listed group by group: this is the `position`-th of the group.
            position = if tool_id > 0 && self.tools[tool_id - 1].group == tool.group {
                position + 1
            } else {
                1
            };
            for (breakdown, spec) in group.breakdowns.iter().enumerate() {
                let first = self.rng.sample(Purpose::Breakdown, spec.first);
                sched.schedule_at(
                    first,
                    Event::Fail {
                        tool: tool_id,
                        breakdown,
                    },
                );
            }
            for (pm, spec) in group.pms.iter().enumerate() {
                if let PmTrigger::Calendar { first, .. } = spec.trigger {
                    let due = first * Time::from(position) / Time::from(group.tools);
                    sched.schedule_at(due, Event::PmDue { tool: tool_id, pm });
                }
            }
        }
        for (index, period) in self.periods.iter().enumerate() {
            sched.schedule_at(period.end, Event::PeriodEnd(index));
        }
        sched.schedule_at(self.horizon + DRAIN_LIMIT, Event::Deadline);
        if let Some(logistics) = &mut self.logistics {
            logistics.system.init(sched);
        }
    }

    fn handle(&mut self, event: Event, sched: &mut Scheduler<Event>) {
        // Days end before the first event at or after their end, a replay's window begins so.
        while sched.now() >= self.days.end {
            self.close_day(self.days.end);
        }
        self.begin_replay(sched.now());
        match event {
            Event::Stream(index) => self.release_stream(index, sched),
            Event::Listed => self.release_listed(sched),
            Event::Arrive(lot) => self.arrive(lot, sched),
            Event::JobDone { tool, slot, epoch } => self.job_done(tool, slot, epoch, sched),
            Event::Fail { tool, breakdown } => self.fail(tool, breakdown, sched),
            Event::Repair { tool, breakdown } => self.repair(tool, breakdown, sched),
            Event::PmDue { tool, pm } => self.pm_due(tool, pm, sched),
            Event::PmDone(tool) => self.pm_done(tool, sched),
            Event::Wake(group) => {
                // An earlier wake may have replaced this one; dispatching again changes nothing.
                if self.groups[group].wake == Some(sched.now()) {
                    self.groups[group].wake = None;
                }
                self.dispatch(group, None, sched);
            }
            Event::PeriodEnd(index) => self.period_end(index, sched),
            Event::Vehicle { vehicle, epoch } => self.vehicle_event(vehicle, epoch, sched),
            // A finished run stops before its deadline.
            Event::Deadline => sched.stop(),
        }
        // Counts that went down in the event release held lots: all of them when a stopping limit
        // no longer holds, those of admission code waiting on these groups otherwise.
        if let Some(counts) = &mut self.counts {
            let mut fewer = mem::take(&mut self.fewer);
            fewer.clear();
            let released = counts.settle(self.strategy.stopping.as_ref(), &mut fewer);
            if released || (self.hooks.admit && !fewer.is_empty()) {
                // In dataset order, whatever order the groups began holding lots in.
                let mut stopped = mem::take(&mut self.stopped);
                stopped.sort_unstable();
                for group in stopped {
                    let waits = || self.groups[group].held_on.iter().any(|g| fewer.contains(g));
                    if released || waits() {
                        self.dispatch(group, None, sched);
                    } else if !self.stopped.contains(&group) {
                        self.stopped.push(group);
                    }
                }
            }
            self.fewer = fewer;
        }
        if self.code_failure.is_some() || self.amhs_failure().is_some() {
            sched.stop();
        }
    }
}

/// A tool's setup and lots left in its setup run after a job needing `needed` under `rule`, from
/// `setup` and `run_left`, and whether the job sets the tool up: a new setup run must reach the
/// group's minimum run length (`rule_LSSU`).
pub(super) fn after_job(
    data: &Dataset,
    rule: Rule,
    setup: Option<SetupId>,
    run_left: u32,
    needed: Option<StepSetup>,
) -> (Option<SetupId>, u32, bool) {
    let (mut next, mut run_left) = (setup, run_left);
    let sets_up = needed.is_some_and(|needed| needed.always || setup != Some(needed.setup));
    if sets_up {
        let needed = needed.expect("a setup");
        if let Rule::SetupRun(setup_group) = rule
            && setup != Some(needed.setup)
        {
            run_left = data.setup_groups[setup_group]
                .min_run
                .iter()
                .find(|run| run.0 == needed.setup)
                .map_or(0, |run| run.1);
        }
        next = Some(needed.setup);
    }
    if let Rule::SetupRun(_) = rule {
        run_left = run_left.saturating_sub(1);
    }
    (next, run_left, sets_up)
}

/// Transport and queue time of a step's wait from the end of the previous step `done` to `start`:
/// up to the arrival in the queue and after it, or with an AMHS (`moved`) the transports' time
/// and the rest.
fn split_wait(times: &LotTimes, done: Time, start: Time, moved: bool) -> (Time, Time) {
    if moved {
        (times.moved, start - done - times.moved)
    } else {
        (times.arrived - done, start - times.arrived)
    }
}
