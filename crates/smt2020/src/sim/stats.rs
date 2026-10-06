//! Statistics accumulated since the last reset and the reports taken at period ends, and the
//! days of a run.

use des_core::{DAY, HOUR, Time};
use serde::{Deserialize, Serialize};

use super::routes::Routes;
use super::tool::{STATES, Tool, ToolState};
use crate::data::{Dataset, PartId, RouteId, StepIndex};

named_enum! {
    /// Lot categories of the papers.
    pub enum LotKind {
        /// Production regular lot (priority 10).
        Prl = "PRL",
        /// Production hot lot (priority 20).
        Phl = "PHL",
        /// Super hot lot (priority 30).
        Shl = "SHL",
        /// Engineering regular lot (priority 10).
        Erl = "ERL",
        /// Engineering hot lot (priority 20).
        Ehl = "EHL",
    }
}

/// Lot kinds in declaration order, which indexes the per-kind statistics.
const KINDS: &[LotKind] = LotKind::ALL;

impl LotKind {
    /// Kind of a lot of an engineering or production part with this priority.
    pub fn of(engineering: bool, priority: u32) -> Option<Self> {
        match (engineering, priority) {
            (false, 10) => Some(Self::Prl),
            (false, 20) => Some(Self::Phl),
            (false, 30) => Some(Self::Shl),
            (true, 10) => Some(Self::Erl),
            (true, 20) => Some(Self::Ehl),
            _ => None,
        }
    }

    pub fn engineering(self) -> bool {
        matches!(self, Self::Erl | Self::Ehl)
    }

    pub fn hot(self) -> bool {
        matches!(self, Self::Phl | Self::Shl | Self::Ehl)
    }
}

/// Outcome of one run. Times are ms since the simulation start.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Results {
    /// The run's random numbers ([`Config::seed`](super::Config::seed) and replication):
    /// results of the same pair share them, so strategies compare pairwise.
    pub seed: u64,
    pub replication: u32,
    /// Reporting periods (REPORT = yes) up to the horizon, then `Drain` covering the completion
    /// of the lots still in the fab at the horizon.
    pub periods: Vec<PeriodReport>,
    /// Day k covers [k·DAY, (k + 1)·DAY), the last one up to the end.
    pub days: Vec<DayReport>,
    /// Lots released (plan and initial WIP) and completed; equal for a finished run.
    pub released: u64,
    pub completed: u64,
    /// Completion of the last lot, which ends the run.
    pub end: Time,
    /// Model events handled.
    pub events: u64,
    /// Per route and step: mean step cycle time (since the previous processed step) over the
    /// expected step duration, in the window ending at the horizon; `None` if never processed.
    /// The QTS rule takes them as flow factors.
    pub step_flow_factors: Vec<Vec<Option<f64>>>,
}

impl Results {
    /// FNV-1a 64-bit hash of the results' postcard encoding, in hex: equal digests mean
    /// bit-identical results, e.g. of a native and a wasm run of one configuration.
    pub fn digest(&self) -> String {
        let bytes = postcard::to_allocvec(self).expect("results encode to memory");
        let hash = bytes.iter().fold(0xcbf2_9ce4_8422_2325_u64, |hash, &byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
        });
        format!("{hash:016x}")
    }
}

/// Statistics of one window, from the last reset to the period end.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PeriodReport {
    pub name: String,
    pub start: Time,
    pub end: Time,
    /// Per part and lot kind with releases or completions in the window.
    pub lots: Vec<LotReport>,
    /// Lot flow factor percentiles per lot kind over all parts.
    pub flow_factors: Vec<FlowFactors>,
    /// Time-averaged lots in the fab.
    pub wip: f64,
    pub tool_groups: Vec<ToolGroupReport>,
    /// CQT segments with stepper (LithoTrack_FE_95/115) steps, and all others.
    pub cqt_litho: CqtReport,
    pub cqt_rest: CqtReport,
    /// Every CQT segment of the dataset, in [`Dataset::segments`] order.
    pub cqt_segments: Vec<CqtSegmentReport>,
}

