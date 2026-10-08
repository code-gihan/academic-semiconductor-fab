//! Lot selection: which ready tool picks first, which lots it may take, their ranking and batch
//! formation. Dispatching runs on events only (arrival, job end, repair, PM end, reservation
//! release, released holds, wake) and reads the queue entries fixed on arrival.

use std::cmp::Ordering;
use std::mem;

use des_core::{Scheduler, Time};

use super::Criterion;
use super::code::{Admit, BatchView, GroupCount, SegmentView, Start};
use super::fab::{Event, Fab, LotId, LotState, ToolId, Waiting};
use super::strategy::MAX_CRITERIA;
use crate::data::{Dist, PartId, RouteId, Rule, SetupId, StepIndex, StepSetup, ToolGroupId};

/// Ranking key compared lexicographically, smallest first: the CAtE/CoT class, the criteria and
/// the release order.
pub(super) type Key = [f64; MAX_CRITERIA + 2];

/// What the keys of one selection share: the group's criteria, the tool's setup and the instant.
struct Ranking<'a> {
    criteria: &'a [Criterion],
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
        if self.mark_held(group, sched.now()) && !self.stopped.contains(&group) {
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
                self.wake_setup_time(setup, self.tools[a].next_setup)
                    .total_cmp(&self.wake_setup_time(setup, self.tools[b].next_setup))
            });
        }
        if self.logistics.is_some() {
            self.assign_lots(group, &order, sched);
        } else {
            for &tool in &order {
                while self.tools[tool].ready && self.select(tool, sched.now()) {
                    self.start_job(tool, sched);
                }
                if self.groups[group].queue.is_empty() {
                    break;
                }
            }
        }
        self.order = order;
        // A held batch or lot may go later by queue-time slack or as the code asked: dispatch
        // again then.
        let state = &mut self.groups[group];
        if let Some(due) = state.due.take()
            && state.wake.is_none_or(|pending| due < pending)
        {
            state.wake = Some(due);
            sched.schedule_at(due, Event::Wake(group));
        }
    }

    /// AMHS: one lot or batch at a time to the ready tool with the least work (jobs and
    /// assignments), then free first, then in `order`; a tool that finds nothing to take sits out
    /// the rest of the dispatch (what it may take only shrinks).
    fn assign_lots(&mut self, group: ToolGroupId, order: &[ToolId], sched: &mut Scheduler<Event>) {
        let mut idle = Vec::new();
        while !self.groups[group].queue.is_empty() {
            let Some(tool) = order
                .iter()
                .copied()
                .filter(|&tool| self.tools[tool].ready && !idle.contains(&tool))
                .min_by_key(|&tool| (self.load(tool), self.free_at(tool)))
            else {
                return;
            };
            if self.select(tool, sched.now()) {
                self.assign(tool, sched);
            } else {
                idle.push(tool);
            }
        }
    }

    /// The index of the best-ranked of `entries` (index, queue entry) for `tool` at `now`, under
    /// its group's ranking with its present setup; the first among equals.
    pub(super) fn best_ranked<'a>(
        &self,
        tool: ToolId,
        entries: impl Iterator<Item = (usize, &'a Waiting)>,
        now: Time,
    ) -> Option<usize> {
        let group = self.tools[tool].group;
        let ranking = Ranking {
            criteria: &self.strategy.ranking[group],
            prefer_engineering: self.strategy.prefer_engineering(
                group,
                now,
                self.groups[group].campaign,
            ),
            setup: self.tools[tool].setup,
            now,
        };
        let mut best: Option<(Key, usize)> = None;
        for (index, waiting) in entries {
            let key = self.key(waiting, &ranking);
            if best
                .as_ref()
                .is_none_or(|(best, _)| compare(&key, best).is_lt())
            {
                best = Some((key, index));
            }
        }
        best.map(|(_, index)| index)
    }

    /// Stopping ([P2] §3.2) and admission code: marks the lots about to enter a CQT segment while
    /// a limit of a tool group of the segment is reached or the code holds them; true if any is
    /// held. The marks hold for the whole dispatch, as job starts leave the counts unchanged.
    fn mark_held(&mut self, group: ToolGroupId, now: Time) -> bool {
        if self.counts.is_none() {
            return false;
        }
        let mut held_on = mem::take(&mut self.groups[group].held_on);
        held_on.clear();
        let mut any = false;
        for index in 0..self.groups[group].queue.len() {
            let held = self.groups[group].queue[index].entering
                && (self.stopping_holds(group, index)
                    || (self.hooks.admit && self.code_holds(group, index, now, &mut held_on)));
            self.groups[group].queue[index].held = held;
            any |= held;
        }
        self.groups[group].held_on = held_on;
        any
    }

    /// Stopping: whether a limit of a tool group of the segment that the lot queued at `index`
    /// of `group` is about to enter is reached.
    fn stopping_holds(&self, group: ToolGroupId, index: usize) -> bool {
        let (Some(stopping), Some(counts)) = (&self.strategy.stopping, &self.counts) else {
            return false;
        };
        let waiting = &self.groups[group].queue[index];
        self.routes.info[waiting.route].segment_groups[waiting.step]
            .iter()
            .any(|&later| stopping.reached(counts, later))
    }

    /// Admission code: whether it holds the lot queued at `index` of `group`, which is about to
    /// enter a CQT segment; the tool groups of its segment join `held_on` if so.
    fn code_holds(
        &mut self,
        group: ToolGroupId,
        index: usize,
        now: Time,
        held_on: &mut Vec<ToolGroupId>,
    ) -> bool {
        if self.code_failure().is_some() {
            return false;
        }
        let waiting = &self.groups[group].queue[index];
        let lot = self.lot_view(waiting, now);
        let segment = self.routes.segment_at[waiting.route][waiting.step].expect("CQT entrance");
        let spec = &self.data.routes[waiting.route].steps[waiting.step];
        let limit = spec.cqt.expect("CQT entrance").limit;
        let groups = &self.routes.info[waiting.route].segment_groups[waiting.step];
        let counts = self.counts.as_ref().expect("counts with admission code");
        self.group_counts.clear();
        self.group_counts
            .extend(groups.iter().map(|&later| GroupCount {
                tool_group: later,
                front: counts.front[later] as u32,
                total: (counts.front[later] + counts.upstream[later]) as u32,
            }));
        let view = SegmentView {
            segment,
            limit,
            exit: self.routes.segments[segment].exit,
            groups: &self.group_counts,
        };
        let code = self.code.as_deref_mut().expect("code with admit");
        let until = match code.admit(&lot, &view, now) {
            Ok(Admit::Now) => return false,
            Ok(Admit::Hold) => None,
            Ok(Admit::HoldUntil(at)) => Some(at),
            Err(message) => {
                Self::record_failure(&mut self.code_failure, format!("admit: {message}"));
                return false;
            }
        };
        if let Some(at) = until.filter(|&at| at > now) {
            let due = &mut self.groups[group].due;
            *due = Some(due.map_or(at, |due| due.min(at)));
        }
        for &later in groups {
            if !held_on.contains(&later) {
                held_on.push(later);
            }
        }
        true
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
        // while any can come (AutoSched documentation: a minimum number of lots is ensured),
        // unless the group's runs would wait on each other for good. The setup and run are those
        // after the lots assigned to the tool ahead.
        let state = &self.tools[tool];
        let current = state.next_setup;
        let run_holds = matches!(self.data.tool_groups[group].rule, Rule::SetupRun(_))
            && state.next_run_left > 0
            && current.is_some_and(|setup| {
                self.can_still_come(&self.routes.setup_members[&(group, setup)])
            })
            && !self.runs_locked(group);
        // AMHS: a tool without a free port takes only the lots standing at its ports.
        let dockable = self.dockable(tool);
        // Held lots, lots dedicated to another tool, and setup changes during a held run wait.
        let eligible = |waiting: &Waiting| {
            !waiting.held
                && waiting.dedicated.is_none_or(|dedicated| dedicated == tool)
                && dockable
                    .as_ref()
                    .is_none_or(|lots| lots.contains(&waiting.lot))
                && !(run_holds
                    && waiting
                        .setup
                        .is_some_and(|setup| current != Some(setup.setup)))
        };
        let ranking = Ranking {
            criteria: &self.strategy.ranking[group],
            prefer_engineering: self.strategy.prefer_engineering(
                group,
                now,
                self.groups[group].campaign,
            ),
            setup: current,
            now,
        };
        if self.data.tool_groups[group].batching.is_some() {
            let mut candidates = mem::take(&mut self.candidates);
            candidates.clear();
            for (index, waiting) in self.groups[group].queue.iter().enumerate() {
                if eligible(waiting) {
                    candidates.push((self.key(waiting, &ranking), index));
                }
            }
            let found = self.form_batch(group, &mut candidates, now);
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

    /// Lexicographic key: CAtE/CoT class preference, the group's criteria, release order. The
    /// queue-time criteria rank lots outside CQT segments last. Inlined into the selection loops,
    /// which call it for every queued lot: as a call it took a third of the run time (DS2, DS4).
    #[inline(always)]
    fn key(&self, waiting: &Waiting, ranking: &Ranking) -> Key {
        let now = ranking.now;
        let mut key = [0.0; MAX_CRITERIA + 2];
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
        let cqt = waiting.cqt.as_ref();
        for &criterion in ranking.criteria {
            push(match criterion {
                Criterion::Priority => -f64::from(waiting.priority),
                Criterion::LeastSetup => self.ranked_setup_time(waiting.setup, ranking.setup),
                Criterion::Fifo => waiting.queued_at as f64,
                // Time to the due date over the expected remaining work.
                Criterion::CriticalRatio => (waiting.due - now) as f64 / waiting.remaining,
                Criterion::DueDate => waiting.due as f64,
                Criterion::ShortestStep => waiting.step_time,
                Criterion::LeastRemaining => waiting.remaining,
                Criterion::Qtcr => {
                    cqt.map_or(f64::INFINITY, |cqt| qtcr(cqt.deadline, cqt.work, now))
                }
                Criterion::Qts => cqt.map_or(f64::INFINITY, |cqt| cqt.latest),
                Criterion::QtDeadline => cqt.map_or(f64::INFINITY, |cqt| cqt.deadline as f64),
                Criterion::QtWithin(within) => {
                    if cqt.is_some_and(|cqt| cqt.slack(now) <= within as f64) {
                        0.0
                    } else {
                        1.0
                    }
                }
                Criterion::Code => waiting.code,
            });
        }
        push(waiting.serial as f64);
        key
    }

    /// [P2] §4.1: the best-ranked lot opens a batch filled with compatible lots in rank order up to
    /// the maximum. It starts at the minimum, or below it once no compatible lot can still come,
    /// one of its lots has used up its queue-time slack down to `batch_start_within` or the
    /// strategy code starts it; otherwise the next-ranked lot of another batch kind opens one.
    fn form_batch(
        &mut self,
        group: ToolGroupId,
        candidates: &mut [(Key, usize)],
        now: Time,
    ) -> bool {
        candidates.sort_by(|a, b| compare(&a.0, &b.0));
        let queue = &self.groups[group].queue;
        let within = self.strategy.batch_start_within;
        let ask = self.hooks.start_batch && self.code_failure().is_none();
        let mut tried = Vec::new();
        let mut started = false;
        // When a batch held here may start by queue-time slack alone.
        let mut due = Time::MAX;
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
            // When the slack of one of the batch's lots first reaches the threshold.
            let mut urgent_from = Time::MAX;
            // For the code: the earliest arrival and the least slack of the batch's lots.
            let mut oldest = Time::MAX;
            let mut slack: Option<f64> = None;
            for &(_, index) in &candidates[first..] {
                let waiting = &queue[index];
                if waiting.batch == Some(key) && wafers + waiting.wafers <= size.max {
                    self.selected.push(waiting.lot);
                    wafers += waiting.wafers;
                    if let (Some(within), Some(cqt)) = (within, &waiting.cqt) {
                        urgent_from = urgent_from.min(cqt.within_from(within));
                    }
                    if ask {
                        oldest = oldest.min(waiting.queued_at);
                        if let Some(cqt) = &waiting.cqt {
                            let left = cqt.slack(now);
                            slack = Some(slack.map_or(left, |least| least.min(left)));
                        }
                    }
                }
            }
            if wafers >= size.min
                || urgent_from <= now
                || !self.can_still_come(&self.routes.batch_members[key])
            {
                started = true;
                break;
            }
            due = due.min(urgent_from);
            if ask && self.code_failure().is_none() {
                let batch = BatchView {
                    tool_group: group,
                    route: head.route,
                    step: head.step,
                    lots: self.selected.len() as u32,
                    wafers,
                    min: size.min,
                    max: size.max,
                    oldest,
                    slack,
                };
                let code = self.code.as_deref_mut().expect("code with start_batch");
                match code.start_batch(&batch, now) {
                    Ok(Start::Now) => {
                        started = true;
                        break;
                    }
                    Ok(Start::WaitUntil(at)) if at > now => due = due.min(at),
                    Ok(Start::Wait | Start::WaitUntil(_)) => {}
                    Err(message) => {
                        Self::record_failure(
                            &mut self.code_failure,
                            format!("start_batch: {message}"),
                        );
                    }
                }
            }
        }
        if due < Time::MAX {
            let held = &mut self.groups[group].due;
            *held = Some(held.map_or(due, |held| held.min(due)));
        }
        started
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

    /// rule_LSSU: whether the group's setup runs wait on each other for good. Every tool of it is
    /// idle in an unfinished run that lots of its setup can still come to (7.8), yet no lot comes
    /// to a step of the group that a run lets its tool take (no setup, or a running one) before a
    /// step needing another setup, which the runs keep every tool from taking up. Runs that cannot
    /// end so give way.
    fn runs_locked(&self, group: ToolGroupId) -> bool {
        let mut running = Vec::new();
        for tool in self.groups[group].tools.clone() {
            let state = &self.tools[tool];
            let idle = if self.logistics.is_some() {
                self.load(tool) == 0
            } else {
                state.busy() == 0
            };
            match state.next_setup {
                Some(setup) if idle && state.next_run_left > 0 => {
                    if !running.contains(&setup) {
                        running.push(setup);
                    }
                }
                _ => return false,
            }
        }
        // Whether a lot at `step` of `route` meets the group first at a step a run takes. Setup
        // steps of rule_LSSU groups are never skipped (checked by the loader).
        let comes = |route: RouteId, step: StepIndex| {
            self.data.routes[route].steps[step..]
                .iter()
                .find(|next| next.tool_group == group)
                .is_some_and(|next| next.setup.is_none_or(|need| running.contains(&need.setup)))
        };
        running
            .iter()
            .all(|&setup| self.can_still_come(&self.routes.setup_members[&(group, setup)]))
            && !self
                .lots
                .iter()
                .any(|lot| lot.alive && comes(lot.route, lot.step))
            && !self
                .data
                .parts
                .iter()
                .enumerate()
                .any(|(part, spec)| self.plan.remaining[part] > 0 && comes(spec.route, 0))
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use des_core::{DAY, HOUR};

    use super::*;
    use crate::data::tiny;
    use crate::sim::fab::WaitingCqt;
    use crate::sim::{Config, LotKind, Recording};

    /// Lot `serial` queued at 0 with priority 10, due in 9 days, 10 h of remaining work and a 1 h
    /// step, outside CQT segments.
    fn waiting(serial: u64) -> Waiting {
        Waiting {
            lot: serial as usize,
            route: 0,
            step: 0,
            kind: LotKind::Prl,
            priority: 10,
            wafers: 25,
            serial,
            queued_at: 0,
            due: 9 * DAY,
            remaining: (10 * HOUR) as f64,
            step_time: HOUR as f64,
            setup: None,
            dedicated: None,
            batch: None,
            cqt: None,
            code: 0.0,
            entering: false,
            held: false,
        }
    }

    /// In a CQT segment ending at `deadline`, with `work` hours to the exit step's end and
    /// `before_exit` hours to its start.
    fn segment(deadline: Time, work: f64, before_exit: f64) -> Option<WaitingCqt> {
        Some(WaitingCqt {
            deadline,
            work: work * HOUR as f64,
            before_exit: before_exit * HOUR as f64,
            latest: (deadline - 2 * HOUR) as f64,
        })
    }

    /// Serials of `queue` in ranking order under `criteria` at `now`, on a tool without setup.
    fn order(criteria: &[Criterion], queue: &[Waiting], now: Time) -> Vec<u64> {
        let fab = Fab::new(
            Arc::new(tiny()),
            &Config::new(DAY),
            None,
            &Recording::default(),
            &mut None,
        )
        .unwrap();
        let ranking = Ranking {
            criteria,
            prefer_engineering: None,
            setup: None,
            now,
        };
        let mut keys: Vec<_> = queue
            .iter()
            .map(|waiting| (fab.key(waiting, &ranking), waiting.serial))
            .collect();
        keys.sort_by(|a, b| compare(&a.0, &b.0));
        keys.into_iter().map(|(_, serial)| serial).collect()
    }

    #[test]
    fn criteria_rank_the_smallest_value_first() {
        use Criterion as C;
        let now = 3 * HOUR;
        let queue = [
            Waiting {
                priority: 20,
                queued_at: 2 * HOUR,
                due: 3 * DAY,
                remaining: (20 * HOUR) as f64,
                cqt: segment(5 * HOUR, 4.0, 0.0),
                ..waiting(0)
            },
            Waiting {
                queued_at: HOUR,
                due: 4 * DAY,
                step_time: (2 * HOUR) as f64,
                setup: Some(StepSetup {
                    setup: 0,
                    always: false,
                    time: None,
                }),
                cqt: segment(4 * HOUR, 1.0, 2.5),
                ..waiting(1)
            },
            Waiting {
                queued_at: 3 * HOUR,
                remaining: (5 * HOUR) as f64,
                step_time: (HOUR / 2) as f64,
                ..waiting(2)
            },
        ];
        // Ties go by release order.
        assert_eq!(order(&[C::Priority], &queue, now), [0, 1, 2]);
        assert_eq!(order(&[C::Fifo], &queue, now), [1, 0, 2]);
        assert_eq!(order(&[C::LeastSetup], &queue, now), [0, 2, 1]);
        // Due date over remaining work: 3 d / 20 h, 4 d / 10 h, 9 d / 5 h.
        assert_eq!(order(&[C::CriticalRatio], &queue, now), [0, 1, 2]);
        assert_eq!(order(&[C::DueDate], &queue, now), [0, 1, 2]);
        assert_eq!(order(&[C::ShortestStep], &queue, now), [2, 0, 1]);
        assert_eq!(order(&[C::LeastRemaining], &queue, now), [2, 1, 0]);
        // Lots outside CQT segments go last. QTCR: 2 h / 4 h before 1 h / 1 h.
        assert_eq!(order(&[C::Qtcr], &queue, now), [0, 1, 2]);
        assert_eq!(order(&[C::Qts, C::Fifo], &queue, now), [1, 0, 2]);
        assert_eq!(order(&[C::QtDeadline], &queue, now), [1, 0, 2]);
        // Slack: 5 h − 3 h = 2 h; 4 h − 3 h − 2.5 h = −1.5 h.
        assert_eq!(
            order(&[C::QtWithin(HOUR), C::Priority], &queue, now),
            [1, 0, 2]
        );
        assert_eq!(
            order(&[C::QtWithin(2 * HOUR), C::Fifo], &queue, now),
            [1, 0, 2]
        );
        assert_eq!(
            order(&[C::QtWithin(2 * HOUR), C::Priority], &queue, now),
            [0, 1, 2]
        );
    }

    #[test]
    fn qtcr_multiplies_the_slack_once_past_the_end() {
        assert_eq!(qtcr(5, 2.0, 1), 2.0);
        assert_eq!(qtcr(1, 2.0, 5), -8.0);
    }
}
