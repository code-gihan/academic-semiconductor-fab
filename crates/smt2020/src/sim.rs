//! SMT2020 fab simulation (README model specification): lots released by the plan flow through
//! their routes on the dataset's tools; a run ends when every released lot is complete.
//!
//! A [`Simulation`] runs one [`Config`] on a dataset from time 0, in steps: up to a given time,
//! or until an observer pauses it; the fab's state can be read in between. [`Config`],
//! [`Progress`], the status types and [`Results`] are also the serialized form of the CLI and the
//! bindings: times are ms (fractional times round to whole ms), omitted configuration fields take
//! their defaults.

mod amhs;
mod code;
mod dispatch;
mod fab;
mod logistics;
mod plan;
mod record;
mod replay;
mod routes;
mod stats;
mod status;
mod strategy;
mod tool;

use std::collections::BTreeMap;
use std::fmt;
use std::ops::ControlFlow;
use std::sync::Arc;

use des_core::{DAY, Outcome, Time};
use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize};

use crate::data::{Dataset, Rank};
use fab::Fab;
use strategy::Strategy;

pub use amhs::{
    Activity, AmhsConfig, AmhsReport, BAY_DISTANCE_CLASSES, BayDistance, Dispatch, Moves,
    VehicleStatus, VehicleTimes,
};
pub use code::{Admit, BatchView, Code, CqtView, GroupCount, Hooks, LotView, SegmentView, Start};
pub use fab::LotState;
pub use record::{EventFilter, EventKind, Events, Recording, Records, ToolGroupDays, Violations};
pub use replay::{FoupFrames, FoupPlace, Frame, Player, Replay, ReplayWindow, VehicleFrames};
pub use stats::{
    CqtReport, CqtSegmentReport, CqtStepReport, CqtTimes, DayReport, FLOW_FACTOR_PERCENTILES,
    FlowFactors, LotKind, LotReport, PeriodReport, Results, StateTimes, ToolGroupReport,
};
pub use status::{
    AmhsStatus, FoupCount, FoupStatus, LotStatus, SegmentLot, SegmentStatus, ToolGroupStatus,
    ToolStatus,
};
pub use strategy::MAX_CRITERIA;
pub(crate) use strategy::STEPPERS;
pub use tool::ToolState;

const DEFAULT_SEED: u64 = 1;
const DEFAULT_LOAD: f64 = 1.0;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Lots planned to start before the horizon are released; reporting periods end at the
    /// horizon, after which the run continues until every released lot is complete.
    #[serde(deserialize_with = "deserialize_time")]
    pub horizon: Time,
    /// Reporting periods WarmUp [0, warm_up) and Period_1 [warm_up, horizon) instead of the
    /// dataset's (a one-year warm-up).
    #[serde(default, deserialize_with = "deserialize_optional_time")]
    pub warm_up: Option<Time>,
    /// Random number streams per (seed, replication, purpose): runs that differ only in their
    /// strategy share them (common random numbers).
    #[serde(default = "default_seed")]
    pub seed: u64,
    #[serde(default)]
    pub replication: u32,
    /// Release rate factor (operating curves): release times are divided by it, due-date offsets
    /// kept. 1 is the dataset plan.
    #[serde(default = "default_load")]
    pub load: f64,
    /// Super hot lots reserve a tool at their next step even where the dataset says HOTLOT = no.
    #[serde(default)]
    pub reserve_super_hot: bool,
    /// Ranked before FIFO/CR at the tool groups without their own [`ranking`](Self::ranking).
    #[serde(default)]
    pub queue_time: QueueTimeRule,
    /// QTS flow factors per route and step, as [`Results::step_flow_factors`] of an earlier run.
    /// Without them, the [`first_pass`](Self::first_pass) measures them.
    #[serde(default)]
    pub flow_factors: Option<Vec<Vec<Option<f64>>>>,
    /// Lot ranking per tool group name: 1 to [`MAX_CRITERIA`] distinct criteria, most
    /// significant first, then release order. Other groups rank by the dataset's criteria with
    /// the [`queue_time`](Self::queue_time) rule before FIFO/CR.
    #[serde(default)]
    pub ranking: BTreeMap<String, Vec<Criterion>>,
    /// A batch below its minimum size also starts once one of its lots has at most this much
    /// queue-time slack left ([`Criterion::QtWithin`]).
    #[serde(default, deserialize_with = "deserialize_optional_time")]
    pub batch_start_within: Option<Time>,
    #[serde(default)]
    pub stopping: Option<Stopping>,
    #[serde(default)]
    pub engineering: EngineeringRule,
    /// AMHS settings of a dataset with a layout, which always runs its AMHS (the defaults
    /// without them); none for a dataset without one.
    #[serde(default)]
    pub amhs: Option<AmhsConfig>,
}

impl Config {
    /// The dataset's own rules over `horizon`: seed 1, replication 0, the plan's load.
    pub fn new(horizon: Time) -> Self {
        Self {
            horizon,
            warm_up: None,
            seed: DEFAULT_SEED,
            replication: 0,
            load: DEFAULT_LOAD,
            reserve_super_hot: false,
            queue_time: QueueTimeRule::None,
            flow_factors: None,
            ranking: BTreeMap::new(),
            batch_start_within: None,
            stopping: None,
            engineering: EngineeringRule::Base,
            amhs: None,
        }
    }

    /// The strategy code's priority ranks somewhere: as the queue-time rule or as a criterion.
    fn uses_code(&self) -> bool {
        self.queue_time == QueueTimeRule::Code
            || self
                .ranking
                .values()
                .flatten()
                .any(|&c| c == Criterion::Code)
    }

    /// QTS ranks somewhere: as the queue-time rule or as a criterion.
    fn uses_qts(&self) -> bool {
        self.queue_time == QueueTimeRule::Qts
            || self
                .ranking
                .values()
                .flatten()
                .any(|&c| c == Criterion::Qts)
    }