/// A CQT segment in a window: its completions and where their waits went.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CqtSegmentReport {
    pub route: String,
    /// Entrance and exit step: the wait runs from the end of the one to the start of the other.
    pub entry: usize,
    pub exit: usize,
    /// Includes stepper steps.
    pub litho: bool,
    pub cqt: CqtReport,
    /// The steps after the entrance through the exit, in route order.
    pub steps: Vec<CqtStepReport>,
}

/// A step of a CQT segment: the time its completions within and over the limit spent there.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CqtStepReport {
    pub step: usize,
    pub met: CqtTimes,
    pub violated: CqtTimes,
}

/// Time spent at a step of a CQT segment, summed over its visits (steps skipped by sampling
/// have none); the parts of all steps add up to the waits.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CqtTimes {
    pub visits: u64,
    /// From the end of the previous processed step to the arrival in the queue.
    pub transport: Time,
    /// From the arrival to the start of the job.
    pub queue: Time,
    /// From the start of the job to the end of the step (setup, load, processing, unload and
    /// outages meanwhile); none at the exit step, whose start ends the wait.
    pub process: Time,
}

/// A day of a run.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DayReport {
    /// Lots released and completed.
    pub started: u64,
    pub completed: u64,
    /// Time-averaged lots in the fab; 0 for a day of no length (a run ending at midnight).
    pub wip: f64,
    /// CQT segments whose exit step started.
    pub cqt: CqtReport,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LotReport {
    pub part: String,
    pub kind: LotKind,
    pub started: u64,
    pub completed: u64,
    /// Completed by the due date.
    pub on_time: u64,
    /// Mean and population standard deviation of the cycle time in ms, and the mean flow factor
    /// (cycle time over raw processing time); `None` without completions.
    pub cycle_time_mean: Option<f64>,
    pub cycle_time_std: Option<f64>,
    pub flow_factor_mean: Option<f64>,
}

/// Flow factor percentiles of the lots of one kind.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FlowFactors {
    pub kind: LotKind,
    /// At [`FLOW_FACTOR_PERCENTILES`].
    pub percentiles: [f64; 7],
}

/// Percent levels of [`FlowFactors::percentiles`].
pub const FLOW_FACTOR_PERCENTILES: [f64; 7] = [0.0, 5.0, 25.0, 50.0, 75.0, 95.0, 100.0];

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolGroupReport {
    pub name: String,
    pub area: String,
    pub tools: u32,
    /// Tool time per state, summed over the group's tools.
    pub time: StateTimes,
}

/// Time in ms per tool state. Where jobs of a cascading tool overlap, one state counts in the
/// order down, PM, setup, process, load, unload.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StateTimes {
    pub down: Time,
    pub pm: Time,
    pub setup: Time,
    pub process: Time,
    pub load: Time,
    pub unload: Time,
    pub idle: Time,
}

/// CQT waits measured at the start of the exit step.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CqtReport {
    /// Segments completed (exit step started).
    pub completed: u64,
    pub violated: u64,
    /// Violations longer than 1, 2 and 4 hours.
    pub violated_1h: u64,
    pub violated_2h: u64,
    pub violated_4h: u64,
    /// Total excess over the limits and total slack under them, in ms.
    pub violation: Time,
    pub slack: Time,
}

impl CqtReport {
    /// Counts a completion that waited `wait` under `limit`; true if over it.
    fn add(&mut self, wait: Time, limit: Time) -> bool {
        self.completed += 1;
        if wait > limit {
            let excess = wait - limit;
            self.violated += 1;
            self.violation += excess;
            self.violated_1h += u64::from(excess > HOUR);
            self.violated_2h += u64::from(excess > 2 * HOUR);
            self.violated_4h += u64::from(excess > 4 * HOUR);
            true
        } else {
            self.slack += limit - wait;
            false
        }
    }

