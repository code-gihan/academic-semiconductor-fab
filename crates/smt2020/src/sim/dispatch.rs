//! Lot selection: which ready tool picks first, which lots it may take, their ranking and batch
//! formation.

use std::cmp::Ordering;
use std::mem;

use des_core::{Scheduler, Time};

use super::fab::{Event, Fab, Lot, LotId, LotState, Segment, ToolId};
use super::strategy::QueueTime;
use crate::data::{Dist, PartId, Rank, Rule, StepIndex, StepSetup, ToolGroupId};

/// Ranking key compared lexicographically, smallest first.
pub(super) type Key = [f64; 7];

fn compare(a: &Key, b: &Key) -> Ordering {
    a.iter()
        .zip(b)
        .map(|(x, y)| x.total_cmp(y))
        .find(|order| order.is_ne())
        .unwrap_or(Ordering::Equal)
}

impl Fab<'_> {
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
        let mut order = mem::take(&mut self.order);
        order.clear();
        order.extend(&self.groups[group].ready);
        if let Some(lot) = arrival
            && self.data.tool_groups[group].wake_least_setup
        {
            order.sort_by(|&a, &b| {
                self.wake_setup_time(lot, a)
                    .total_cmp(&self.wake_setup_time(lot, b))
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
                .filter(|&&id| self.lots[id].kind.engineering());
            if waiting.count() >= trigger as usize {
                self.groups[group].campaign = trigger;
            }
        }
        // rule_LSSU: an unfinished setup run, hot lots included, waits for lots keeping the setup
        // while any can come (AutoSched documentation: a minimum number of lots is ensured).
        let state = &self.tools[tool];
        let run_holds = matches!(self.data.tool_groups[group].rule, Rule::SetupRun(_))
            && state.run_left > 0
            && state.setup.is_some_and(|setup| {
                self.can_still_come(&self.routes.setup_members[&(group, setup)])
            });
        let mut candidates = mem::take(&mut self.candidates);
        candidates.clear();
        let mut stopped = false;
        for &id in &self.groups[group].queue {
            if self.stopping_holds(id) {
                stopped = true;
            } else if self.fits(id, tool) && !(run_holds && self.needed_setup(id, tool).is_some()) {
                candidates.push((self.key(id, tool, now), id));
            }
        }
        if stopped && !self.stopped.contains(&group) {
            self.stopped.push(group);
        }
        let found = if self.data.tool_groups[group].batching.is_some() {
            self.form_batch(&mut candidates)
        } else if let Some(&(_, id)) = candidates.iter().min_by(|a, b| compare(&a.0, &b.0)) {
            self.selected.clear();
            self.selected.push(id);
            true
        } else {
            false
        };
        self.candidates = candidates;
        found
    }

    /// Stopping ([P2] §3.2): a lot about to enter a CQT segment waits while a limit of a tool group
    /// of the segment is reached, unless it is leaving the previous segment at this step.
    fn stopping_holds(&self, id: LotId) -> bool {
        let Some(stopping) = &self.strategy.stopping else {
            return false;
        };
        let lot = &self.lots[id];
        let steps = &self.data.routes[lot.route].steps;
        let Some(cqt) = steps[lot.step].cqt else {
            return false;
        };
        if lot
            .segment
            .as_ref()
            .is_some_and(|segment| segment.exit == lot.step)
        {
            return false;
        }
        steps[lot.step + 1..=cqt.until]
            .iter()
            .any(|step| stopping.reached(step.tool_group))
    }

    /// Lot-to-lens dedication: a lot dedicated to another tool at this step does not fit.
    fn fits(&self, id: LotId, tool: ToolId) -> bool {
        let lot = &self.lots[id];
        !lot.dedicated
            .iter()
            .any(|&(step, dedicated)| step == lot.step && dedicated != tool)
    }

    /// Setup a lot needs on a tool, if any.
    fn needed_setup(&self, id: LotId, tool: ToolId) -> Option<StepSetup> {
        let lot = &self.lots[id];
        self.data.routes[lot.route].steps[lot.step]
            .setup
            .filter(|setup| setup.always || self.tools[tool].setup != Some(setup.setup))
    }

    /// `rank_RSETUP`: mean setup time from the setup change table. Setup times given only in the
    /// route rank as none, as in the AutoSched reference runs.
    fn ranked_setup_time(&self, id: LotId, tool: ToolId) -> f64 {
        self.needed_setup(id, tool)
            .and_then(|setup| self.setup_dist(self.tools[tool].setup, setup.setup))
            .map_or(0.0, |dist| dist.mean() as f64)
    }

    /// `wake_LeastSetupTime`: mean time of the setup the lot needs on the tool.
    fn wake_setup_time(&self, id: LotId, tool: ToolId) -> f64 {
        self.needed_setup(id, tool).map_or(0.0, |setup| {
            setup
                .time
                .or_else(|| {
                    self.setup_dist(self.tools[tool].setup, setup.setup)
                        .map(Dist::mean)
                })
                .unwrap_or(0) as f64
        })
    }

    /// Lexicographic key: CAtE/CoT class preference, then the group's ranks with the queue-time
    /// urgency right before FIFO/CR, then release order.
    fn key(&self, id: LotId, tool: ToolId, now: Time) -> Key {
        let lot = &self.lots[id];
        let group = self.tools[tool].group;
        let spec = &self.data.tool_groups[group];
        let mut key = [0.0; 7];
        let mut len = 0;
        let mut push = |value: f64| {
            key[len] = value;
            len += 1;
        };
        if let Some(class) =
            self.strategy
                .class_rank(group, lot.kind, now, self.groups[group].campaign)
        {
            push(class);
        }
        for &rank in &spec.ranks {
            match rank {
                Rank::Priority => push(-f64::from(lot.priority)),
                Rank::LeastSetup => push(self.ranked_setup_time(id, tool)),
                Rank::Fifo | Rank::CriticalRatio => {
                    if let Some(urgency) = self.queue_time_urgency(lot, now) {
                        push(urgency);
                    }
                    push(match rank {
                        Rank::Fifo => lot.queued_at as f64,
                        _ => self.critical_ratio(lot, now),
                    });
                }
            }
        }
        push(lot.serial as f64);
        key
    }

    /// Time to the due date over the expected remaining work.
    fn critical_ratio(&self, lot: &Lot, now: Time) -> f64 {
        (lot.due - now) as f64 / self.routes.info[lot.route].remaining[lot.step].at(lot.wafers)
    }

    /// QTCR index or QTS start deadline; `None` without a queue-time rule, +∞ outside segments.
    fn queue_time_urgency(&self, lot: &Lot, now: Time) -> Option<f64> {
        match &self.strategy.queue_time {
            QueueTime::None => None,
            QueueTime::Qtcr => Some(
                lot.segment
                    .as_ref()
                    .map_or(f64::INFINITY, |segment| self.qtcr(lot, segment, now)),
            ),
            QueueTime::Qts(flow_factors) => {
                Some(lot.segment.as_ref().map_or(f64::INFINITY, |segment| {
                    self.qts(lot, segment, flow_factors)
                }))
            }
        }
    }

    /// [P2] eq. (1) with p_k the expected step durations from the current step to the exit.
    fn qtcr(&self, lot: &Lot, segment: &Segment, now: Time) -> f64 {
        let remaining = &self.routes.info[lot.route].remaining;
        let work = remaining[lot.step].at(lot.wafers) - remaining[segment.exit + 1].at(lot.wafers);
        let slack = (segment.entered + segment.limit - now) as f64;
        if slack >= 0.0 {
            slack / work
        } else {
            slack * work
        }
    }

    /// [P2] eq. (2)–(6): latest start of the current step; flow factors that were never measured
    /// count as 1.
    fn qts(&self, lot: &Lot, segment: &Segment, flow_factors: &[Vec<f64>]) -> f64 {
        let deadline = (segment.entered + segment.limit) as f64;
        if lot.step >= segment.exit {
            return deadline;
        }
        let info = &self.routes.info[lot.route];
        let steps = &self.data.routes[lot.route].steps;
        let p = |k: usize| steps[k].sampling * info.step[k].at(lot.wafers);
        let ff = |k: usize| {
            Some(flow_factors[lot.route][k])
                .filter(|ff| ff.is_finite())
                .unwrap_or(1.0)
        };
        let span = (segment.entry + 1..segment.exit)
            .map(|k| ff(k) * p(k))
            .sum::<f64>()
            + (ff(segment.exit) - 1.0) * p(segment.exit);
        let allocated: f64 = (segment.entry + 1..=lot.step).map(|k| ff(k) * p(k)).sum();
        let share = if span > 0.0 { allocated / span } else { 0.0 };
        segment.entered as f64 + segment.limit as f64 * share - p(lot.step)
    }

    /// [P2] §4.1: the best-ranked lot opens a batch filled with compatible lots in rank order up to
    /// the maximum. It starts at the minimum, or below it once no compatible lot can still come;
    /// otherwise the next-ranked lot of another batch kind opens one.
    fn form_batch(&mut self, candidates: &mut [(Key, LotId)]) -> bool {
        candidates.sort_by(|a, b| compare(&a.0, &b.0));
        let mut tried = Vec::new();
        for first in 0..candidates.len() {
            let head = &self.lots[candidates[first].1];
            let key = self.routes.batch_key[head.part][head.step].expect("batch step");
            if tried.contains(&key) {
                continue;
            }
            tried.push(key);
            let size = self.data.routes[head.route].steps[head.step]
                .batch
                .expect("batch size");
            self.selected.clear();
            let mut wafers = 0;
            for &(_, id) in &candidates[first..] {
                let lot = &self.lots[id];
                if self.routes.batch_key[lot.part][lot.step] == Some(key)
                    && wafers + lot.wafers <= size.max
                {
                    self.selected.push(id);
                    wafers += lot.wafers;
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
