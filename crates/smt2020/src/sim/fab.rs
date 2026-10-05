//! The fab as a `des_core` model: releases, lot moves, tool jobs, outages and period reports.

use std::collections::VecDeque;
use std::mem;

use des_core::{Model, Scheduler, Time};

use super::dispatch::Key;
use super::plan::{Plan, releases_before};
use super::routes::Routes;
use super::stats::{LotKind, PeriodReport, Results, Stats};
use super::strategy::{QueueTime, Stopping, Strategy};
use super::tool::{STATES, Tool, Work};
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
    /// End of the reporting period with this index.
    PeriodEnd(usize),
    /// [`DRAIN_LIMIT`] after the horizon: the run stops unfinished.
    Deadline,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum LotState {
    Moving,
    Queued,
    Processing,
}

/// Open CQT segment: from the end of step `entry` to the start of step `exit`.
pub(super) struct Segment {
    pub entry: StepIndex,
    pub exit: StepIndex,
    pub entered: Time,
    pub limit: Time,
    pub litho: bool,
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

pub(super) struct Reservation {
    pub lot: LotId,
    pub step: StepIndex,
    pub tool: Option<ToolId>,
}

/// A lot queued at a tool group with everything its ranking reads, fixed on arrival: dispatching
/// scans these entries instead of lots and route data.
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
    /// Expected remaining work from this step (critical ratio).
    pub remaining: f64,
    pub setup: Option<StepSetup>,
    /// Lot-to-lens dedication: the only tool that may process the lot here.
    pub dedicated: Option<ToolId>,
    /// Batch compatibility key (batch steps).
    pub batch: Option<usize>,
    pub urgency: Urgency,
    /// The lot enters a CQT segment here, so stopping limits apply.
    pub entering: bool,
    /// Held by stopping, as of the group's current dispatch.
    pub held: bool,
}

/// Queue-time urgency inputs of a waiting lot.
#[derive(Clone, Copy)]
pub(super) enum Urgency {
    /// No queue-time rule.
    None,
    /// Outside CQT segments: ranks after every constrained lot.
    Outside,
    /// QTCR: segment end date and expected work up to the exit step.
    Qtcr { deadline: Time, work: f64 },
    /// QTS: latest start of the step.
    Qts(f64),
}

#[derive(Default)]
pub(super) struct Group {
    pub queue: Vec<Waiting>,
    /// Available tools, longest available first.
    pub ready: VecDeque<ToolId>,
    pub reservation: Option<Reservation>,
    /// CoT: engineering lots still to start in the current campaign.
    pub campaign: u32,
}

struct Period {
    name: String,
    end: Time,
    report: bool,
    reset: bool,
}

pub(super) struct Fab<'a> {
    pub data: &'a Dataset,
    pub routes: Routes,
    pub strategy: Strategy,
    pub plan: Plan,
    horizon: Time,
    periods: Vec<Period>,
    reserve_super_hot: bool,
    pub rng: Streams,
    pub lots: Vec<Lot>,
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
    step_flow_factors: Vec<Vec<Option<f64>>>,
    released: u64,
    completed: u64,
    wip: usize,
    /// Releases continue until the horizon.
    releasing: bool,
    finished: Option<Time>,
    /// Groups with lots held by stopping, dispatched again when a stopping limit is released.
    pub stopped: Vec<ToolGroupId>,
    /// Scratch buffers of dispatching; candidates index the group queue.
    pub selected: Vec<LotId>,
    pub candidates: Vec<(Key, usize)>,
    pub order: Vec<ToolId>,
}

impl<'a> Fab<'a> {
    pub(super) fn new(
        data: &'a Dataset,
        config: &Config,
        flow_factors: Option<&[Vec<Option<f64>>]>,
    ) -> Result<Self, Error> {
        if config.horizon <= 0 {
            return Err(Error("the horizon must be positive".into()));
        }
        let strategy = Strategy::new(data, config, flow_factors)?;
        let routes = Routes::new(data, &strategy.steppers);
        let plan = Plan::new(data, config.horizon, config.load)?;

        let mut periods = Vec::new();
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

        Ok(Self {
            data,
            routes,
            strategy,
            plan,
            horizon: config.horizon,
            periods,
            reserve_super_hot: config.reserve_super_hot,
            rng: Streams::new(config.seed, config.replication),
            lots: Vec::new(),
            free: Vec::new(),
            serial: 0,
            tools,
            groups,
            setup_times,
            transports,
            stats: Stats::new(data),
            reports: Vec::new(),
            step_flow_factors: Vec::new(),
            released: 0,
            completed: 0,
            wip: 0,
            releasing: true,
            finished: None,
            stopped: Vec::new(),
            selected: Vec::new(),
            candidates: Vec::new(),
            order: Vec::new(),
        })
    }

    pub(super) fn finished(&self) -> bool {
        self.finished.is_some()
    }

