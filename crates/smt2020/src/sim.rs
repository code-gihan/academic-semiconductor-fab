//! SMT2020 fab simulation (README model specification): lots released by the plan flow through
//! their routes on the dataset's tools; a run ends when every released lot is complete.
//!
//! A [`Simulation`] runs one [`Config`] on a dataset from time 0, in steps: up to a given time,
//! or until an observer pauses it; the fab's state can be read in between. [`Config`],
//! [`Progress`], the status types and [`Results`] are also the serialized form of the CLI and the
//! bindings: times are ms (fractional times round to whole ms), omitted configuration fields take
//! their defaults.

mod dispatch;
mod fab;
mod plan;
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

use crate::data::Dataset;
use fab::Fab;
use strategy::Strategy;

pub use fab::LotState;
pub use stats::{
    CqtReport, FLOW_FACTOR_PERCENTILES, FlowFactors, LotKind, LotReport, PeriodReport, Results,
    StateTimes, ToolGroupReport,
};
pub use status::{LotStatus, ToolGroupStatus, ToolStatus};
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
    #[serde(default)]
    pub queue_time: QueueTimeRule,
    /// QTS flow factors per route and step, as [`Results::step_flow_factors`] of an earlier run.
    /// Without them, a first pass of this configuration without queue-time rule and stopping
    /// measures them.
    #[serde(default)]
    pub flow_factors: Option<Vec<Vec<Option<f64>>>>,
    #[serde(default)]
    pub stopping: Option<Stopping>,
    #[serde(default)]
    pub engineering: EngineeringRule,
}

impl Config {
    /// The dataset's own rules over `horizon`: seed 1, replication 0, the plan's load.
    pub fn new(horizon: Time) -> Self {
        Self {
            horizon,
            seed: DEFAULT_SEED,
            replication: 0,
            load: DEFAULT_LOAD,
            reserve_super_hot: false,
            queue_time: QueueTimeRule::None,
            flow_factors: None,
            stopping: None,
            engineering: EngineeringRule::Base,
        }
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

/// Critical queue time dispatching of \[P2\] §3.1, ranked after least setup and before FIFO/CR.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QueueTimeRule {
    #[default]
    None,
    /// Queue time critical ratio, eq. (1).
    Qtcr,
    /// Queue time slack, eq. (2)–(6), with [`Config::flow_factors`].
    Qts,
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
    /// Every lot of the configured run is complete: the results are final.
    pub finished: bool,
}

#[derive(Clone, Debug)]
pub struct Error(String);

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
/// Between runs, [`progress`](Self::progress), [`lots`](Self::lots), [`tools`](Self::tools) and
/// [`tool_groups`](Self::tool_groups) read the fab's state, and [`reset`](Self::reset) starts over;
/// the finished run has its [`results`](Self::results). Pausing leaves the results unchanged.
///
/// QTS without flow factors runs in two passes: the configuration without queue-time rule and
/// stopping measures the flow factors (\[P2\] §3.1, assumed), then the configured run takes them.
pub struct Simulation {
    data: Arc<Dataset>,
    config: Config,
    pass: u32,
    passes: u32,
    engine: des_core::Simulation<Fab>,
    /// Failure of a run that passed its deadline; every later run reports it.
    failure: Option<Error>,
}

impl Simulation {
    /// The simulation of `config` on `data` at time 0.
    pub fn new(data: Arc<Dataset>, config: Config) -> Result<Self, Error> {
        let measure_first =
            config.queue_time == QueueTimeRule::Qts && config.flow_factors.is_none();
        let fab = if measure_first {
            // The configured run's rules are checked now, not after the first pass.
            let unmeasured: Vec<Vec<Option<f64>>> = data
                .routes
                .iter()
                .map(|route| vec![None; route.steps.len()])
                .collect();
            Strategy::new(&data, &config, Some(&unmeasured))?;
            let first = Config {
                queue_time: QueueTimeRule::None,
                stopping: None,
                ..config.clone()
            };
            Fab::new(Arc::clone(&data), &first, None)?
        } else {
            Fab::new(Arc::clone(&data), &config, config.flow_factors.as_deref())?
        };
        Ok(Self {
            data,
            config,
            pass: 0,
            passes: if measure_first { 2 } else { 1 },
            engine: des_core::Simulation::new(fab),
            failure: None,
        })
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Starts over at time 0 with `config`; on error the simulation stays as it was.
    pub fn reset(&mut self, config: Config) -> Result<(), Error> {
        *self = Self::new(Arc::clone(&self.data), config)?;
        Ok(())
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
                    let fab = Fab::new(
                        Arc::clone(&self.data),
                        &self.config,
                        Some(fab.step_flow_factors()),
                    )?;
                    self.engine = des_core::Simulation::new(fab);
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
            finished: pass + 1 == passes && fab.finished(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::tiny;
    use des_core::HOUR;
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
            r#"{"horizon": 1e9, "seed": 7, "replication": 3, "load": 0.9,
                "reserve_super_hot": true, "queue_time": "qtcr",
                "stopping": {"limits": {"LithoTrack_FE_95": {"front": 50, "total": 85}}},
                "engineering": {"cate": {"production": 544320000.0000001, "engineering": 6.048e7}}}"#,
        )
        .unwrap();
        assert_eq!(
            config,
            Config {
                horizon: 1_000_000_000,
                seed: 7,
                replication: 3,
                load: 0.9,
                reserve_super_hot: true,
                queue_time: QueueTimeRule::Qtcr,
                flow_factors: None,
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
            }
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