    /// The pass measuring the QTS flow factors that a QTS run without them needs first: this
    /// configuration without its queue-time controls (rule, criteria, batch starts, stopping)
    /// and without strategy code; a ranking left empty falls back to the dataset's (\[P2\] §3.1,
    /// assumed).
    fn first_pass(&self) -> Option<Self> {
        if self.flow_factors.is_some() || !self.uses_qts() {
            return None;
        }
        let ranking = self
            .ranking
            .iter()
            .filter_map(|(group, criteria)| {
                let kept: Vec<Criterion> = criteria
                    .iter()
                    .copied()
                    .filter(|&criterion| !criterion.queue_time() && criterion != Criterion::Code)
                    .collect();
                (!kept.is_empty()).then(|| (group.clone(), kept))
            })
            .collect();
        Some(Self {
            queue_time: QueueTimeRule::None,
            ranking,
            batch_start_within: None,
            stopping: None,
            ..self.clone()
        })
    }
}

fn default_seed() -> u64 {
    DEFAULT_SEED
}

fn default_load() -> f64 {
    DEFAULT_LOAD
}

/// A time from a number of ms, rounded to whole ms as the loader rounds input times: the times
/// of the serialized form and of the bindings.
pub fn time(ms: f64) -> Result<Time, Error> {
    if ms.is_finite() && ms.abs() < Time::MAX as f64 {
        Ok(ms.round() as Time)
    } else {
        Err(Error(format!("{ms} is not a time in ms")))
    }
}

fn deserialize_time<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Time, D::Error> {
    time(f64::deserialize(deserializer)?).map_err(D::Error::custom)
}

fn deserialize_optional_time<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Time>, D::Error> {
    Option::<f64>::deserialize(deserializer)?
        .map(time)
        .transpose()
        .map_err(D::Error::custom)
}

/// Critical queue time dispatching of \[P2\] §3.1, or the strategy code's in its place: ranked
/// after least setup and before FIFO/CR.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QueueTimeRule {
    #[default]
    None,
    /// Queue time critical ratio, eq. (1).
    Qtcr,
    /// Queue time slack, eq. (2)–(6), with [`Config::flow_factors`].
    Qts,
    /// The strategy code's priority ([`Code::priority`]).
    Code,
}

/// A lot ranking criterion: the lot with the smaller value goes first. The queue-time criteria
/// rank the lots outside CQT segments last.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Criterion {
    /// Higher dispatching priority (`rank_HP`).
    Priority,
    /// Least setup time needed on the tool (`rank_RSETUP`).
    LeastSetup,
    /// Earliest arrival in the queue (`rank_FIFO`).
    Fifo,
    /// Smallest critical ratio: time to the due date over the expected remaining work
    /// (`rank_CR`).
    CriticalRatio,
    /// Earliest due date.
    DueDate,
    /// Shortest expected duration of the step.
    ShortestStep,
    /// Least expected remaining work.
    LeastRemaining,
    /// Smallest queue time critical ratio (\[P2\] eq. (1)).
    Qtcr,
    /// Earliest latest start of the step (\[P2\] QTS, eq. (2)–(6)).
    Qts,
    /// Earliest end of the CQT segment (start + limit).
    QtDeadline,
    /// Lots with at most this much queue-time slack first, the others alike. Slack = segment end
    /// − now − expected work before the exit step starts.
    QtWithin(#[serde(deserialize_with = "deserialize_time")] Time),
    /// The strategy code's priority, given on the lot's arrival in the queue
    /// ([`Code::priority`]).
    Code,
}

impl Criterion {
    /// Serialized name.
    pub fn name(self) -> &'static str {
        match self {
            Self::Priority => "priority",
            Self::LeastSetup => "least_setup",
            Self::Fifo => "fifo",
            Self::CriticalRatio => "critical_ratio",
            Self::DueDate => "due_date",
            Self::ShortestStep => "shortest_step",
            Self::LeastRemaining => "least_remaining",
            Self::Qtcr => "qtcr",
            Self::Qts => "qts",
            Self::QtDeadline => "qt_deadline",
            Self::QtWithin(_) => "qt_within",
            Self::Code => "code",
        }
    }

    /// Ranks by CQT segments.
    pub fn queue_time(self) -> bool {
        matches!(
            self,
            Self::Qtcr | Self::Qts | Self::QtDeadline | Self::QtWithin(_)
        )
    }
}

impl From<Rank> for Criterion {
    fn from(rank: Rank) -> Self {
        match rank {
            Rank::Priority => Self::Priority,
            Rank::LeastSetup => Self::LeastSetup,
            Rank::Fifo => Self::Fifo,
            Rank::CriticalRatio => Self::CriticalRatio,
        }
    }
}

/// Kanban-type stopping at CQT segment entrances (\[P2\] §3.2). Limits low enough to hold the lots
/// a waiting batch or setup run needs block those lots for good, and the run fails.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Stopping {
    /// Limits per tool group name.
    #[serde(default)]
    pub limits: BTreeMap<String, Limits>,
    /// Limits of all other tool groups.
    #[serde(default)]
    pub default: Limits,
}

/// Stopping limits of a tool group, both positive. The default is \[P2\] Table 3's 1,000/1,000.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Limits {
    /// Constrained lots in front of the group (queued or processing).
    pub front: u32,
    /// Those plus the constrained lots still upstream of the group in their segments.
    pub total: u32,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            front: 1_000,
            total: 1_000,
        }
    }
}

/// Production and engineering lot dispatching of \[P1\] §V. CAtE and CoT apply to the steppers
/// LithoTrack_FE_95 and LithoTrack_FE_115 only.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum EngineeringRule {
    #[default]
    Base,
    /// Priorities EHL 25, PHL 20, ERL 15, PRL 10.
    EngineeringFirst,
    /// Alternating production and engineering intervals, each preferring its lots.
    Cate {
        #[serde(deserialize_with = "deserialize_time")]
        production: Time,
        #[serde(deserialize_with = "deserialize_time")]
        engineering: Time,
    },
    /// Once `trigger` engineering lots wait, that many go first.
    Cot { trigger: u32 },
}

