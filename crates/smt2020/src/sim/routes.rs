//! Route quantities derived once per run: expected step durations, remaining work, raw processing
//! time, batch compatibility and the steps that need each setup.

use std::collections::HashMap;

use crate::data::{
    BatchCriterion, Dataset, LocationId, PartId, Route, RouteId, SetupId, StepIndex, ToolGroupId,
    Unit,
};

/// `base + per_wafer · wafers`, in ms.
#[derive(Clone, Copy, Default)]
pub(super) struct Affine {
    pub base: f64,
    pub per_wafer: f64,
}

impl Affine {
    pub(super) fn at(self, wafers: u32) -> f64 {
        self.base + self.per_wafer * f64::from(wafers)
    }

    fn plus(self, other: Self) -> Self {
        Self {
            base: self.base + other.base,
            per_wafer: self.per_wafer + other.per_wafer,
        }
    }

    fn times(self, factor: f64) -> Self {
        Self {
            base: self.base * factor,
            per_wafer: self.per_wafer * factor,
        }
    }
}

pub(super) struct RouteInfo {
    /// Expected duration of each step when processed: load, processing (a lot alone on a
    /// cascading tool takes p + (n−1)·c) and unload.
    pub step: Vec<Affine>,
    /// Sampling-weighted expected step durations from each step to the end (no transport or
    /// rework); one extra zero entry at the end.
    pub remaining: Vec<Affine>,
    /// Raw processing time: expected cycle time of a lot alone in the fab.
    pub rpt: Affine,
    /// The CQT segment starting at each step includes stepper steps.
    pub cqt_litho: Vec<bool>,
    /// Distinct tool groups of the CQT segment starting at each step, after the entrance step
    /// (stopping limits); empty without a segment.
    pub segment_groups: Vec<Vec<ToolGroupId>>,
}

/// A CQT segment of a route ([`Dataset::segments`] order).
pub(super) struct CqtSegment {
    pub route: RouteId,
    pub entry: StepIndex,
    pub exit: StepIndex,
    /// Includes stepper steps.
    pub litho: bool,
}

pub(super) struct Routes {
    pub info: Vec<RouteInfo>,
    pub segments: Vec<CqtSegment>,
    /// Per route and step: the segment starting at the end of the step.
    pub segment_at: Vec<Vec<Option<usize>>>,
    /// Batch compatibility key per part and step (batch steps only); a key's steps share a tool
    /// group.
    pub batch_key: Vec<Vec<Option<usize>>>,
    /// (part, step) pairs per batch key.
    pub batch_members: Vec<Vec<(PartId, StepIndex)>>,
    /// (part, step) pairs per tool group and the setup they need there.
    pub setup_members: HashMap<(ToolGroupId, SetupId), Vec<(PartId, StepIndex)>>,
}

impl Routes {
    pub(super) fn new(data: &Dataset, steppers: &[bool]) -> Self {
        #[derive(PartialEq, Eq, Hash)]
        enum Key<'a> {
            RouteStep(usize, StepIndex),
            FamilyStep(ToolGroupId, &'a str, &'a str),
        }
        let mut keys = HashMap::new();
        let mut batch_members: Vec<Vec<(PartId, StepIndex)>> = Vec::new();
        let batch_key = data
            .parts
            .iter()
            .enumerate()
            .map(|(part, spec)| {
                let steps = &data.routes[spec.route].steps;
                (0..steps.len())
                    .map(|index| {
                        let key = match data.tool_groups[steps[index].tool_group].batching? {
                            BatchCriterion::SameRouteStep => Key::RouteStep(spec.route, index),
                            BatchCriterion::SameFamilyStepName => Key::FamilyStep(
                                steps[index].tool_group,
                                &spec.family,
                                &steps[index].name,
                            ),
                        };
                        let next = keys.len();
                        let id = *keys.entry(key).or_insert(next);
                        if id == batch_members.len() {
                            batch_members.push(Vec::new());
                        }
                        batch_members[id].push((part, index));
                        Some(id)
                    })
                    .collect()
            })
            .collect();
        let mut setup_members: HashMap<_, Vec<_>> = HashMap::new();
        for (part, spec) in data.parts.iter().enumerate() {
            for (index, step) in data.routes[spec.route].steps.iter().enumerate() {
                if let Some(setup) = step.setup {
                    let members = setup_members.entry((step.tool_group, setup.setup));
                    members.or_default().push((part, index));
                }
            }
        }
        let info: Vec<RouteInfo> = data
            .routes
            .iter()
            .map(|route| RouteInfo::new(data, route, steppers))
            .collect();
        let mut segment_at: Vec<Vec<Option<usize>>> = data
            .routes
            .iter()
            .map(|route| vec![None; route.steps.len()])
            .collect();
        let segments = data
            .segments()
            .enumerate()
            .map(|(index, (route, entry, cqt))| {
                segment_at[route][entry] = Some(index);
                CqtSegment {
                    route,
                    entry,
                    exit: cqt.until,
                    litho: info[route].cqt_litho[entry],
                }
            })
            .collect();
        Self {
            info,
            segments,
            segment_at,
            batch_key,
            batch_members,
            setup_members,
        }
    }
}