    /// The completions of both.
    pub fn merge(&self, other: &Self) -> Self {
        Self {
            completed: self.completed + other.completed,
            violated: self.violated + other.violated,
            violated_1h: self.violated_1h + other.violated_1h,
            violated_2h: self.violated_2h + other.violated_2h,
            violated_4h: self.violated_4h + other.violated_4h,
            violation: self.violation + other.violation,
            slack: self.slack + other.slack,
        }
    }

    /// The completions since `earlier`, a count of the same completions before.
    fn since(&self, earlier: &Self) -> Self {
        Self {
            completed: self.completed - earlier.completed,
            violated: self.violated - earlier.violated,
            violated_1h: self.violated_1h - earlier.violated_1h,
            violated_2h: self.violated_2h - earlier.violated_2h,
            violated_4h: self.violated_4h - earlier.violated_4h,
            violation: self.violation - earlier.violation,
            slack: self.slack - earlier.slack,
        }
    }
}

/// A visit of a lot in a CQT segment to one of its steps (`CqtTimes`).
#[derive(Clone, Copy)]
pub(super) struct Visit {
    pub step: StepIndex,
    pub transport: Time,
    pub queue: Time,
    pub process: Time,
}

#[derive(Default)]
struct LotStats {
    started: u64,
    on_time: u64,
    cycle_times: Vec<Time>,
    flow_factors: Vec<f64>,
}

/// A CQT segment's completions in the window and, per step after the entrance, their times
/// within the limit (`[0]`) and over it (`[1]`).
struct SegmentStats {
    cqt: CqtReport,
    steps: Vec<[CqtTimes; 2]>,
}

pub(super) struct Stats {
    since: Time,
    wip_since: Time,
    wip_area: f64,
    /// Per part and kind.
    lots: Vec<LotStats>,
    /// Per CQT segment of the routes.
    segments: Vec<SegmentStats>,
    /// Every CQT completion of the run, never reset (days, progress).
    pub cqt_total: CqtReport,
    /// Every completion of each CQT segment of the run, never reset (segment statuses).
    pub segment_totals: Vec<CqtReport>,
    /// Per route and step: sum of step cycle time over expected duration, and visits.
    steps: Vec<Vec<(f64, u64)>>,
}

impl Stats {
    pub(super) fn new(data: &Dataset, routes: &Routes) -> Self {
        Self {
            since: 0,
            wip_since: 0,
            wip_area: 0.0,
            lots: (0..data.parts.len() * KINDS.len())
                .map(|_| LotStats::default())
                .collect(),
            segments: routes
                .segments
                .iter()
                .map(|segment| SegmentStats {
                    cqt: CqtReport::default(),
                    steps: vec![[CqtTimes::default(); 2]; segment.exit - segment.entry],
                })
                .collect(),
            cqt_total: CqtReport::default(),
            segment_totals: vec![CqtReport::default(); routes.segments.len()],
            steps: data
                .routes
                .iter()
                .map(|route| vec![(0.0, 0); route.steps.len()])
                .collect(),
        }
    }

    fn lot(&mut self, part: PartId, kind: LotKind) -> &mut LotStats {
        &mut self.lots[part * KINDS.len() + kind as usize]
    }

    /// Integrates the WIP level `wip` that held since the last change up to `now`.
    pub(super) fn wip(&mut self, now: Time, wip: usize) {
        self.wip_area += wip as f64 * (now - self.wip_since) as f64;
        self.wip_since = now;
    }

    pub(super) fn started(&mut self, part: PartId, kind: LotKind) {
        self.lot(part, kind).started += 1;
    }

    pub(super) fn completed(
        &mut self,
        part: PartId,
        kind: LotKind,
        cycle_time: Time,
        flow_factor: f64,
        on_time: bool,
    ) {
        let lot = self.lot(part, kind);
        lot.cycle_times.push(cycle_time);
        lot.flow_factors.push(flow_factor);
        lot.on_time += u64::from(on_time);
    }