/// Where a simulation stands: shown to observers once per simulated day, and on request.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Progress {
    /// Pass of the run, from 0: QTS without flow factors measures them in a first pass.
    pub pass: u32,
    pub passes: u32,
    /// Simulation time of the pass; after the horizon the lots left drain.
    pub now: Time,
    pub horizon: Time,
    /// Lots released and completed so far, and lots in the fab.
    pub released: u64,
    pub completed: u64,
    pub wip: u64,
    /// CQT segment completions so far, and those over the limit.
    pub cqt_completed: u64,
    pub cqt_violated: u64,
    /// Every lot of the configured run is complete: the results are final.
    pub finished: bool,
}

#[derive(Clone, Debug)]
pub struct Error(pub(crate) String);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

/// Every released lot must be complete this long after the horizon; otherwise the run fails.
const DRAIN_LIMIT: Time = 365 * DAY;

/// One run of a [`Config`] on a dataset, from time 0 until every released lot is complete.
///
/// [`run`](Self::run) advances it up to a time or to the end; [`run_observed`](Self::run_observed)
/// also shows its [`Progress`] once per simulated day and pauses where the observer breaks off.
/// Between runs, [`progress`](Self::progress), [`lots`](Self::lots), [`tools`](Self::tools),
/// [`tool_groups`](Self::tool_groups) and [`segments`](Self::segments) read the fab's state, and
/// [`reset`](Self::reset) starts over;
/// the finished run has its [`results`](Self::results). Pausing leaves the results unchanged.
///
/// QTS without flow factors runs in two passes: the [`Config::first_pass`] measures the flow
/// factors, then the configured run takes them. Only the configured run is recorded, and only it
/// asks the strategy [`Code`].
pub struct Simulation {
    data: Arc<Dataset>,
    config: Config,
    recording: Recording,
    pass: u32,
    passes: u32,
    engine: des_core::Simulation<Fab>,
    /// QTS flow factors the first pass measured.
    measured: Option<Vec<Vec<Option<f64>>>>,
    /// Failure of a run that passed its deadline or whose code failed; every later run reports
    /// it.
    failure: Option<Error>,
    /// The strategy code while a first pass runs; the configured run's fab holds it then.
    code: Option<Box<dyn Code>>,
    hooks: Hooks,
}

impl Simulation {
    /// The simulation of `config` on `data` at time 0.
    pub fn new(data: Arc<Dataset>, config: Config) -> Result<Self, Error> {
        Self::with_recording(data, config, Recording::default())
    }

    /// The simulation of `config` on `data` at time 0, recording what `recording` asks
    /// ([`records`](Self::records)). Recording leaves the results unchanged.
    pub fn with_recording(
        data: Arc<Dataset>,
        config: Config,
        recording: Recording,
    ) -> Result<Self, Error> {
        Self::build(data, config, recording, &mut None)
    }

    /// [`with_recording`](Self::with_recording) with strategy `code` deciding what its hooks
    /// decide. A configuration that ranks by code needs its priority.
    pub fn with_code(
        data: Arc<Dataset>,
        config: Config,
        recording: Recording,
        code: Box<dyn Code>,
    ) -> Result<Self, Error> {
        Self::build(data, config, recording, &mut Some(code))
    }