impl RouteInfo {
    fn new(data: &Dataset, route: &Route, steppers: &[bool]) -> Self {
        let steps = &route.steps;
        let step: Vec<Affine> = steps
            .iter()
            .map(|step| {
                let group = &data.tool_groups[step.tool_group];
                let handling = (group.load + group.unload) as f64;
                let mean = step.time.mean() as f64;
                match (step.unit, step.cascade_interval) {
                    (Unit::Wafer, None) => Affine {
                        base: handling,
                        per_wafer: mean,
                    },
                    (Unit::Wafer, Some(interval)) => Affine {
                        base: handling + mean - interval as f64,
                        per_wafer: interval as f64,
                    },
                    _ => Affine {
                        base: handling + mean,
                        per_wafer: 0.0,
                    },
                }
            })
            .collect();

        let mut remaining = vec![Affine::default(); steps.len() + 1];
        for index in (0..steps.len()).rev() {
            remaining[index] = remaining[index + 1].plus(step[index].times(steps[index].sampling));
        }

        // Expected transport from the previous processed step: track where the lot last was.
        let locations = data.locations.len();
        let mut last = vec![0.0; locations + 1];
        last[locations] = 1.0; // not yet processed: no transport
        let mut visits = Vec::with_capacity(steps.len());
        for (index, spec) in steps.iter().enumerate() {
            let to = data.tool_groups[spec.tool_group].location;
            let transport: f64 = (0..locations)
                .map(|from| last[from] * transport_mean(data, from, to))
                .sum();
            visits.push(
                step[index]
                    .plus(Affine {
                        base: transport,
                        per_wafer: 0.0,
                    })
                    .times(spec.sampling),
            );
            last.iter_mut()
                .for_each(|share| *share *= 1.0 - spec.sampling);
            last[to] += spec.sampling;
        }
        let mut rpt = visits
            .iter()
            .fold(Affine::default(), |sum, &visit| sum.plus(visit));
        // A loop reworked with probability q per pass is redone q/(1−q) times on average.
        for (index, spec) in steps.iter().enumerate() {
            if let Some(rework) = spec.rework {
                let redo = visits[rework.to..=index]
                    .iter()
                    .fold(Affine::default(), |sum, &visit| sum.plus(visit));
                let q = spec.sampling * rework.probability;
                rpt = rpt.plus(redo.times(q / (1.0 - q)));
            }
        }

        let cqt_litho = (0..steps.len())
            .map(|index| {
                steps[index].cqt.is_some_and(|cqt| {
                    steps[index..=cqt.until]
                        .iter()
                        .any(|step| steppers[step.tool_group])
                })
            })
            .collect();
        let segment_groups = steps
            .iter()
            .enumerate()
            .map(|(index, step)| {
                let mut groups = Vec::new();
                if let Some(cqt) = step.cqt {
                    for later in &steps[index + 1..=cqt.until] {
                        if !groups.contains(&later.tool_group) {
                            groups.push(later.tool_group);
                        }
                    }
                }
                groups
            })
            .collect();
        Self {
            step,
            remaining,
            rpt,
            cqt_litho,
            segment_groups,
        }
    }
}

fn transport_mean(data: &Dataset, from: LocationId, to: LocationId) -> f64 {
    data.transports
        .iter()
        .find(|transport| (transport.from, transport.to) == (from, to))
        .map_or(0.0, |transport| transport.time.mean() as f64)
}
