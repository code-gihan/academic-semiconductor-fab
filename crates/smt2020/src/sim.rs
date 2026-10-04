//! SMT2020 fab simulation (README model specification): lots released by the plan flow through
//! their routes on the dataset's tools; the run ends when every released lot is complete.

mod dispatch;
mod fab;
mod plan;
mod routes;
mod stats;
mod strategy;
mod tool;

use std::fmt;

use des_core::{DAY, Simulation, Time};

use crate::data::Dataset;
use fab::Fab;

pub use stats::{CqtReport, LotKind, LotReport, PeriodReport, Results, ToolGroupReport};
pub use tool::State;

pub struct Config {
    /// Lots planned to start before the horizon are released; reporting periods end at the
    /// horizon, after which the run continues until every released lot is complete.
    pub horizon: Time,
    pub seed: u64,
    pub replication: u32,
    /// Release rate factor (operating curves): release times are divided by it, due-date offsets
    /// kept. 1.0 is the dataset plan.
    pub load: f64,
    /// Super hot lots reserve a tool at their next step even where the dataset says HOTLOT = no.
    pub reserve_super_hot: bool,
    pub queue_time: QueueTimeRule,
    pub stopping: Option<Stopping>,
    pub engineering: EngineeringRule,
}

/// Critical queue time dispatching of [P2] §3.1, ranked after least setup and before FIFO/CR.
pub enum QueueTimeRule {
    None,
    /// Queue time critical ratio, eq. (1).
    Qtcr,
    /// Queue time slack, eq. (2)–(6), with flow factors per route and step such as
    /// [`Results::step_flow_factors`] of a long BASE run.
    Qts {
        flow_factors: Vec<Vec<f64>>,
    },
}

/// Kanban-type stopping at CQT segment entrances ([P2] §3.2): per tool group, limits on the
/// constrained lots in front of it (queued or processing) and on those plus the constrained lots
/// still upstream of it in their segments. Limits must be positive. Limits low enough to hold the
/// lots a waiting batch or setup run needs block those lots for good, and the run fails.
pub struct Stopping {
    /// (tool group, first limit, second limit).
    pub limits: Vec<(String, u32, u32)>,
    /// Limits of all other tool groups ([P2] Table 3: 1,000/1,000).
    pub default: (u32, u32),
}

/// Production and engineering lot dispatching of [P1] §V. CAtE and CoT apply to the steppers
/// LithoTrack_FE_95 and LithoTrack_FE_115 only.
pub enum EngineeringRule {
    Base,
    /// Priorities EHL 25, PHL 20, ERL 15, PRL 10.
    EngineeringFirst,
    /// Alternating production and engineering intervals, each preferring its lots.
    Cate {
        production: Time,
        engineering: Time,
    },
    /// Once `trigger` engineering lots wait, that many go first.
    Cot {
        trigger: u32,
    },
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

pub fn run(data: &Dataset, config: &Config) -> Result<Results, Error> {
    let mut simulation = Simulation::new(Fab::new(data, config)?);
    let mut until = 0;
    while !simulation.model().finished() {
        if until > config.horizon + DRAIN_LIMIT {
            return Err(Error(format!(
                "{} lots unfinished a year after the horizon",
                simulation.model().wip()
            )));
        }
        until += DAY;
        simulation.run_until(until);
    }
    Ok(simulation.model().results(simulation.events_processed()))
}
