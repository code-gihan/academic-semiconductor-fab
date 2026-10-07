//! What a dataset holds, by name and index: the areas, tool groups, parts, routes, CQT segments
//! and reporting periods that configurations name and records index.

use des_core::Time;
use serde::Serialize;

use crate::data::{Dataset, Rule};
use crate::layout::PortKind;
use crate::sim::{Criterion, STEPPERS};

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct DatasetInfo {
    pub areas: Vec<String>,
    pub tool_groups: Vec<ToolGroupInfo>,
    pub parts: Vec<PartInfo>,
    pub routes: Vec<RouteInfo>,
    /// CQT segments in route and step order ([`Dataset::segments`]).
    pub segments: Vec<SegmentInfo>,
    pub periods: Vec<PeriodInfo>,
    /// The AMHS layout's size; none without one.
    pub layout: Option<LayoutInfo>,
}

/// An AMHS layout's size: its vehicles, bays, rails (with their length, m) and ports.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct LayoutInfo {
    pub vehicles: usize,
    pub bays: usize,
    pub rails: usize,
    pub rail_length: f64,
    pub zones: usize,
    pub tool_ports: usize,
    pub buffers: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ToolGroupInfo {
    pub name: String,
    /// Index in [`DatasetInfo::areas`].
    pub area: usize,
    pub tools: u32,
    /// Processes batches.
    pub batching: bool,
    /// Runs setups of a minimum length (`rule_LSSU`).
    pub setup_runs: bool,
    /// Stepper of the CAtE/CoT rules and of the Litho CQT statistics.
    pub stepper: bool,
    /// The dataset's lot ranking, most significant first.
    pub ranks: Vec<Criterion>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PartInfo {
    pub name: String,
    pub family: String,
    pub engineering: bool,
    /// Index in [`DatasetInfo::routes`].
    pub route: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RouteInfo {
    pub name: String,
    pub steps: Vec<StepInfo>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct StepInfo {
    pub name: String,
    /// Index in [`DatasetInfo::tool_groups`].
    pub tool_group: usize,
}

/// A CQT segment: the queue time from the end of step `entry` to the start of step `exit` of
/// route `route` is at most `limit`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SegmentInfo {
    pub route: usize,
    pub entry: usize,
    pub exit: usize,
    pub limit: Time,
    /// Includes stepper steps (the Litho CQT statistics).
    pub litho: bool,
    /// Distinct tool groups after the entrance step through the exit step (stopping limits).
    pub tool_groups: Vec<usize>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PeriodInfo {
    pub name: String,
    pub start: Time,
    pub report: bool,
    /// Statistics restart at the end of the period.
    pub reset: bool,
}

impl Dataset {
    pub fn info(&self) -> DatasetInfo {
        let stepper = |group: usize| STEPPERS.contains(&self.tool_groups[group].name.as_str());
        DatasetInfo {
            areas: self.areas.clone(),
            tool_groups: self
                .tool_groups
                .iter()
                .enumerate()
                .map(|(index, group)| ToolGroupInfo {
                    name: group.name.clone(),
                    area: group.area,
                    tools: group.tools,
                    batching: group.batching.is_some(),
                    setup_runs: matches!(group.rule, Rule::SetupRun(_)),
                    stepper: stepper(index),
                    ranks: group.ranks.iter().map(|&rank| rank.into()).collect(),
                })
                .collect(),
            parts: self
                .parts
                .iter()
                .map(|part| PartInfo {
                    name: part.name.clone(),
                    family: part.family.clone(),
                    engineering: part.engineering,
                    route: part.route,
                })
                .collect(),
            routes: self
                .routes
                .iter()
                .map(|route| RouteInfo {
                    name: route.name.clone(),
                    steps: route
                        .steps
                        .iter()
                        .map(|step| StepInfo {
                            name: step.name.clone(),
                            tool_group: step.tool_group,
                        })
                        .collect(),
                })
                .collect(),
            segments: self
                .segments()
                .map(|(route, entry, cqt)| {
                    let steps = &self.routes[route].steps;
                    let mut tool_groups = Vec::new();
                    for step in &steps[entry + 1..=cqt.until] {
                        if !tool_groups.contains(&step.tool_group) {
                            tool_groups.push(step.tool_group);
                        }
                    }
                    SegmentInfo {
                        route,
                        entry,
                        exit: cqt.until,
                        limit: cqt.limit,
                        litho: steps[entry..=cqt.until]
                            .iter()
                            .any(|step| stepper(step.tool_group)),
                        tool_groups,
                    }
                })
                .collect(),
            periods: self
                .periods
                .iter()
                .map(|period| PeriodInfo {
                    name: period.name.clone(),
                    start: period.start,
                    report: period.report,
                    reset: period.reset,
                })
                .collect(),
            layout: self.layout.as_ref().map(|layout| {
                let ports = |role: fn(&PortKind) -> bool| {
                    layout.ports.iter().filter(|port| role(&port.kind)).count()
                };
                LayoutInfo {
                    vehicles: layout.vehicles.len(),
                    bays: layout.bays.len(),
                    rails: layout.links.len(),
                    rail_length: layout.links.iter().map(|link| link.length).sum::<f64>() / 1_000.0,
                    zones: layout.zones.len(),
                    tool_ports: ports(|kind| matches!(kind, PortKind::Tool(_))),
                    buffers: ports(|kind| matches!(kind, PortKind::Buffer(_))),
                }
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::data::tiny;
    use crate::sim::Criterion;

    #[test]
    fn info_names_and_indexes_the_dataset() {
        let info = tiny().info();
        assert_eq!(info.areas, ["Etch"]);
        let group = &info.tool_groups[0];
        assert_eq!(
            (group.name.as_str(), group.area, group.tools),
            ("Etch_1", 0, 2)
        );
        assert!(!group.batching && !group.setup_runs && !group.stepper);
        assert_eq!(group.ranks, [Criterion::Priority, Criterion::Fifo]);
        assert_eq!(info.routes[0].steps[0].tool_group, 0);
        assert_eq!(info.parts[0].route, 0);
        assert!(info.segments.is_empty());
        assert_eq!(info.periods[0].name, "P");
    }
}
