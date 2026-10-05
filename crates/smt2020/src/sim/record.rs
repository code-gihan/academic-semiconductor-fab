//! What a simulation records besides its results ([`Recording`]): the lots over a CQT limit, the
//! tool groups day by day and the events of a window. Records are tables of equal-length columns
//! that refer to the dataset info by index. Recording never changes the run.

use std::collections::HashSet;

use des_core::Time;
use serde::{Deserialize, Serialize};

use super::stats::LotKind;
use super::tool::STATES;
use super::{Error, deserialize_time};
use crate::data::{Dataset, PartId, StepIndex, ToolGroupId};

/// What to record ([`Simulation::with_recording`](super::Simulation::with_recording)).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Recording {
    /// Every CQT segment completion over its limit.
    #[serde(default)]
    pub violations: bool,
    /// Each tool group's mean queue and tool time per state, day by day.
    #[serde(default)]
    pub tool_groups: bool,
    /// The events of a window.
    #[serde(default)]
    pub events: Option<EventFilter>,
}

/// Events from `from` up to `until`, at these tool groups (none: all) and of these lots (release
/// numbers; none: all). Events without a tool group (release, completion) pass no tool group
/// filter, events without a lot (tool outages) no lot filter.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventFilter {
    #[serde(deserialize_with = "deserialize_time")]
    pub from: Time,
    #[serde(deserialize_with = "deserialize_time")]
    pub until: Time,
    #[serde(default)]
    pub tool_groups: Vec<String>,
    #[serde(default)]
    pub lots: Vec<u64>,
}

named_enum! {
    /// An event of the events record.
    pub enum EventKind {
        /// A lot enters the fab.
        Release = "release",
        /// A lot joins the queue of its step's tool group.
        Arrive = "arrive",
        /// A lot's job starts on a tool (setup first, if any).
        Start = "start",
        /// A lot's step ends on a tool.
        End = "end",
        /// A lot leaves the fab after its last step.
        Complete = "complete",
        /// A tool breaks down, and is repaired.
        Down = "down",
        Up = "up",
        /// A tool's preventive maintenance starts, and ends.
        PmStart = "pm_start",
        PmEnd = "pm_end",
    }
}

/// The tables recorded.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Records {
    pub violations: Violations,
    pub tool_groups: ToolGroupDays,
    pub events: Events,
}

/// CQT segment completions over the limit, in time order.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Violations {
    /// Release number of the lot.
    pub lot: Vec<u64>,
    pub part: Vec<PartId>,
    pub kind: Vec<LotKind>,
    /// Segment index (dataset info).
    pub segment: Vec<usize>,
    pub release: Vec<Time>,
    /// End of the entrance step, arrival in the exit step's queue, start of the exit step.
    pub entered: Vec<Time>,
    pub arrived: Vec<Time>,
    pub exit: Vec<Time>,
}

/// Per tool group and day (as [`DayReport`](super::DayReport)): the time-averaged queue and the
/// tool time per state, summed over the group's tools (ms).
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct ToolGroupDays {
    pub day: Vec<usize>,
    pub tool_group: Vec<ToolGroupId>,
    pub queue: Vec<f64>,
    pub down: Vec<Time>,
    pub pm: Vec<Time>,
    pub setup: Vec<Time>,
    pub process: Vec<Time>,
    pub load: Vec<Time>,
    pub unload: Vec<Time>,
    pub idle: Vec<Time>,
}

/// Events in time order; a field an event has not is null.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Events {
    pub time: Vec<Time>,
    pub kind: Vec<EventKind>,
    pub lot: Vec<Option<u64>>,
    /// Tool index (tool status).
    pub tool: Vec<Option<usize>>,
    pub tool_group: Vec<Option<ToolGroupId>>,
    pub step: Vec<Option<StepIndex>>,
}

/// The recording of a run: what to record, resolved against the dataset.
pub(super) struct Recorder {
    violations: bool,
    tool_groups: bool,
    events: Option<Window>,
    /// Tool group days: per group, the tool time per state and the queue area when the open day
    /// began.
    pub day_start: Vec<([Time; STATES], f64)>,
}

/// An event filter with tool groups by index.
struct Window {
    from: Time,
    until: Time,
    tool_groups: Option<Vec<bool>>,
    lots: Option<HashSet<u64>>,
}

impl Recorder {
    /// The recorder of `recording` for `data`, or none if it records nothing.
    pub(super) fn new(data: &Dataset, recording: &Recording) -> Result<Option<Self>, Error> {
        let events = match &recording.events {
            None => None,
            Some(filter) => {
                if filter.until <= filter.from {
                    return Err(Error("the event window must end after it starts".into()));
                }
                let tool_groups = if filter.tool_groups.is_empty() {
                    None
                } else {
                    let mut chosen = vec![false; data.tool_groups.len()];
                    for name in &filter.tool_groups {
                        let group = data
                            .tool_groups
                            .iter()
                            .position(|group| &group.name == name)
                            .ok_or_else(|| Error(format!("no tool group {name}")))?;
                        chosen[group] = true;
                    }
                    Some(chosen)
                };
                let lots = (!filter.lots.is_empty()).then(|| filter.lots.iter().copied().collect());
                Some(Window {
                    from: filter.from,
                    until: filter.until,
                    tool_groups,
                    lots,
                })
            }
        };
        if !recording.violations && !recording.tool_groups && events.is_none() {
            return Ok(None);
        }
        Ok(Some(Self {
            violations: recording.violations,
            tool_groups: recording.tool_groups,
            events,
            day_start: vec![([0; STATES], 0.0); data.tool_groups.len()],
        }))
    }

    pub(super) fn violations(&self) -> bool {
        self.violations
    }

    pub(super) fn tool_groups(&self) -> bool {
        self.tool_groups
    }

    /// Records `entry` at `time` into `records` if the window and filters pass it.
    pub(super) fn event(&self, records: &mut Records, time: Time, entry: Entry) {
        let Some(window) = &self.events else {
            return;
        };
        let passes = (window.from..window.until).contains(&time)
            && window
                .tool_groups
                .as_ref()
                .is_none_or(|chosen| entry.tool_group.is_some_and(|group| chosen[group]))
            && window
                .lots
                .as_ref()
                .is_none_or(|lots| entry.lot.is_some_and(|lot| lots.contains(&lot)));
        if passes {
            let events = &mut records.events;
            events.time.push(time);
            events.kind.push(entry.kind);
            events.lot.push(entry.lot);
            events.tool.push(entry.tool);
            events.tool_group.push(entry.tool_group);
            events.step.push(entry.step);
        }
    }
}

/// An event for the events record ([`Events`]).
pub(super) struct Entry {
    pub kind: EventKind,
    pub lot: Option<u64>,
    pub tool: Option<usize>,
    pub tool_group: Option<ToolGroupId>,
    pub step: Option<StepIndex>,
}
