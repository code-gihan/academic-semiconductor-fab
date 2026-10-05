//! Statistics accumulated since the last reset and the reports taken at period ends.

use des_core::{HOUR, Time};
use serde::{Deserialize, Serialize};

use super::tool::{STATES, Tool, ToolState};
use crate::data::{Dataset, PartId, RouteId};

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
    /// Reporting periods (REPORT = yes) up to the horizon, then `Drain` covering the completion
    /// of the lots still in the fab at the horizon.
    pub periods: Vec<PeriodReport>,
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

#[derive(Default)]
struct LotStats {
    started: u64,
    on_time: u64,
    cycle_times: Vec<Time>,
    flow_factors: Vec<f64>,
}

pub(super) struct Stats {
    since: Time,
    wip_since: Time,
    wip_area: f64,
    /// Per part and kind.
    lots: Vec<LotStats>,
    cqt: [CqtReport; 2],
    /// Per route and step: sum of step cycle time over expected duration, and visits.
    steps: Vec<Vec<(f64, u64)>>,
}

impl Stats {
    pub(super) fn new(data: &Dataset) -> Self {
        Self {
            since: 0,
            wip_since: 0,
            wip_area: 0.0,
            lots: (0..data.parts.len() * KINDS.len())
                .map(|_| LotStats::default())
                .collect(),
            cqt: [CqtReport::default(); 2],
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

    pub(super) fn cqt(&mut self, litho: bool, wait: Time, limit: Time) {
        let cqt = &mut self.cqt[usize::from(litho)];
        cqt.completed += 1;
        if wait > limit {
            let excess = wait - limit;
            cqt.violated += 1;
            cqt.violation += excess;
            cqt.violated_1h += u64::from(excess > HOUR);
            cqt.violated_2h += u64::from(excess > 2 * HOUR);
            cqt.violated_4h += u64::from(excess > 4 * HOUR);
        } else {
            cqt.slack += limit - wait;
        }
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
            cqt_litho: self.cqt[1],
            cqt_rest: self.cqt[0],
        }
    }

    /// Starts a new window at `now`; the WIP must be integrated up to `now`.
    pub(super) fn reset(&mut self, now: Time) {
        self.since = now;
        self.wip_area = 0.0;
        self.lots
            .iter_mut()
            .for_each(|lot| *lot = LotStats::default());
        self.cqt = [CqtReport::default(); 2];
        self.steps
            .iter_mut()
            .flatten()
            .for_each(|step| *step = (0.0, 0));
    }
}

/// Linear interpolation between closest ranks of ascending `values`, `p` in [0, 1].
fn percentile(values: &[f64], p: f64) -> f64 {
    let position = p * (values.len() - 1) as f64;
    let (low, high) = (position.floor() as usize, position.ceil() as usize);
    values[low] + (values[high] - values[low]) * (position - low as f64)
}
