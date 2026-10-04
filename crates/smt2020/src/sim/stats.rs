//! Statistics accumulated since the last reset and the reports taken at period ends.

use des_core::{HOUR, Time};

use super::tool::{STATES, Tool};
use crate::data::{Dataset, PartId, RouteId, ToolGroupId};

/// Lot categories of the papers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum LotKind {
    /// Production regular lot (priority 10).
    Prl,
    /// Production hot lot (priority 20).
    Phl,
    /// Super hot lot (priority 30).
    Shl,
    /// Engineering regular lot (priority 10).
    Erl,
    /// Engineering hot lot (priority 20).
    Ehl,
}

const KINDS: [LotKind; 5] = [
    LotKind::Prl,
    LotKind::Phl,
    LotKind::Shl,
    LotKind::Erl,
    LotKind::Ehl,
];

impl LotKind {
    pub(super) fn of(engineering: bool, priority: u32) -> Option<Self> {
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

#[derive(Clone, Debug)]
pub struct Results {
    /// Reporting periods (REPORT = yes) up to the horizon, then `Drain` covering the completion
    /// of the lots still in the fab at the horizon.
    pub periods: Vec<PeriodReport>,
    /// Lots released (plan and initial WIP) and completed; equal for a finished run.
    pub released: u64,
    pub completed: u64,
    /// Completion of the last lot.
    pub end: Time,
    pub events: u64,
    /// Per route and step: mean step cycle time (since the previous processed step) over the
    /// expected step duration, in the window ending at the horizon; NaN if never processed.
    pub step_flow_factors: Vec<Vec<f64>>,
}

/// Statistics of one window, from the last reset to the period end.
#[derive(Clone, Debug)]
pub struct PeriodReport {
    pub name: String,
    pub start: Time,
    pub end: Time,
    /// Per part and lot kind with releases or completions in the window.
    pub lots: Vec<LotReport>,
    /// Flow factor percentiles 0, 5, 25, 50, 75, 95, 100 per lot kind over all parts.
    pub flow_factors: Vec<(LotKind, [f64; 7])>,
    /// Time-averaged lots in the fab.
    pub wip: f64,
    pub tool_groups: Vec<ToolGroupReport>,
    /// CQT segments with stepper (LithoTrack_FE_95/115) steps, and all others.
    pub cqt_litho: CqtReport,
    pub cqt_rest: CqtReport,
}

#[derive(Clone, Debug)]
pub struct LotReport {
    pub part: PartId,
    pub kind: LotKind,
    pub started: u64,
    pub completed: u64,
    pub on_time: u64,
    /// Mean and standard deviation of the cycle time in ms.
    pub cycle_time: (f64, f64),
    /// Mean of the lots' flow factors (cycle time over raw processing time).
    pub flow_factor: f64,
}

#[derive(Clone, Debug)]
pub struct ToolGroupReport {
    pub group: ToolGroupId,
    /// Tool time per [`super::State`] in ms, summed over the group's tools.
    pub time: [Time; STATES],
}

/// CQT waits measured at the start of the exit step.
#[derive(Clone, Copy, Debug, Default)]
pub struct CqtReport {
    pub completed: u64,
    pub violated: u64,
    /// Violations longer than 1, 2 and 4 hours.
    pub violated_over: [u64; 3],
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
            cqt.violated += 1;
            cqt.violation += wait - limit;
            for (count, hours) in cqt.violated_over.iter_mut().zip([1, 2, 4]) {
                *count += u64::from(wait - limit > hours * HOUR);
            }
        } else {
            cqt.slack += limit - wait;
        }
    }

    pub(super) fn step_flow_factors(&self) -> Vec<Vec<f64>> {
        self.steps
            .iter()
            .map(|steps| {
                steps
                    .iter()
                    .map(|&(sum, visits)| sum / visits as f64)
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
            let count = stats.cycle_times.len() as f64;
            let mean = stats.cycle_times.iter().map(|&ct| ct as f64).sum::<f64>() / count;
            let variance = stats
                .cycle_times
                .iter()
                .map(|&ct| (ct as f64 - mean).powi(2))
                .sum::<f64>()
                / count;
            lots.push(LotReport {
                part: index / KINDS.len(),
                kind: KINDS[index % KINDS.len()],
                started: stats.started,
                completed: stats.cycle_times.len() as u64,
                on_time: stats.on_time,
                cycle_time: (mean, variance.sqrt()),
                flow_factor: stats.flow_factors.iter().sum::<f64>() / count,
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
                (!values.is_empty()).then(|| {
                    (
                        kind,
                        [0.0, 0.05, 0.25, 0.5, 0.75, 0.95, 1.0].map(|p| percentile(&values, p)),
                    )
                })
            })
            .collect();
        let mut tool_groups: Vec<ToolGroupReport> = (0..data.tool_groups.len())
            .map(|group| ToolGroupReport {
                group,
                time: [0; STATES],
            })
            .collect();
        for tool in tools {
            for (total, time) in tool_groups[tool.group].time.iter_mut().zip(tool.time) {
                *total += time;
            }
        }
        let wip_area = self.wip_area;
        PeriodReport {
            name,
            start: self.since,
            end: now,
            lots,
            flow_factors,
            wip: if now > self.since {
                wip_area / (now - self.since) as f64
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

/// Linear interpolation between closest ranks of ascending `values`.
fn percentile(values: &[f64], p: f64) -> f64 {
    let position = p * (values.len() - 1) as f64;
    let (low, high) = (position.floor() as usize, position.ceil() as usize);
    values[low] + (values[high] - values[low]) * (position - low as f64)
}
