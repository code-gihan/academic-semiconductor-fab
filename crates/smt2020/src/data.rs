//! Static factory model of one SMT2020 dataset, validated and simulation-ready: names resolved to
//! indices, durations in ms, dates relative to the simulation start.

use des_core::Time;

pub type AreaId = usize;
pub type LocationId = usize;
pub type ToolGroupId = usize;
pub type SetupId = usize;
pub type SetupGroupId = usize;
pub type RouteId = usize;
pub type PartId = usize;
/// Position of a step within its route.
pub type StepIndex = usize;

#[derive(Debug)]
pub struct Dataset {
    /// Manufacturing areas (AutoSched station groups).
    pub areas: Vec<String>,
    pub locations: Vec<String>,
    pub tool_groups: Vec<ToolGroup>,
    pub setups: Vec<String>,
    pub setup_changes: Vec<SetupChange>,
    pub setup_groups: Vec<SetupGroup>,
    pub routes: Vec<Route>,
    pub parts: Vec<Part>,
    pub transports: Vec<Transport>,
    /// Periodic lot releases.
    pub streams: Vec<ReleaseStream>,
    /// Individually listed lots, including the initial WIP.
    pub lots: Vec<LotRelease>,
    /// Reporting periods in ascending start order.
    pub periods: Vec<Period>,
}

/// Duration distribution.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Dist {
    Constant(Time),
    /// Continuous uniform on [mean − half_width, mean + half_width].
    Uniform {
        mean: Time,
        half_width: Time,
    },
    Exponential {
        mean: Time,
    },
}

impl Dist {
    /// Smallest value the distribution can take.
    pub fn min(self) -> Time {
        match self {
            Dist::Constant(time) => time,
            Dist::Uniform { mean, half_width } => mean - half_width,
            Dist::Exponential { .. } => 0,
        }
    }

    pub fn mean(self) -> Time {
        match self {
            Dist::Constant(mean) | Dist::Uniform { mean, .. } | Dist::Exponential { mean } => mean,
        }
    }
}

#[derive(Debug)]
pub struct ToolGroup {
    pub name: String,
    pub area: AreaId,
    pub location: LocationId,
    pub tools: u32,
    pub load: Time,
    pub unload: Time,
    /// Two serial slots, so consecutive units are processed in parallel (STNCAP = 2).
    pub cascading: bool,
    /// Batch processing, sized in wafers, and which lots may share a batch.
    pub batching: Option<BatchCriterion>,
    pub rule: Rule,
    /// Lot ranking criteria, most significant first.
    pub ranks: Vec<Rank>,
    /// Among idle tools, an arriving lot wakes the one needing the least setup.
    pub wake_least_setup: bool,
    pub breakdowns: Vec<Breakdown>,
    pub pms: Vec<Pm>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BatchCriterion {
    /// `crit_sameroutestep`
    SameRouteStep,
    /// `crit_samepartfam|crit_samestepname`
    SameFamilyStepName,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rule {
    /// `rule_HotLotFIRST`: first-ranked lot; flagged super hot lots reserve their next tool.
    HotLotFirst,
    /// `rule_LSSU`: setup runs bounded below by the group's minimum run lengths.
    SetupRun(SetupGroupId),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rank {
    /// `rank_HP`: higher priority first.
    Priority,
    /// `rank_RSETUP`: least required setup time first.
    LeastSetup,
    /// `rank_FIFO`: earliest queue arrival first.
    Fifo,
    /// `rank_CR`: smallest critical ratio first.
    CriticalRatio,
}

/// Unscheduled downtime on calendar time.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Breakdown {
    /// Time to the first failure.
    pub first: Dist,
    pub ttf: Dist,
    pub ttr: Dist,
}

/// Scheduled downtime.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pm {
    pub trigger: PmTrigger,
    pub duration: Dist,
}

/// PM start rule. The first PM of the k-th of N tools (k = 1..N) falls at `first`·k/N.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PmTrigger {
    /// Every `interval` of calendar time, duration included (`mtbpm_by_cal`).
    Calendar { interval: Time, first: Time },
    /// Every `interval` wafers processed by the tool (`mtbpm_by_pieces`).
    Wafers { interval: u32, first: u32 },
}

/// Setup duration from `from` (any setup if `None`) to `to`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SetupChange {
    pub from: Option<SetupId>,
    pub to: SetupId,
    pub time: Dist,
}

/// After changing to one of these setups, a tool must process the given number of lots before
/// changing again.
#[derive(Debug, PartialEq)]
pub struct SetupGroup {
    pub name: String,
    pub min_run: Vec<(SetupId, u32)>,
}

#[derive(Debug)]
pub struct Route {
    pub name: String,
    pub steps: Vec<Step>,
}

#[derive(Debug, PartialEq)]
pub struct Step {
    pub name: String,
    pub tool_group: ToolGroupId,
    pub unit: Unit,
    /// Processing time per unit.
    pub time: Dist,
    /// Cascading tool groups: time a unit occupies the first slot.
    pub cascade_interval: Option<Time>,
    /// Batch size in wafers (batch steps only).
    pub batch: Option<BatchSize>,
    pub setup: Option<StepSetup>,
    /// Probability that the step is processed (metrology sampling).
    pub sampling: f64,
    pub rework: Option<Rework>,
    /// Lot-to-lens dedication: the tool used here must also process this later step.
    pub dedicate_to: Option<StepIndex>,
    pub cqt: Option<Cqt>,
}

/// Unit the processing time applies to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unit {
    Lot,
    Wafer,
    Batch,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BatchSize {
    pub min: u32,
    pub max: u32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StepSetup {
    pub setup: SetupId,
    /// `WHEN = always`: performed even if the tool already has the setup.
    pub always: bool,
    /// Time given in the route; otherwise looked up in [`Dataset::setup_changes`].
    pub time: Option<Time>,
}

/// After processing, the whole lot returns to step `to` with this probability.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rework {
    pub probability: f64,
    pub to: StepIndex,
}

/// Critical queue time from the end of this step to the start of step `until`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cqt {
    pub until: StepIndex,
    pub limit: Time,
}

#[derive(Debug, PartialEq)]
pub struct Part {
    pub name: String,
    /// Part family; a production part and the engineering part derived from it share it.
    pub family: String,
    pub engineering: bool,
    pub route: RouteId,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transport {
    pub from: LocationId,
    pub to: LocationId,
    pub time: Dist,
}

/// `lots` lots released at `start + k·interval` for k = 0..`count`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ReleaseStream {
    pub part: PartId,
    pub priority: u32,
    pub wafers: u32,
    pub start: Time,
    pub interval: Time,
    pub count: u32,
    pub lots: u32,
    /// Due date minus release time.
    pub due_offset: Time,
    /// Super hot lots that reserve a tool at their next step.
    pub reserve: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LotRelease {
    pub part: PartId,
    pub priority: u32,
    pub wafers: u32,
    pub start: Time,
    pub due: Time,
    /// Super hot lot that reserves a tool at its next step.
    pub reserve: bool,
    /// Initial WIP: the step the lot waits at when the run starts; `None` starts the route.
    pub step: Option<StepIndex>,
}

#[derive(Debug, PartialEq)]
pub struct Period {
    pub name: String,
    pub start: Time,
    pub report: bool,
    /// Statistics restart at the end of this period.
    pub reset: bool,
}
