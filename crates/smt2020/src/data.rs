//! Static factory model of one SMT2020 dataset, validated and simulation-ready: names resolved to
//! indices, durations in ms, dates relative to the simulation start. Dataset files hold it in
//! postcard form after a magic and a format version.

use std::fmt;

use des_core::Time;
use serde::{Deserialize, Serialize};

use crate::layout::Layout;

/// Start of every dataset file, followed by [`FORMAT_VERSION`] (u32, little endian) and the
/// postcard-encoded [`Dataset`].
const MAGIC: &[u8; 8] = b"SMT2020\0";

/// Layout version of dataset files. Bumped whenever a serialized type of this module changes,
/// so files of another layout are rejected instead of misread.
pub const FORMAT_VERSION: u32 = 2;

impl Dataset {
    /// Encodes the dataset as a dataset file.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = MAGIC.to_vec();
        bytes.extend(FORMAT_VERSION.to_le_bytes());
        postcard::to_extend(self, bytes).expect("datasets encode to memory")
    }

    /// The CQT segments in route and step order, as (route, entrance step, its CQT): their
    /// position is the segment index of the dataset info.
    pub fn segments(&self) -> impl Iterator<Item = (RouteId, StepIndex, Cqt)> + '_ {
        self.routes.iter().enumerate().flat_map(|(route, spec)| {
            spec.steps
                .iter()
                .enumerate()
                .filter_map(move |(step, spec)| spec.cqt.map(|cqt| (route, step, cqt)))
        })
    }

    /// Decodes a dataset file of this [`FORMAT_VERSION`].
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, FormatError> {
        let error = |message: String| FormatError(message);
        let rest = bytes
            .strip_prefix(MAGIC)
            .ok_or_else(|| error("not an SMT2020 dataset file".into()))?;
        let (version, payload) = rest
            .split_first_chunk::<4>()
            .ok_or_else(|| error("truncated dataset file".into()))?;
        let version = u32::from_le_bytes(*version);
        if version != FORMAT_VERSION {
            return Err(error(format!(
                "dataset file format {version}, this build reads {FORMAT_VERSION}: convert the \
                 dataset again"
            )));
        }
        match postcard::take_from_bytes(payload) {
            Ok((dataset, [])) => Ok(dataset),
            Ok(_) => Err(error("trailing bytes after the dataset".into())),
            Err(cause) => Err(error(format!("corrupt dataset file: {cause}"))),
        }
    }
}

/// A byte string that is not a dataset file of this format version.
#[derive(Debug)]
pub struct FormatError(String);

impl fmt::Display for FormatError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for FormatError {}

pub type AreaId = usize;
pub type LocationId = usize;
pub type ToolGroupId = usize;
pub type SetupId = usize;
pub type SetupGroupId = usize;
pub type RouteId = usize;
pub type PartId = usize;
/// Position of a step within its route.
pub type StepIndex = usize;

#[derive(Debug, PartialEq, Serialize, Deserialize)]
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
    /// AMHS layout (SMAT2022): with it, OHTs carry the lots between ports instead of the
    /// transport times.
    pub layout: Option<Layout>,
}

/// Duration distribution.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
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

#[derive(Debug, PartialEq, Serialize, Deserialize)]
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BatchCriterion {
    /// `crit_sameroutestep`
    SameRouteStep,
    /// `crit_samepartfam|crit_samestepname`
    SameFamilyStepName,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Rule {
    /// `rule_HotLotFIRST`: first-ranked lot; flagged super hot lots reserve their next tool.
    HotLotFirst,
    /// `rule_LSSU`: setup runs bounded below by the group's minimum run lengths.
    SetupRun(SetupGroupId),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
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
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Breakdown {
    /// Time to the first failure.
    pub first: Dist,
    pub ttf: Dist,
    pub ttr: Dist,
}

/// Scheduled downtime.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Pm {
    pub trigger: PmTrigger,
    pub duration: Dist,
}

/// PM start rule. The first PM of the k-th of N tools (k = 1..N) falls at `first`·k/N.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum PmTrigger {
    /// Every `interval` of calendar time, duration included (`mtbpm_by_cal`).
    Calendar { interval: Time, first: Time },
    /// Every `interval` wafers processed by the tool (`mtbpm_by_pieces`).
    Wafers { interval: u32, first: u32 },
}

/// Setup duration from `from` (any setup if `None`) to `to`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SetupChange {
    pub from: Option<SetupId>,
    pub to: SetupId,
    pub time: Dist,
}