    pub(super) fn step(&mut self, route: RouteId, step: usize, flow_factor: f64) {
        let (sum, visits) = &mut self.steps[route][step];
        *sum += flow_factor;
        *visits += 1;
    }

    /// A completion of CQT `segment` (entrance step `entry`) that waited `wait` under `limit`,
    /// with the lot's visits to the segment's steps; true if over the limit.
    pub(super) fn cqt(
        &mut self,
        segment: usize,
        entry: StepIndex,
        wait: Time,
        limit: Time,
        visits: &[Visit],
    ) -> bool {
        self.cqt_total.add(wait, limit);
        self.segment_totals[segment].add(wait, limit);
        let stats = &mut self.segments[segment];
        let violated = stats.cqt.add(wait, limit);
        for visit in visits {
            let times = &mut stats.steps[visit.step - entry - 1][usize::from(violated)];
            times.visits += 1;
            times.transport += visit.transport;
            times.queue += visit.queue;
            times.process += visit.process;
        }
        violated
    }

    pub(super) fn step_flow_factors(&self) -> Vec<Vec<Option<f64>>> {
        self.steps
            .iter()
            .map(|steps| {
                steps
                    .iter()
                    .map(|&(sum, visits)| (visits > 0).then(|| sum / visits as f64))
                    .collect()
            })
            .collect()
    }

    /// Report of the window ending `now`; tools must be accounted up to `now`.
    pub(super) fn report(
        &self,
        name: String,
        now: Time,
        data: &Dataset,
        routes: &Routes,
        tools: &[Tool],
    ) -> PeriodReport {
        let mut lots = Vec::new();
        for (index, stats) in self.lots.iter().enumerate() {
            if stats.started == 0 && stats.cycle_times.is_empty() {
                continue;
            }
            let completed = stats.cycle_times.len();
            let (mut mean, mut std, mut flow_factor) = (None, None, None);
            if completed > 0 {
                let count = completed as f64;
                let average = stats.cycle_times.iter().map(|&ct| ct as f64).sum::<f64>() / count;
                let variance = stats
                    .cycle_times
                    .iter()
                    .map(|&ct| (ct as f64 - average).powi(2))
                    .sum::<f64>()
                    / count;
                mean = Some(average);
                std = Some(variance.sqrt());
                flow_factor = Some(stats.flow_factors.iter().sum::<f64>() / count);
            }
            lots.push(LotReport {
                part: data.parts[index / KINDS.len()].name.clone(),
                kind: KINDS[index % KINDS.len()],
                started: stats.started,
                completed: completed as u64,
                on_time: stats.on_time,
                cycle_time_mean: mean,
                cycle_time_std: std,
                flow_factor_mean: flow_factor,
            });
        }
        let flow_factors = KINDS
            .iter()
            .filter_map(|&kind| {
                let mut values: Vec<f64> = (0..data.parts.len())
                    .flat_map(|part| {
                        self.lots[part * KINDS.len() + kind as usize]
                            .flow_factors
                            .iter()
                            .copied()
                    })
                    .collect();
                values.sort_by(f64::total_cmp);
                (!values.is_empty()).then(|| FlowFactors {
                    kind,
                    percentiles: FLOW_FACTOR_PERCENTILES
                        .map(|percent| percentile(&values, percent / 100.0)),
                })
            })
            .collect();
        let mut times = vec![[0; STATES]; data.tool_groups.len()];
        for tool in tools {
            for (total, time) in times[tool.group].iter_mut().zip(tool.time) {
                *total += time;
            }
        }
        let tool_groups = data
            .tool_groups
            .iter()
            .zip(times)
            .map(|(group, time)| ToolGroupReport {
                name: group.name.clone(),
                area: data.areas[group.area].clone(),
                tools: group.tools,
                time: StateTimes {
                    down: time[ToolState::Down as usize],
                    pm: time[ToolState::Pm as usize],
                    setup: time[ToolState::Setup as usize],
                    process: time[ToolState::Process as usize],
                    load: time[ToolState::Load as usize],
                    unload: time[ToolState::Unload as usize],
                    idle: time[ToolState::Idle as usize],
                },
            })
            .collect();
        let (mut cqt_litho, mut cqt_rest) = (CqtReport::default(), CqtReport::default());
        let cqt_segments = routes
            .segments
            .iter()
            .zip(&self.segments)
            .map(|(segment, stats)| {
                let total = if segment.litho {
                    &mut cqt_litho
                } else {
                    &mut cqt_rest
                };
                *total = total.merge(&stats.cqt);
                CqtSegmentReport {
                    route: data.routes[segment.route].name.clone(),
                    entry: segment.entry,
                    exit: segment.exit,
                    litho: segment.litho,
                    cqt: stats.cqt,
                    steps: (segment.entry + 1..=segment.exit)
                        .zip(&stats.steps)
                        .map(|(step, &[met, violated])| CqtStepReport {
                            step,
                            met,
                            violated,
                        })
                        .collect(),
                }
            })
            .collect();
        PeriodReport {
            name,
            start: self.since,
            end: now,
            lots,
            flow_factors,
            wip: if now > self.since {
                self.wip_area / (now - self.since) as f64
            } else {
                0.0
            },
            tool_groups,
            cqt_litho,
            cqt_rest,
            cqt_segments,
        }
    }

