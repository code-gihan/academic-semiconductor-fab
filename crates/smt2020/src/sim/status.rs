//! The fab's state at the simulation time: its lots, tools and tool groups.

use des_core::Time;
use serde::Serialize;

use super::fab::{Fab, LotState};
use super::stats::LotKind;
use super::tool::{STATES, ToolState};

/// A lot in the fab.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct LotStatus {
    /// Release number: lots are numbered from 0 in release order.
    pub id: u64,
    pub part: String,
    pub kind: LotKind,
    /// Dispatching priority: the dataset's, or the engineering strategy's.
    pub priority: u32,
    pub wafers: u32,
    pub release: Time,
    pub due: Time,
    /// The step the lot moves to, waits at or is processed at: its index in the route, its name
    /// and its tool group.
    pub step: usize,
    pub step_name: String,
    pub tool_group: String,
    pub state: LotState,
    /// The tool processing the lot ([`ToolStatus::id`]).
    pub tool: Option<usize>,
    /// The CQT segment the lot is in, until its exit step ends: that step's index and the latest
    /// start of it within the limit.
    pub cqt_exit: Option<usize>,
    pub cqt_deadline: Option<Time>,
}

/// A tool.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ToolStatus {
    /// Position among all tools, tool group by tool group in dataset order.
    pub id: usize,
    pub tool_group: String,
    pub state: ToolState,
    pub setup: Option<String>,
    /// Lots in process ([`LotStatus::id`]); a cascading tool runs two jobs.
    pub lots: Vec<u64>,
}

/// A tool group: its queue and its tools per state.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ToolGroupStatus {
    pub name: String,
    pub area: String,
    pub tools: u32,
    /// Lots queued.
    pub queue: usize,
    pub down: u32,
    pub pm: u32,
    pub setup: u32,
    pub process: u32,
    pub load: u32,
    pub unload: u32,
    pub idle: u32,
}

impl Fab {
    /// The lots in the fab, by id.
    pub(super) fn lot_statuses(&self) -> Vec<LotStatus> {
        let mut processing = vec![None; self.lots.len()];
        for (tool_id, tool) in self.tools.iter().enumerate() {
            for job in tool.jobs.iter().filter(|job| job.active) {
                for &lot in &job.lots {
                    processing[lot] = Some(tool_id);
                }
            }
        }
        let mut lots: Vec<LotStatus> = self
            .lots
            .iter()
            .zip(processing)
            .filter(|(lot, _)| lot.alive)
            .map(|(lot, tool)| {
                let step = &self.data.routes[lot.route].steps[lot.step];
                LotStatus {
                    id: lot.serial,
                    part: self.data.parts[lot.part].name.clone(),
                    kind: lot.kind,
                    priority: lot.priority,
                    wafers: lot.wafers,
                    release: lot.release,
                    due: lot.due,
                    step: lot.step,
                    step_name: step.name.clone(),
                    tool_group: self.data.tool_groups[step.tool_group].name.clone(),
                    state: lot.state,
                    tool,
                    cqt_exit: lot.segment.as_ref().map(|segment| segment.exit),
                    cqt_deadline: lot
                        .segment
                        .as_ref()
                        .map(|segment| segment.entered + segment.limit),
                }
            })
            .collect();
        lots.sort_unstable_by_key(|lot| lot.id);
        lots
    }

    /// Every tool at `now`, by id.
    pub(super) fn tool_statuses(&self, now: Time) -> Vec<ToolStatus> {
        self.tools
            .iter()
            .enumerate()
            .map(|(id, tool)| ToolStatus {
                id,
                tool_group: self.data.tool_groups[tool.group].name.clone(),
                state: tool.state(now),
                setup: tool.setup.map(|setup| self.data.setups[setup].clone()),
                lots: tool
                    .jobs
                    .iter()
                    .filter(|job| job.active)
                    .flat_map(|job| &job.lots)
                    .map(|&lot| self.lots[lot].serial)
                    .collect(),
            })
            .collect()
    }

    /// Every tool group at `now`, in dataset order.
    pub(super) fn tool_group_statuses(&self, now: Time) -> Vec<ToolGroupStatus> {
        let mut states = vec![[0; STATES]; self.groups.len()];
        for tool in &self.tools {
            states[tool.group][tool.state(now) as usize] += 1;
        }
        self.data
            .tool_groups
            .iter()
            .zip(&self.groups)
            .zip(states)
            .map(|((spec, group), states)| {
                let tools = |state: ToolState| states[state as usize];
                ToolGroupStatus {
                    name: spec.name.clone(),
                    area: self.data.areas[spec.area].clone(),
                    tools: spec.tools,
                    queue: group.queue.len(),
                    down: tools(ToolState::Down),
                    pm: tools(ToolState::Pm),
                    setup: tools(ToolState::Setup),
                    process: tools(ToolState::Process),
                    load: tools(ToolState::Load),
                    unload: tools(ToolState::Unload),
                    idle: tools(ToolState::Idle),
                }
            })
            .collect()
    }
}