    /// The simulation at time 0; `code` goes into it unless this fails.
    fn build(
        data: Arc<Dataset>,
        config: Config,
        recording: Recording,
        code: &mut Option<Box<dyn Code>>,
    ) -> Result<Self, Error> {
        let hooks = code.as_ref().map_or(Hooks::default(), |code| code.hooks());
        if code.is_some() && hooks == Hooks::default() {
            return Err(Error("the strategy code defines no hook".into()));
        }
        if code.is_some() && config.uses_code() && !hooks.priority {
            return Err(Error(
                "the configuration ranks by code, but the strategy code has no priority".into(),
            ));
        }
        let first = config.first_pass();
        let fab = match &first {
            Some(first) => {
                // The configured run's rules and recording are checked now, not after the first
                // pass.
                let unmeasured: Vec<Vec<Option<f64>>> = data
                    .routes
                    .iter()
                    .map(|route| vec![None; route.steps.len()])
                    .collect();
                Strategy::new(&data, &config, Some(&unmeasured))?;
                record::Recorder::new(&data, &recording)?;
                Fab::new(
                    Arc::clone(&data),
                    first,
                    None,
                    &Recording::default(),
                    &mut None,
                )?
            }
            None => Fab::new(
                Arc::clone(&data),
                &config,
                config.flow_factors.as_deref(),
                &recording,
                code,
            )?,
        };
        Ok(Self {
            data,
            config,
            recording,
            pass: 0,
            passes: if first.is_some() { 2 } else { 1 },
            engine: des_core::Simulation::new(fab),
            measured: None,
            failure: None,
            code: code.take(),
            hooks,
        })
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    pub fn recording(&self) -> &Recording {
        &self.recording
    }

    /// Starts over at time 0 with `config`, the same recording and the same strategy code; on
    /// error the simulation stays as it was.
    pub fn reset(&mut self, config: Config) -> Result<(), Error> {
        // The code waits for the configured run, or that run's fab holds it.
        let waiting = self.code.is_some();
        let mut code = if waiting {
            self.code.take()
        } else {
            self.engine.model_mut().code.take()
        };
        match Self::build(
            Arc::clone(&self.data),
            config,
            self.recording.clone(),
            &mut code,
        ) {
            Ok(simulation) => {
                *self = simulation;
                Ok(())
            }
            Err(error) => {
                if waiting {
                    self.code = code;
                } else {
                    self.engine.model_mut().code = code;
                }
                Err(error)
            }
        }
    }

    /// The QTS flow factors of the configured run: given, or measured by the first pass once it
    /// ended; none without QTS. A configuration with them runs the same in one pass (replays).
    pub fn flow_factors(&self) -> Option<&[Vec<Option<f64>>]> {
        self.config
            .flow_factors
            .as_deref()
            .or(self.measured.as_deref())
    }

    /// The tables recorded so far (empty during a first pass).
    pub fn records(&self) -> &Records {
        self.engine.model().records()
    }

    /// Runs up to `until` (events at it included) or, without it, to the end, and returns where
    /// the run stands. `until` is a time of the configured run: a first pass always completes.
    pub fn run(&mut self, until: Option<Time>) -> Result<Progress, Error> {
        self.run_observed(until, |_| ControlFlow::Continue(()))
    }

    /// [`run`](Self::run) shown to `observe` once per simulated day; breaking off pauses the run
    /// there.
    pub fn run_observed(
        &mut self,
        until: Option<Time>,
        mut observe: impl FnMut(&Progress) -> ControlFlow<()>,
    ) -> Result<Progress, Error> {
        if self.config.uses_code() && !self.hooks.priority {
            return Err(Error(
                "the configuration ranks by code, but no strategy code was given".into(),
            ));
        }
        loop {
            if let Some(failure) = &self.failure {
                return Err(failure.clone());
            }
            let last = self.pass + 1 == self.passes;
            if last && self.engine.model().finished() {
                return Ok(self.progress());
            }
            let end = match until {
                Some(until) if last => until,
                _ => Time::MAX,
            };
            let (pass, passes, horizon) = (self.pass, self.passes, self.config.horizon);
            // The model stops the run when its last lot completes, or at its deadline.
            let outcome = self.engine.run_observed(end, DAY, |fab, now| {
                observe(&Progress::of(fab, now, pass, passes, horizon))
            });
            let fab = self.engine.model();
            let failure = fab
                .code_failure()
                .map(|failure| format!("strategy code: {failure}"))
                .or_else(|| fab.amhs_failure().map(str::to_owned));
            if let Some(failure) = failure {
                let failure = Error(failure);
                self.failure = Some(failure.clone());
                return Err(failure);
            }
            match outcome {
                Outcome::Reached | Outcome::Interrupted => return Ok(self.progress()),
                _ if !fab.finished() => {
                    let failure = Error(format!(
                        "{} lots unfinished a year after the horizon",
                        fab.wip()
                    ));
                    self.failure = Some(failure.clone());
                    return Err(failure);
                }
                _ if last => return Ok(self.progress()),
                _ => {
                    // The first pass measured the configured run's flow factors.
                    let measured = fab.step_flow_factors().to_vec();
                    let fab = Fab::new(
                        Arc::clone(&self.data),
                        &self.config,
                        Some(&measured),
                        &self.recording,
                        &mut self.code,
                    )?;
                    self.engine = des_core::Simulation::new(fab);
                    self.measured = Some(measured);
                    self.pass += 1;
                }
            }
        }
    }

    pub fn progress(&self) -> Progress {
        Progress::of(
            self.engine.model(),
            self.engine.now(),
            self.pass,
            self.passes,
            self.config.horizon,
        )
    }

    /// The lots in the fab, by id.
    pub fn lots(&self) -> Vec<LotStatus> {
        self.engine.model().lot_statuses()
    }

    /// Every tool, by id.
    pub fn tools(&self) -> Vec<ToolStatus> {
        self.engine.model().tool_statuses(self.engine.now())
    }

    /// Every tool group, in dataset order.
    pub fn tool_groups(&self) -> Vec<ToolGroupStatus> {
        self.engine.model().tool_group_statuses(self.engine.now())
    }

    /// Every CQT segment, in [`DatasetInfo::segments`](crate::DatasetInfo::segments) order.
    pub fn segments(&self) -> Vec<SegmentStatus> {
        self.engine.model().segment_statuses(self.engine.now())
    }

    /// The AMHS: its vehicles, FOUPs and tool states; none without a layout.
    pub fn amhs(&self) -> Option<AmhsStatus> {
        self.engine.model().amhs_status(self.engine.now())
    }

    /// The AMHS replay recorded so far ([`Recording::replay`]), up to its window's end or the
    /// simulation time; none without one or before its window began.
    pub fn replay(&self) -> Option<Replay> {
        self.engine.model().replay(self.engine.now())
    }

    /// Results of the finished run.
    pub fn results(&self) -> Result<Results, Error> {
        if !self.progress().finished {
            return Err(Error("the run has not finished".into()));
        }
        Ok(self.engine.model().results(self.engine.events_processed()))
    }
}

impl Progress {
    fn of(fab: &Fab, now: Time, pass: u32, passes: u32, horizon: Time) -> Self {
        Self {
            pass,
            passes,
            now,
            horizon,
            released: fab.released(),
            completed: fab.completed(),
            wip: fab.wip() as u64,
            cqt_completed: fab.cqt_total().completed,
            cqt_violated: fab.cqt_total().violated,
            finished: pass + 1 == passes && fab.finished(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::tiny;
    use des_core::{HOUR, MINUTE};
    use std::ops::ControlFlow::{Break, Continue};

    fn parse(json: &str) -> Result<Config, serde_json::Error> {
        serde_json::from_str(json)
    }

    fn simulation(config: Config) -> Simulation {
        Simulation::new(Arc::new(tiny()), config).unwrap_or_else(|error| panic!("{error}"))
    }

    fn error(config: Config) -> String {
        match Simulation::new(Arc::new(tiny()), config) {
            Ok(_) => panic!("configuration accepted"),
            Err(error) => error.to_string(),
        }
    }

    #[test]
    fn config_fields_default_and_round_ms() {
        assert_eq!(
            parse(r#"{"horizon": 63072000000}"#).unwrap(),
            Config::new(730 * DAY)
        );
        assert_eq!(parse(r#"{"horizon": 1.5}"#).unwrap().horizon, 2);
        assert_eq!(time(-1.5).unwrap(), -2);
        assert!(time(f64::NAN).is_err());
        let config = parse(
            r#"{"horizon": 1e9, "warm_up": 1e8, "seed": 7, "replication": 3, "load": 0.9,
                "reserve_super_hot": true, "queue_time": "qtcr",
                "ranking": {"Etch_1": [{"qt_within": 3600000.4}, "priority", "fifo"]},
                "batch_start_within": 1800000,
                "stopping": {"limits": {"LithoTrack_FE_95": {"front": 50, "total": 85}}},
                "engineering": {"cate": {"production": 544320000.0000001, "engineering": 6.048e7}},
                "amhs": {"vehicles": 300, "dispatch": "bay", "look_ahead": 1, "hoist": 8000.4,
                         "max_speed": 3000}}"#,
        )
        .unwrap();
        assert_eq!(
            config,
            Config {
                horizon: 1_000_000_000,
                warm_up: Some(100_000_000),
                seed: 7,
                replication: 3,
                load: 0.9,
                reserve_super_hot: true,
                queue_time: QueueTimeRule::Qtcr,
                flow_factors: None,
                ranking: BTreeMap::from([(
                    "Etch_1".into(),
                    vec![
                        Criterion::QtWithin(HOUR),
                        Criterion::Priority,
                        Criterion::Fifo
                    ]
                )]),
                batch_start_within: Some(30 * 60_000),
                stopping: Some(Stopping {
                    limits: BTreeMap::from([(
                        "LithoTrack_FE_95".into(),
                        Limits {
                            front: 50,
                            total: 85
                        }
                    )]),
                    default: Limits::default(),
                }),
                engineering: EngineeringRule::Cate {
                    production: 544_320_000,
                    engineering: 60_480_000,
                },
                amhs: Some(AmhsConfig {
                    vehicles: Some(300),
                    dispatch: Dispatch::Bay,
                    look_ahead: Some(1),
                    hoist: 8_000,
                    max_speed: Some(3_000.0),
                    ..AmhsConfig::default()
                }),
            }
        );
        // The serialized form reads back as it is.
        assert_eq!(
            parse(&serde_json::to_string(&config).unwrap()).unwrap(),
            config
        );
        assert_eq!(
            serde_json::to_value(&config.ranking).unwrap(),
            serde_json::json!({"Etch_1": [{"qt_within": 3_600_000}, "priority", "fifo"]})
        );
        assert_eq!(
            parse(r#"{"horizon": 1, "engineering": "engineering_first", "queue_time": "qts"}"#)
                .unwrap()
                .engineering,
            EngineeringRule::EngineeringFirst
        );
        // Misspelled fields and unknown rules are errors, not defaults.
        assert!(parse(r#"{"horizon": 1, "sead": 2}"#).is_err());
        assert!(parse(r#"{"horizon": 1, "queue_time": "qtx"}"#).is_err());
        assert!(parse(r#"{"horizon": 1, "ranking": {"Etch_1": ["fifoo"]}}"#).is_err());
        assert!(parse(r#"{"seed": 2}"#).is_err());
    }

    #[test]
    fn runs_are_observed_daily_until_the_last_lot_completes() {
        let mut seen = Vec::new();
        let mut sim = simulation(Config::new(3 * DAY));
        let progress = sim
            .run_observed(None, |progress| {
                seen.push(*progress);
                Continue(())
            })
            .unwrap();
        assert_eq!(
            progress,
            Progress {
                pass: 0,
                passes: 1,
                now: 3 * DAY,
                horizon: 3 * DAY,
                released: 11,
                completed: 11,
                wip: 0,
                cqt_completed: 0,
                cqt_violated: 0,
                finished: true,
            }
        );
        let observed: Vec<_> = seen
            .iter()
            .map(|p| (p.now, p.released, p.wip, p.finished))
            .collect();
        assert_eq!(observed, [(DAY, 11, 0, false), (2 * DAY, 11, 0, false)]);
        let results = sim.results().unwrap();
        assert_eq!((results.released, results.end), (11, 3 * DAY));
        // A finished run stays as it is.
        assert_eq!(sim.run(None).unwrap(), progress);
    }

    #[test]
    fn a_paused_run_shows_the_fab_and_resumes_to_the_same_results() {
        let config = Config::new(3 * DAY);
        let mut straight = simulation(config.clone());
        let finished = straight.run(None).unwrap();
        let mut sim = simulation(config);
        assert_eq!(
            sim.results().unwrap_err().to_string(),
            "the run has not finished"
        );
        // At time 0 the first stream lot and the initial WIP lot start their setups.
        let progress = sim.run(Some(0)).unwrap();
        assert_eq!((progress.now, progress.released, progress.wip), (0, 2, 2));
        let lots: Vec<_> = sim
            .lots()
            .into_iter()
            .map(|lot| (lot.id, lot.kind, lot.state, lot.tool, lot.tool_group))
            .collect();
        assert_eq!(
            lots,
            [
                (
                    0,
                    LotKind::Prl,
                    LotState::Processing,
                    Some(0),
                    "Etch_1".into()
                ),
                (
                    1,
                    LotKind::Phl,
                    LotState::Processing,
                    Some(1),
                    "Etch_1".into()
                ),
            ]
        );
        let tools: Vec<_> = sim
            .tools()
            .into_iter()
            .map(|tool| (tool.state, tool.setup, tool.lots))
            .collect();
        assert_eq!(
            tools,
            [
                (ToolState::Setup, Some("S1".into()), vec![0]),
                (ToolState::Setup, Some("S1".into()), vec![1]),
            ]
        );
        let group = &sim.tool_groups()[0];
        assert_eq!(
            (group.tools, group.queue, group.setup, group.idle),
            (2, 0, 2, 0)
        );
        // Paused by the observer, then at an end time between observations.
        let paused = sim
            .run_observed(None, |progress| {
                if progress.now == DAY {
                    Break(())
                } else {
                    Continue(())
                }
            })
            .unwrap();
        assert_eq!((paused.now, paused.finished), (DAY, false));
        assert_eq!(sim.run(Some(DAY + HOUR)).unwrap().now, DAY + HOUR);
        assert_eq!(sim.run(None).unwrap(), finished);
        assert_eq!(sim.results().unwrap(), straight.results().unwrap());
    }

    #[test]
    fn resets_start_over() {
        let mut sim = simulation(Config::new(3 * DAY));
        sim.run(None).unwrap();
        let results = sim.results().unwrap();
        // A rejected configuration leaves the simulation as it was.
        assert!(sim.reset(Config::new(0)).is_err());
        assert!(sim.progress().finished);
        sim.reset(sim.config().clone()).unwrap();
        assert_eq!(
            sim.progress(),
            Progress {
                pass: 0,
                passes: 1,
                now: 0,
                horizon: 3 * DAY,
                released: 0,
                completed: 0,
                wip: 0,
                cqt_completed: 0,
                cqt_violated: 0,
                finished: false,
            }
        );
        sim.run(None).unwrap();
        assert_eq!(sim.results().unwrap(), results);
        sim.reset(Config {
            seed: 2,
            ..Config::new(3 * DAY)
        })
        .unwrap();
        sim.run(None).unwrap();
        assert_ne!(sim.results().unwrap(), results);
    }

    #[test]
    fn qts_without_flow_factors_measures_them_in_a_first_pass() {
        let config = Config {
            queue_time: QueueTimeRule::Qts,
            ..Config::new(2 * DAY)
        };
        let mut sim = simulation(config.clone());
        let mut seen = Vec::new();
        let progress = sim
            .run_observed(Some(DAY), |progress| {
                seen.push((progress.pass, progress.passes, progress.now));
                Continue(())
            })
            .unwrap();
        // The first pass runs to its end; the end time is one of the configured run.
        assert_eq!(seen, [(0, 2, DAY), (1, 2, DAY)]);
        assert_eq!((progress.pass, progress.now), (1, DAY));
        sim.run(None).unwrap();
        // The configured run takes the first pass's flow factors.
        let mut first = simulation(Config::new(2 * DAY));
        first.run(None).unwrap();
        let mut given = simulation(Config {
            flow_factors: Some(first.results().unwrap().step_flow_factors),
            ..config
        });
        given.run(None).unwrap();
        assert_eq!(sim.results().unwrap(), given.results().unwrap());
    }

    #[test]
    fn a_qts_criterion_also_measures_flow_factors_first() {
        let ranking = |criteria: Vec<Criterion>| BTreeMap::from([("Etch_1".into(), criteria)]);
        let config = Config {
            ranking: ranking(vec![Criterion::Qts, Criterion::Fifo]),
            ..Config::new(2 * DAY)
        };
        let mut sim = simulation(config.clone());
        assert_eq!(sim.progress().passes, 2);
        sim.run(None).unwrap();
        // The first pass ranks without the queue-time criteria.
        let mut first = simulation(Config {
            ranking: ranking(vec![Criterion::Fifo]),
            ..Config::new(2 * DAY)
        });
        first.run(None).unwrap();
        let mut given = simulation(Config {
            flow_factors: Some(first.results().unwrap().step_flow_factors),
            ..config
        });
        assert_eq!(given.progress().passes, 1);
        given.run(None).unwrap();
        assert_eq!(sim.results().unwrap(), given.results().unwrap());
    }

    #[test]
    fn a_warm_up_replaces_the_reporting_periods() {
        let mut sim = simulation(Config {
            warm_up: Some(DAY),
            ..Config::new(3 * DAY)
        });
        sim.run(None).unwrap();
        let periods: Vec<_> = sim
            .results()
            .unwrap()
            .periods
            .iter()
            .map(|period| (period.name.clone(), period.start, period.end))
            .collect();
        assert_eq!(
            periods,
            [
                ("WarmUp".into(), 0, DAY),
                ("Period_1".into(), DAY, 3 * DAY),
                ("Drain".into(), 3 * DAY, 3 * DAY)
            ]
        );
    }

    /// [`tiny`] with a batch furnace after the etch step and a 2 h CQT from etch to furnace:
    /// lots of 25 wafers every 10 h, batches of 50 to 100 wafers.
    fn tiny_batch() -> Dataset {
        use crate::data::{
            BatchCriterion, BatchSize, Cqt, Dist, Rank, Rule, Step, ToolGroup, Unit,
        };
        let mut data = tiny();
        data.lots.clear();
        data.streams[0].interval = 10 * HOUR;
        data.streams[0].count = 3;
        data.tool_groups[0].breakdowns.clear();
        data.tool_groups[0].pms.clear();
        data.tool_groups.push(ToolGroup {
            name: "Furnace_1".into(),
            area: 0,
            location: 0,
            tools: 1,
            load: 0,
            unload: 0,
            cascading: false,
            batching: Some(BatchCriterion::SameRouteStep),
            rule: Rule::HotLotFirst,
            ranks: vec![Rank::Fifo],
            wake_least_setup: false,
            breakdowns: Vec::new(),
            pms: Vec::new(),
        });
        let etch = &mut data.routes[0].steps[0];
        etch.setup = None;
        etch.time = Dist::Constant(10 * MINUTE);
        etch.cqt = Some(Cqt {
            until: 1,
            limit: 2 * HOUR,
        });
        data.routes[0].steps.push(Step {
            name: "2".into(),
            tool_group: 1,
            unit: Unit::Batch,
            time: Dist::Constant(HOUR),
            cascade_interval: None,
            batch: Some(BatchSize { min: 50, max: 100 }),
            setup: None,
            sampling: 1.0,
            rework: None,
            dedicate_to: None,
            cqt: None,
        });
        data
    }

    #[test]
    fn batches_start_below_their_minimum_on_queue_time_slack() {
        let data = Arc::new(tiny_batch());
        let cqt = |batch_start_within| {
            let config = Config {
                batch_start_within,
                ..Config::new(DAY)
            };
            let mut sim = Simulation::new(Arc::clone(&data), config).unwrap();
            sim.run(None).unwrap();
            sim.results().unwrap().periods[0].cqt_rest
        };
        // The etch ends at 12 min: the first lot waits for the second, 10 h later.
        let waited = cqt(None);
        assert_eq!(
            (waited.completed, waited.violated, waited.violation),
            (3, 1, 8 * HOUR)
        );
        // With 30 min of slack left, the first two lots start alone; the last one never waits.
        let started = cqt(Some(30 * MINUTE));
        assert_eq!(
            (started.completed, started.violated, started.slack),
            (3, 0, 3 * HOUR)
        );
    }

    /// [`tiny`]'s tool group as one rule_LSSU tool (runs of 3 lots) and a route of setups S1, S2,
    /// S1 there: after two lots with S1, its run waits for them, and they wait at S2 for the run.
    #[test]
    fn setup_runs_never_wait_on_each_other() {
        use crate::data::{Dist, Rule, SetupChange, SetupGroup, SetupId, Step, StepSetup, Unit};
        let mut data = tiny();
        data.lots.clear();
        data.streams[0].count = 1;
        data.streams[0].lots = 2;
        let group = &mut data.tool_groups[0];
        group.tools = 1;
        group.rule = Rule::SetupRun(0);
        group.breakdowns.clear();
        group.pms.clear();
        data.setups.push("S2".into());
        data.setup_changes.push(SetupChange {
            from: None,
            to: 1,
            time: Dist::Constant(10 * MINUTE),
        });
        data.setup_groups.push(SetupGroup {
            name: "Gas".into(),
            min_run: vec![(0, 3), (1, 3)],
        });
        let step = |name: &str, setup: SetupId| Step {
            name: name.into(),
            tool_group: 0,
            unit: Unit::Lot,
            time: Dist::Constant(10 * MINUTE),
            cascade_interval: None,
            batch: None,
            setup: Some(StepSetup {
                setup,
                always: false,
                time: None,
            }),
            sampling: 1.0,
            rework: None,
            dedicate_to: None,
            cqt: None,
        };
        data.routes[0].steps = vec![step("1", 0), step("2", 1), step("3", 0)];
        let mut sim = Simulation::new(Arc::new(data), Config::new(DAY)).unwrap();
        let progress = sim.run(None).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!((progress.finished, progress.completed), (true, 2));
    }

    /// [`tiny_batch`] over its day, run to the end with `recording`.
    fn recorded(recording: Recording) -> Simulation {
        let data = Arc::new(tiny_batch());
        let mut sim = Simulation::with_recording(data, Config::new(DAY), recording).unwrap();
        sim.run(None).unwrap();
        sim
    }

    fn everything() -> Recording {
        Recording {
            violations: true,
            tool_groups: true,
            events: Some(EventFilter {
                from: 0,
                until: 2 * DAY,
                tool_groups: Vec::new(),
                lots: Vec::new(),
            }),
            replay: None,
        }
    }

    #[test]
    fn segments_steps_and_days_account_for_every_wait() {
        let sim = recorded(Recording::default());
        let results = sim.results().unwrap();
        let period = &results.periods[0];
        // The first lot waited 10 h in the furnace's queue; the others did not wait.
        let segment = &period.cqt_segments[0];
        assert_eq!(
            (segment.route.as_str(), segment.entry, segment.exit),
            ("R1", 0, 1)
        );
        assert_eq!(segment.cqt, period.cqt_rest);
        let furnace = &segment.steps[0];
        assert_eq!(
            (
                furnace.step,
                furnace.violated.visits,
                furnace.violated.queue
            ),
            (1, 1, 10 * HOUR)
        );
        assert_eq!((furnace.met.visits, furnace.met.queue), (2, 0));
        // Days: the releases at 0 h, 10 h and 20 h; completions after each furnace hour.
        let days = &results.days;
        assert_eq!(days.len(), 2);
        let sum = |count: fn(&DayReport) -> u64| days.iter().map(count).sum::<u64>();
        assert_eq!(sum(|day| day.started), results.released);
        assert_eq!(sum(|day| day.completed), results.completed);
        assert_eq!(sum(|day| day.cqt.completed), 3);
        assert_eq!(days[0].cqt.violated, 1);
        // In the fab: 11.2 h, 1.2 h and 1.2 h; the run ends at midnight with an empty day 1.
        assert!((days[0].wip - 13.6 / 24.0).abs() < 1e-12);
        assert_eq!(days[1], DayReport::default());
    }

    #[test]
    fn segments_show_their_lots_and_completions_so_far() {
        let mut sim = Simulation::new(Arc::new(tiny_batch()), Config::new(DAY)).unwrap();
        // At 5 h the first lot waits in the furnace's queue: it entered at 12 min, 2 h 48 min
        // ago past its limit; nothing has completed the segment yet.
        sim.run(Some(5 * HOUR)).unwrap();
        let segments = sim.segments();
        assert_eq!(segments.len(), 1);
        assert_eq!(
            segments[0].lots,
            [SegmentLot {
                id: 0,
                kind: LotKind::Prl,
                step: 1,
                state: LotState::Queued,
                entered: 12 * MINUTE,
                slack: 12 * MINUTE + 2 * HOUR - 5 * HOUR,
            }]
        );
        assert_eq!(segments[0].cqt, CqtReport::default());
        // The second lot's etch ends at 10 h 12 min: both start the furnace and leave the clock;
        // the first one over the limit.
        sim.run(Some(10 * HOUR + 12 * MINUTE)).unwrap();
        let segment = &sim.segments()[0];
        assert!(segment.lots.is_empty());
        assert_eq!((segment.cqt.completed, segment.cqt.violated), (2, 1));
        // The totals stay after the reporting periods, and match the run's.
        sim.run(None).unwrap();
        let segment = &sim.segments()[0];
        let progress = sim.progress();
        assert_eq!(
            (segment.cqt.completed, segment.cqt.violated),
            (progress.cqt_completed, progress.cqt_violated)
        );
        assert_eq!(
            segment.cqt,
            sim.results().unwrap().periods[0].cqt_segments[0].cqt
        );
    }

    #[test]
    fn recording_leaves_the_results_unchanged() {
        let plain = recorded(Recording::default());
        assert_eq!(plain.records(), &Records::default());
        let sim = recorded(everything());
        assert_eq!(sim.results().unwrap(), plain.results().unwrap());
        let records = sim.records();
        // The violation: the first lot, entering at 12 min, waiting in the queue until 10 h 12 min.
        let violations = &records.violations;
        assert_eq!(
            (violations.lot.as_slice(), violations.segment.as_slice()),
            (&[0][..], &[0][..])
        );
        assert_eq!(
            (
                violations.entered[0],
                violations.arrived[0],
                violations.exit[0]
            ),
            (12 * MINUTE, 12 * MINUTE, 10 * HOUR + 12 * MINUTE)
        );
        // Two days of both tool groups; every tool's time accounted, the furnace's queue of one lot
        // for 10 h on day 0.
        let days = &records.tool_groups;
        assert_eq!(days.day, [0, 0, 1, 1]);
        let total = |row: usize| {
            [
                &days.down,
                &days.pm,
                &days.setup,
                &days.process,
                &days.load,
                &days.unload,
                &days.idle,
            ]
            .iter()
            .map(|column| column[row])
            .sum::<Time>()
        };
        assert_eq!((total(0), total(1)), (2 * DAY, DAY));
        assert!((days.queue[1] - 10.0 / 24.0).abs() < 1e-12);
        // Each lot: release, arrive, start, end at both steps, completion.
        let events = &records.events;
        let kinds = |lot: u64| {
            events
                .lot
                .iter()
                .zip(&events.kind)
                .filter(|(each, _)| **each == Some(lot))
                .map(|(_, kind)| kind.name())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            kinds(2),
            [
                "release", "arrive", "start", "end", "arrive", "start", "end", "complete"
            ]
        );
        assert!(events.time.windows(2).all(|pair| pair[0] <= pair[1]));
        // Lot events name the lot's part, tool events neither.
        assert!(
            events
                .lot
                .iter()
                .zip(&events.part)
                .all(|(lot, part)| lot.is_some() == part.is_some())
        );
    }

    #[test]
    fn events_pass_their_window_and_filters() {
        let lots = |filter: EventFilter| {
            let sim = recorded(Recording {
                events: Some(filter),
                ..Recording::default()
            });
            let events = &sim.records().events;
            (
                events.time.clone(),
                events.lot.clone(),
                events.tool_group.clone(),
            )
        };
        let window = EventFilter {
            from: 10 * HOUR,
            until: 11 * HOUR,
            tool_groups: vec!["Furnace_1".into()],
            lots: Vec::new(),
        };
        let (times, _, groups) = lots(window.clone());
        assert!(
            times
                .iter()
                .all(|&time| (10 * HOUR..11 * HOUR).contains(&time))
        );
        assert!(!times.is_empty() && groups.iter().all(|&group| group == Some(1)));
        let (_, events_lots, _) = lots(EventFilter {
            tool_groups: Vec::new(),
            lots: vec![1],
            from: 0,
            until: 2 * DAY,
        });
        assert!(events_lots.iter().all(|&lot| lot == Some(1)));
        let error = |filter| {
            Simulation::with_recording(
                Arc::new(tiny_batch()),
                Config::new(DAY),
                Recording {
                    events: Some(filter),
                    ..Recording::default()
                },
            )
            .err()
            .unwrap()
            .to_string()
        };
        assert_eq!(
            error(EventFilter {
                until: 10 * HOUR,
                ..window.clone()
            }),
            "the event window must end after it starts"
        );
        assert_eq!(
            error(EventFilter {
                tool_groups: vec!["Nope".into()],
                ..window
            }),
            "no tool group Nope"
        );
    }

    #[test]
    fn bad_configurations_fail_at_once() {
        assert_eq!(error(Config::new(0)), "the horizon must be positive");
        let flow_factors = Config {
            flow_factors: Some(vec![vec![Some(1.0)]]),
            ..Config::new(DAY)
        };
        assert_eq!(
            error(flow_factors),
            "flow factors apply to the QTS rule only"
        );
        for warm_up in [0, DAY] {
            let config = Config {
                warm_up: Some(warm_up),
                ..Config::new(DAY)
            };
            assert_eq!(
                error(config),
                "the warm-up must lie between 0 and the horizon"
            );
        }
        let ranking = |group: &str, criteria: Vec<Criterion>| Config {
            ranking: BTreeMap::from([(group.into(), criteria)]),
            ..Config::new(DAY)
        };
        assert_eq!(
            error(ranking("Nope", vec![Criterion::Fifo])),
            "no tool group Nope"
        );
        assert_eq!(
            error(ranking("Etch_1", Vec::new())),
            "the ranking of Etch_1 needs 1 to 6 criteria"
        );
        let batch = Config {
            batch_start_within: Some(0),
            ..Config::new(DAY)
        };
        assert_eq!(error(batch), "queue-time thresholds must be positive");
        let cate = Config {
            engineering: EngineeringRule::Cate {
                production: HOUR,
                engineering: 0,
            },
            ..Config::new(DAY)
        };
        assert_eq!(
            error(cate),
            "CAtE intervals and CoT triggers must be positive"
        );
        // Also with a first pass ahead.
        let stopping = Config {
            queue_time: QueueTimeRule::Qts,
            stopping: Some(Stopping {
                limits: BTreeMap::from([("Nope".into(), Limits::default())]),
                default: Limits::default(),
            }),
            ..Config::new(DAY)
        };
        assert_eq!(error(stopping), "no tool group Nope");
    }
}