    /// Starts a new window at `now`; the WIP must be integrated up to `now`.
    pub(super) fn reset(&mut self, now: Time) {
        self.since = now;
        self.wip_area = 0.0;
        self.lots
            .iter_mut()
            .for_each(|lot| *lot = LotStats::default());
        for segment in &mut self.segments {
            segment.cqt = CqtReport::default();
            segment.steps.fill([CqtTimes::default(); 2]);
        }
        self.steps
            .iter_mut()
            .flatten()
            .for_each(|step| *step = (0.0, 0));
    }
}

/// The days of a run ([`Results::days`]): the open day closes at the first event at or after
/// its end, and at the end of the run, from totals of the whole run.
pub(super) struct Days {
    /// End of the open day.
    pub end: Time,
    wip_area: f64,
    wip_since: Time,
    /// Lots released and completed and CQT completions before the open day.
    before: (u64, u64, CqtReport),
    pub reports: Vec<DayReport>,
}

impl Days {
    pub(super) fn new() -> Self {
        Self {
            end: DAY,
            wip_area: 0.0,
            wip_since: 0,
            before: (0, 0, CqtReport::default()),
            reports: Vec::new(),
        }
    }

    /// Integrates the WIP level `wip` that held since the last change up to `now`.
    pub(super) fn wip(&mut self, now: Time, wip: usize) {
        self.wip_area += wip as f64 * (now - self.wip_since) as f64;
        self.wip_since = now;
    }

    /// Closes the open day at `at`, its end or the end of the run, given the totals then.
    pub(super) fn close(
        &mut self,
        at: Time,
        wip: usize,
        released: u64,
        completed: u64,
        cqt: &CqtReport,
    ) {
        self.wip(at, wip);
        let length = at - (self.end - DAY);
        let (released_before, completed_before, cqt_before) = &self.before;
        self.reports.push(DayReport {
            started: released - released_before,
            completed: completed - completed_before,
            wip: if length > 0 {
                self.wip_area / length as f64
            } else {
                0.0
            },
            cqt: cqt.since(cqt_before),
        });
        self.wip_area = 0.0;
        self.before = (released, completed, *cqt);
        self.end += DAY;
    }
}

/// Linear interpolation between closest ranks of ascending `values`, `p` in [0, 1].
fn percentile(values: &[f64], p: f64) -> f64 {
    let position = p * (values.len() - 1) as f64;
    let (low, high) = (position.floor() as usize, position.ceil() as usize);
    values[low] + (values[high] - values[low]) * (position - low as f64)
}