/// After changing to one of these setups, a tool must process the given number of lots before
/// changing again.
#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct SetupGroup {
    pub name: String,
    pub min_run: Vec<(SetupId, u32)>,
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct Route {
    pub name: String,
    pub steps: Vec<Step>,
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
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
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Unit {
    Lot,
    Wafer,
    Batch,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BatchSize {
    pub min: u32,
    pub max: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct StepSetup {
    pub setup: SetupId,
    /// `WHEN = always`: performed even if the tool already has the setup.
    pub always: bool,
    /// Time given in the route; otherwise looked up in [`Dataset::setup_changes`].
    pub time: Option<Time>,
}

/// After processing, the whole lot returns to step `to` with this probability.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Rework {
    pub probability: f64,
    pub to: StepIndex,
}

/// Critical queue time from the end of this step to the start of step `until`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cqt {
    pub until: StepIndex,
    pub limit: Time,
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct Part {
    pub name: String,
    /// Part family; a production part and the engineering part derived from it share it.
    pub family: String,
    pub engineering: bool,
    pub route: RouteId,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Transport {
    pub from: LocationId,
    pub to: LocationId,
    pub time: Dist,
}

/// `lots` lots released at `start + k·interval` for k = 0..`count`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
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

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
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

#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct Period {
    pub name: String,
    pub start: Time,
    pub report: bool,
    /// Statistics restart at the end of this period.
    pub reset: bool,
}

/// Smallest dataset that runs: one tool group, one single-step route, a periodic plan and one
/// initial WIP lot.
#[cfg(test)]
pub(crate) fn tiny() -> Dataset {
    use des_core::{DAY, HOUR, MINUTE};
    let week = Dist::Exponential { mean: 7 * DAY };
    Dataset {
        areas: vec!["Etch".into()],
        locations: vec!["Fab".into()],
        tool_groups: vec![ToolGroup {
            name: "Etch_1".into(),
            area: 0,
            location: 0,
            tools: 2,
            load: MINUTE,
            unload: MINUTE,
            cascading: false,
            batching: None,
            rule: Rule::HotLotFirst,
            ranks: vec![Rank::Priority, Rank::Fifo],
            wake_least_setup: false,
            breakdowns: vec![Breakdown {
                first: week,
                ttf: week,
                ttr: Dist::Exponential { mean: HOUR },
            }],
            pms: vec![Pm {
                trigger: PmTrigger::Calendar {
                    interval: 30 * DAY,
                    first: DAY,
                },
                duration: Dist::Constant(HOUR),
            }],
        }],
        setups: vec!["S1".into()],
        setup_changes: vec![SetupChange {
            from: None,
            to: 0,
            time: Dist::Constant(10 * MINUTE),
        }],
        setup_groups: Vec::new(),
        routes: vec![Route {
            name: "R1".into(),
            steps: vec![Step {
                name: "1".into(),
                tool_group: 0,
                unit: Unit::Lot,
                time: Dist::Uniform {
                    mean: 10 * MINUTE,
                    half_width: 30_000,
                },
                cascade_interval: None,
                batch: None,
                setup: Some(StepSetup {
                    setup: 0,
                    always: false,
                    time: None,
                }),
                sampling: 1.0,
                rework: None,
                dedicate_to: None,
                cqt: None,
            }],
        }],
        parts: vec![Part {
            name: "part_1".into(),
            family: "product_1".into(),
            engineering: false,
            route: 0,
        }],
        transports: Vec::new(),
        streams: vec![ReleaseStream {
            part: 0,
            priority: 10,
            wafers: 25,
            start: 0,
            interval: HOUR,
            count: 10,
            lots: 1,
            due_offset: DAY,
            reserve: false,
        }],
        lots: vec![LotRelease {
            part: 0,
            priority: 20,
            wafers: 25,
            start: 0,
            due: DAY,
            reserve: false,
            step: Some(0),
        }],
        periods: vec![Period {
            name: "P".into(),
            start: 0,
            report: true,
            reset: true,
        }],
        layout: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dataset_files_round_trip() {
        let dataset = tiny();
        assert_eq!(Dataset::from_bytes(&dataset.to_bytes()).unwrap(), dataset);
    }

    /// The layout guard: a change of a serialized type changes these bytes. Then bump
    /// `FORMAT_VERSION` and update them.
    #[test]
    fn dataset_file_layout_is_version_2() {
        let hex: String = tiny()
            .to_bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        assert_eq!(
            hex,
            concat!(
                "534d5432303230000200000001044574636801034661620106457463685f31000002c0a907c0a907",
                "0000000200020001028090e4c004028090e4c0040280bab703010080a0f6a71380f0b2520080bab7",
                "030102533101000000809f490001025231010131000001809f49e0d4030000010000000000000000",
                "00f03f0000000106706172745f310970726f647563745f3100000001000a190080bab7030a0180f0",
                "b25200010014190080f0b252000100010150000101",
                "00",
            )
        );
    }

    #[test]
    fn foreign_files_are_rejected() {
        let bytes = tiny().to_bytes();
        let message = |bytes: &[u8]| Dataset::from_bytes(bytes).unwrap_err().to_string();
        assert_eq!(message(b"not a dataset"), "not an SMT2020 dataset file");
        assert_eq!(message(&bytes[..10]), "truncated dataset file");
        let mut other = bytes.clone();
        other[8] = 1;
        assert!(message(&other).starts_with("dataset file format 1, this build reads 2"));
        let mut longer = bytes.clone();
        longer.push(0);
        assert_eq!(message(&longer), "trailing bytes after the dataset");
        assert!(message(&bytes[..bytes.len() - 1]).starts_with("corrupt dataset file"));
    }
}