    pub(super) fn wip(&self) -> usize {
        self.wip
    }

    pub(super) fn results(&self, events: u64) -> Results {
        Results {
            periods: self.reports.clone(),
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
                let mut dedicated = mem::take(&mut self.lots[id].dedicated);
                dedicated.clear();
                self.lots[id] = Lot { dedicated, ..lot };
                id
            }
            None => {
                self.lots.push(lot);
                self.lots.len() - 1
            }
        };
        self.serial += 1;
        self.plan.remaining[spec.part] -= 1;
        self.released += 1;
        self.stats.started(spec.part, kind);
        self.stats.wip(now, self.wip);
        self.wip += 1;
        if spec.step.is_some() {
            // Initial WIP waits at its current step.
            sched.schedule_in(0, Event::Arrive(id));
        } else {
            self.advance(id, sched);
        }
    }

    /// Sends a lot to its next processed step (metrology sampling may skip steps), or completes it.
    fn advance(&mut self, id: LotId, sched: &mut Scheduler<Event>) {
        let data = self.data;
        let steps = &data.routes[self.lots[id].route].steps;
        let mut step = self.lots[id].step;
        while step < steps.len() && !self.rng.chance(Purpose::Sampling, steps[step].sampling) {
            step += 1;
        }
        self.lots[id].step = step;
        if let Some(group) = self.lots[id].reservation
            && self.groups[group]
                .reservation
                .as_ref()
                .is_some_and(|reservation| reservation.step != step)
        {
            self.release_reservation(group, sched);
        }
        if step == steps.len() {
            self.complete(id, sched);
            return;
        }
        let to = data.tool_groups[steps[step].tool_group].location;
        let transport = self.lots[id]
            .location
            .and_then(|from| self.transports[from * data.locations.len() + to]);
        let delay = transport.map_or(0, |time| self.rng.sample(Purpose::Transport, time));
        self.lots[id].state = LotState::Moving;
        self.count_segment(id, 1);
        sched.schedule_in(delay, Event::Arrive(id));
    }

    fn arrive(&mut self, id: LotId, sched: &mut Scheduler<Event>) {
        self.count_segment(id, -1);
        self.lots[id].state = LotState::Queued;
        self.count_segment(id, 1);
        let group = self.group_of(id);
        let waiting = self.waiting(id, sched.now());
        self.groups[group].queue.push(waiting);
        if let Some(reservation) = &self.groups[group].reservation
            && reservation.lot == id
            && let Some(tool) = reservation.tool
        {
            self.selected.clear();
            self.selected.push(id);
            self.start_job(tool, sched);
            return;
        }
        self.dispatch(group, Some(id), sched);
    }

    /// Queue entry of a lot arriving at its step now.
    fn waiting(&self, id: LotId, now: Time) -> Waiting {
        let lot = &self.lots[id];
        let step = &self.data.routes[lot.route].steps[lot.step];
        let info = &self.routes.info[lot.route];
        let urgency = match (&self.strategy.queue_time, &lot.segment) {
            (QueueTime::None, _) => Urgency::None,
            (_, None) => Urgency::Outside,
            (QueueTime::Qtcr, Some(segment)) => Urgency::Qtcr {
                deadline: segment.entered + segment.limit,
                work: info.remaining[lot.step].at(lot.wafers)
                    - info.remaining[segment.exit + 1].at(lot.wafers),
            },
            (QueueTime::Qts(flow_factors), Some(segment)) => {
                Urgency::Qts(self.qts_deadline(lot, segment, flow_factors))
            }
        };
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
            remaining: info.remaining[lot.step].at(lot.wafers),
            setup: step.setup,
            dedicated: lot
                .dedicated
                .iter()
                .find(|&&(step, _)| step == lot.step)
                .map(|&(_, tool)| tool),
            batch: self.routes.batch_key[lot.part][lot.step],
            urgency,
            // A lot leaving its previous segment here is exempt.
            entering: self.strategy.stopping.is_some()
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
        let steps = &self.data.routes[lot.route].steps;
        let p = |k: usize| steps[k].sampling * info.step[k].at(lot.wafers);
        let ff = |k: usize| flow_factors[lot.route][k];
        let span = (segment.entry + 1..segment.exit)
            .map(|k| ff(k) * p(k))
            .sum::<f64>()
            + (ff(segment.exit) - 1.0) * p(segment.exit);
        let allocated: f64 = (segment.entry + 1..=lot.step).map(|k| ff(k) * p(k)).sum();
        let share = if span > 0.0 { allocated / span } else { 0.0 };
        segment.entered as f64 + segment.limit as f64 * share - p(lot.step)
    }

    fn complete(&mut self, id: LotId, sched: &mut Scheduler<Event>) {
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
        self.wip -= 1;
        self.completed += 1;
        if !self.releasing && self.wip == 0 {
            self.finish(sched);
        }
    }

    // ---- jobs ----

    /// Starts the lots in `selected` (one lot or a batch) on `tool`.
    pub(super) fn start_job(&mut self, tool_id: ToolId, sched: &mut Scheduler<Event>) {
        let data = self.data;
        let now = sched.now();
        let lots = mem::take(&mut self.selected);
        let head = &self.lots[lots[0]];
        let (route, step_index) = (head.route, head.step);
        let step = &data.routes[route].steps[step_index];
        let group_id = step.tool_group;
        let group = &data.tool_groups[group_id];
        self.groups[group_id]
            .queue
            .retain(|waiting| !lots.contains(&waiting.lot));

        self.tools[tool_id].account(now);
        let current = self.tools[tool_id].setup;
        let mut setup = 0;
        if let Some(needed) = step.setup
            && (needed.always || current != Some(needed.setup))
        {
            let dist = needed
                .time
                .map(Dist::Constant)
                .or_else(|| self.setup_dist(current, needed.setup));
            setup = dist.map_or(0, |dist| self.rng.sample(Purpose::Setup, dist));
            if let Rule::SetupRun(setup_group) = group.rule
                && current != Some(needed.setup)
            {
                // A new setup run must reach the group's minimum run length.
                self.tools[tool_id].run_left = data.setup_groups[setup_group]
                    .min_run
                    .iter()
                    .find(|run| run.0 == needed.setup)
                    .map_or(0, |run| run.1);
            }
            self.tools[tool_id].setup = Some(needed.setup);
        }
        if let Rule::SetupRun(_) = group.rule {
            let tool = &mut self.tools[tool_id];
            tool.run_left = tool.run_left.saturating_sub(1);
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

        for &id in &lots {
            let lot = &mut self.lots[id];
            lot.state = LotState::Processing;
            if let Some(segment) = &lot.segment
                && segment.exit == step_index
            {
                self.stats
                    .cqt(segment.litho, now - segment.entered, segment.limit);
            }
            if let Some(target) = step.dedicate_to {
                lot.dedicated.retain(|&(step, _)| step != target);
                lot.dedicated.push((target, tool_id));
            }
            if lot.kind.engineering()
                && self.groups[group_id].campaign > 0
                && self.strategy.steppers[group_id]
            {
                self.groups[group_id].campaign -= 1;
            }
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
        // A tool with room left goes to the back of the ready queue.
        self.set_unready(tool_id);
        self.refresh(tool_id, sched);

        for index in 0..self.tools[tool_id].jobs[slot].lots.len() {
            let id = self.tools[tool_id].jobs[slot].lots[index];
            if self.lots[id].reservation == Some(group_id) {
                self.release_reservation(group_id, sched);
            }
            if self.lots[id].reserve && group.rule == Rule::HotLotFirst {
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
        let pms = &self.data.tool_groups[self.tools[tool_id].group].pms;
        self.tools[tool_id].add_wafers(wafers, pms);
        for &id in &lots {
            self.finish_step(id, sched);
        }
        self.selected = lots;
        self.selected.clear();
        self.tool_changed(tool_id, sched);
    }

    fn finish_step(&mut self, id: LotId, sched: &mut Scheduler<Event>) {
        let data = self.data;
        let now = sched.now();
        self.count_segment(id, -1);
        let lot = &mut self.lots[id];
        let step = &data.routes[lot.route].steps[lot.step];
        let info = &self.routes.info[lot.route];
        self.stats.step(
            lot.route,
            lot.step,
            (now - lot.last_done) as f64 / info.step[lot.step].at(lot.wafers),
        );
        lot.last_done = now;
        lot.location = Some(data.tool_groups[step.tool_group].location);
        if lot
            .segment
            .as_ref()
            .is_some_and(|segment| segment.exit == lot.step)
        {
            lot.segment = None;
        }
        if let Some(cqt) = step.cqt {
            lot.segment = Some(Segment {
                entry: lot.step,
                exit: cqt.until,
                entered: now,
                limit: cqt.limit,
                litho: info.cqt_litho[lot.step],
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
        self.refresh(tool_id, sched);
        if self.tools[tool_id].ready {
            self.dispatch(self.tools[tool_id].group, None, sched);
        }
    }

    /// Lists an available tool in its group's ready queue (or holds it for a reservation), and
    /// unlists an unavailable one.
    fn refresh(&mut self, tool_id: ToolId, sched: &mut Scheduler<Event>) {
        let tool = &self.tools[tool_id];
        if !tool.available() || tool.held {
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

    fn set_unready(&mut self, tool_id: ToolId) {
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
        self.unhold(tool_id);
        self.refresh(tool_id, sched);
    }

    fn repair(&mut self, tool_id: ToolId, breakdown: usize, sched: &mut Scheduler<Event>) {
        let now = sched.now();
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
    }

    fn pm_due(&mut self, tool_id: ToolId, pm: usize, sched: &mut Scheduler<Event>) {
        let group = &self.data.tool_groups[self.tools[tool_id].group];
        if let PmTrigger::Calendar { interval, .. } = group.pms[pm].trigger {
            sched.schedule_in(interval, Event::PmDue { tool: tool_id, pm });
        }
        self.tools[tool_id].request_pm(pm);
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
        self.unhold(tool_id);
    }

    fn pm_done(&mut self, tool_id: ToolId, sched: &mut Scheduler<Event>) {
        let tool = &mut self.tools[tool_id];
        tool.account(sched.now());
        tool.pm = None;
        for breakdown in mem::take(&mut tool.deferred) {
            self.fail(tool_id, breakdown, sched);
        }
        self.tool_changed(tool_id, sched);
    }

    // ---- super hot lot reservations ----

    /// Reserves a tool of the next step's group for the lot (`rule_HotLotFIRST` groups only).
    fn reserve_next(&mut self, id: LotId, sched: &mut Scheduler<Event>) {
        let data = self.data;
        let lot = &self.lots[id];
        let step = lot.step + 1;
        let Some(next) = data.routes[lot.route].steps.get(step) else {
            return;
        };
        let group = next.tool_group;
        if data.tool_groups[group].rule != Rule::HotLotFirst
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
        if self.lots[lot].state == LotState::Queued && self.group_of(lot) == group {
            self.selected.clear();
            self.selected.push(lot);
            self.start_job(tool, sched);
        }
    }

    /// A held tool that goes down or into PM stops waiting; the reservation waits for another.
    fn unhold(&mut self, tool: ToolId) {
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

    // ---- CQT segment counts for stopping ----

    /// Adds (`delta` = 1) or removes (−1) a constrained lot's contribution to the stopping counts:
    /// in front of the group it is queued or processing at, upstream of every other group it still
    /// reaches in its segment (a moving lot also of the group it moves to); once per group.
    fn count_segment(&mut self, id: LotId, delta: i64) {
        let Some(stopping) = &mut self.strategy.stopping else {
            return;
        };
        let lot = &self.lots[id];
        let Some(segment) = &lot.segment else {
            return;
        };
        let steps = &self.data.routes[lot.route].steps;
        let at = (lot.state != LotState::Moving).then(|| steps[lot.step].tool_group);
        if let Some(group) = at {
            stopping.count(group, true, delta);
        }
        let ahead = &steps[lot.step..=segment.exit];
        for (index, step) in ahead.iter().enumerate() {
            let group = step.tool_group;
            if Some(group) != at
                && ahead[..index]
                    .iter()
                    .all(|earlier| earlier.tool_group != group)
            {
                stopping.count(group, false, delta);
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

    /// The last lot is complete: reports the drain and ends the run.
    fn finish(&mut self, sched: &mut Scheduler<Event>) {
        let now = sched.now();
        self.snapshot("Drain".into(), now, true, false);
        self.finished = Some(now);
        sched.stop();
    }

    /// Closes the statistics window at `now`: reports it (REPORT = yes) and restarts it
    /// (RESET = yes).
    fn snapshot(&mut self, name: String, now: Time, report: bool, reset: bool) {
        for tool in &mut self.tools {
            tool.account(now);
        }
        self.stats.wip(now, self.wip);
        if report {
            self.reports
                .push(self.stats.report(name, now, self.data, &self.tools));
        }
        if reset {
            self.stats.reset(now);
            for tool in &mut self.tools {
                tool.time = [0; STATES];
            }
        }
    }
}

impl Model for Fab<'_> {
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
    }

    fn handle(&mut self, event: Event, sched: &mut Scheduler<Event>) {
        match event {
            Event::Stream(index) => self.release_stream(index, sched),
            Event::Listed => self.release_listed(sched),
            Event::Arrive(lot) => self.arrive(lot, sched),
            Event::JobDone { tool, slot, epoch } => self.job_done(tool, slot, epoch, sched),
            Event::Fail { tool, breakdown } => self.fail(tool, breakdown, sched),
            Event::Repair { tool, breakdown } => self.repair(tool, breakdown, sched),
            Event::PmDue { tool, pm } => self.pm_due(tool, pm, sched),
            Event::PmDone(tool) => self.pm_done(tool, sched),
            Event::PeriodEnd(index) => self.period_end(index, sched),
            // A finished run stops before its deadline.
            Event::Deadline => sched.stop(),
        }
        // A stopping limit released by the event lets the groups holding lots dispatch again.
        if self
            .strategy
            .stopping
            .as_mut()
            .is_some_and(Stopping::released)
        {
            for group in mem::take(&mut self.stopped) {
                self.dispatch(group, None, sched);
            }
        }
    }
}
