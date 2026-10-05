//! SMT2020 fab simulation (README model specification): lots released by the plan flow through
//! their routes on the dataset's tools; the run ends when every released lot is complete.
//!
//! [`Config`], [`Progress`] and [`Results`] are also the serialized form of the CLI and the
//! bindings: times are ms (fractional configuration times round to whole ms), omitted
//! configuration fields take their defaults.

mod dispatch;
mod fab;
mod plan;
mod routes;
mod stats;
mod strategy;
mod tool;

use std::collections::BTreeMap;
use std::fmt;
use std::ops::ControlFlow;

use des_core::{DAY, Outcome, Simulation, Time};
use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize};

use crate::data::Dataset;
use fab::Fab;

pub use stats::{
    CqtReport, FLOW_FACTOR_PERCENTILES, FlowFactors, LotKind, LotReport, PeriodReport, Results,
    StateTimes, ToolGroupReport,
};

const DEFAULT_SEED: u64 = 1;
const DEFAULT_LOAD: f64 = 1.0;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Lots planned to start before the horizon are released; reporting periods end at the
    /// horizon, after which the run continues until every released lot is complete.
    #[serde(deserialize_with = "time")]
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
    /// Without them, a run of this configuration without queue-time rule and stopping provides
    /// them first.
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

/// A time in ms from any number, rounded to whole ms as the loader rounds input times.
fn time<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Time, D::Error> {
    let ms = f64::deserialize(deserializer)?;
    if ms.is_finite() && ms.abs() < Time::MAX as f64 {
        Ok(ms.round() as Time)
    } else {
        Err(D::Error::custom(format_args!("{ms} is not a time in ms")))
    }
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
        #[serde(deserialize_with = "time")]
        production: Time,
        #[serde(deserialize_with = "time")]
        engineering: Time,
    },
    /// Once `trigger` engineering lots wait, that many go first.
    Cot { trigger: u32 },
}

/// State of a run at an observation, once per simulated day.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Progress {
    /// Pass of the run, from 0: QTS without flow factors makes a flow factor pass first.
    pub pass: u32,
    pub passes: u32,
    /// Simulation time; after the horizon the lots left drain.
    pub now: Time,
    pub horizon: Time,
    /// Lots in the fab.
    pub wip: u64,
}

#[derive(Debug)]
pub struct Error(String);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

/// Every released lot must be complete this long after the horizon; otherwise the run fails.
const DRAIN_LIMIT: Time = 365 * DAY;

/// Runs `config` on `data` until every released lot is complete.
pub fn run(data: &Dataset, config: &Config) -> Result<Results, Error> {
    run_observed(data, config, |_| ControlFlow::Continue(()))
}

/// [`run`] observed once per simulated day; breaking off cancels the run.
pub fn run_observed(
    data: &Dataset,
    config: &Config,
    mut observe: impl FnMut(&Progress) -> ControlFlow<()>,
) -> Result<Results, Error> {
    if config.queue_time != QueueTimeRule::Qts || config.flow_factors.is_some() {
        return simulate(
            data,
            config,
            config.flow_factors.as_deref(),
            0,
            1,
            &mut observe,
        );
    }
    // [P2] §3.1 takes the local flow factors from long runs without CQT-aware dispatching (assumed:
    // this configuration without queue-time rule and stopping).
    let base = Config {
        queue_time: QueueTimeRule::None,
        stopping: None,
        ..config.clone()
    };
    let flow_factors = simulate(data, &base, None, 0, 2, &mut observe)?.step_flow_factors;
    simulate(data, config, Some(&flow_factors), 1, 2, &mut observe)
}

fn simulate(
    data: &Dataset,
    config: &Config,
    flow_factors: Option<&[Vec<Option<f64>>]>,
    pass: u32,
    passes: u32,
    observe: &mut impl FnMut(&Progress) -> ControlFlow<()>,
) -> Result<Results, Error> {
    let mut simulation = Simulation::new(Fab::new(data, config, flow_factors)?);
    // The model stops the run when its last lot completes, or at its deadline.
    let outcome = simulation.run_observed(DAY, |fab, now| {
        observe(&Progress {
            pass,
            passes,
            now,
            horizon: config.horizon,
            wip: fab.wip() as u64,
        })
    });
    let fab = simulation.model();
    match outcome {
        _ if fab.finished() => Ok(fab.results(simulation.events_processed())),
        Outcome::Interrupted => Err(Error("the run was cancelled".into())),
        Outcome::Stopped | Outcome::Exhausted => Err(Error(format!(
            "{} lots unfinished a year after the horizon",
            fab.wip()
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::tiny;
    use des_core::HOUR;

    fn parse(json: &str) -> Result<Config, serde_json::Error> {
        serde_json::from_str(json)
    }

    #[test]
    fn config_fields_default_and_round_ms() {
        assert_eq!(
            parse(r#"{"horizon": 63072000000}"#).unwrap(),
            Config::new(730 * DAY)
        );
        assert_eq!(parse(r#"{"horizon": 1.5}"#).unwrap().horizon, 2);
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
        let results = run_observed(&tiny(), &Config::new(3 * DAY), |progress| {
            seen.push(*progress);
            ControlFlow::Continue(())
        })
        .unwrap();
        assert_eq!(
            (results.released, results.completed, results.end),
            (11, 11, 3 * DAY)
        );
        let observed: Vec<_> = seen
            .iter()
            .map(|p| (p.pass, p.passes, p.now, p.wip))
            .collect();
        assert_eq!(observed, [(0, 1, DAY, 0), (0, 1, 2 * DAY, 0)]);
        assert_eq!(results, run(&tiny(), &Config::new(3 * DAY)).unwrap());
    }

    #[test]
    fn qts_without_flow_factors_makes_a_flow_factor_pass() {
        let config = Config {
            queue_time: QueueTimeRule::Qts,
            ..Config::new(2 * DAY)
        };
        let mut passes = Vec::new();
        run_observed(&tiny(), &config, |progress| {
            passes.push((progress.pass, progress.passes));
            ControlFlow::Continue(())
        })
        .unwrap();
        assert_eq!(passes, [(0, 2), (1, 2)]);
    }

    #[test]
    fn breaking_off_cancels_and_bad_configurations_fail() {
        let cancelled = run_observed(&tiny(), &Config::new(3 * DAY), |_| ControlFlow::Break(()));
        assert_eq!(cancelled.unwrap_err().to_string(), "the run was cancelled");
        let error = |config: Config| run(&tiny(), &config).unwrap_err().to_string();
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
    }
}
