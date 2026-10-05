//! Operating strategies of the papers resolved against a dataset.

use des_core::Time;

use super::stats::LotKind;
use super::{Config, EngineeringRule, Error, Limits, QueueTimeRule};
use crate::data::{Dataset, ToolGroupId};

/// Stepper tool groups of the CAtE/CoT ([P1] §V) and complex-CQT ([P2] §4.1) experiments.
const STEPPERS: [&str; 2] = ["LithoTrack_FE_95", "LithoTrack_FE_115"];

pub(super) enum QueueTime {
    None,
    Qtcr,
    /// Flow factors per route and step; never measured counts as 1.
    Qts(Vec<Vec<f64>>),
}

/// Stopping limits and the constrained-lot counts they apply to, per tool group.
pub(super) struct Stopping {
    limits: Vec<(i64, i64)>,
    /// Constrained lots queued or processing at the group.
    front: Vec<i64>,
    /// Constrained lots that will still reach the group in their segment.
    upstream: Vec<i64>,
    /// Groups whose counts changed during the current event, and whether their limit was reached
    /// before it.
    changed: Vec<(ToolGroupId, bool)>,
}

impl Stopping {
    pub(super) fn reached(&self, group: ToolGroupId) -> bool {
        let (front, both) = self.limits[group];
        self.front[group] >= front || self.front[group] + self.upstream[group] >= both
    }

    /// Adds `delta` constrained lots in front of `group` (`front`) or upstream of it.
    pub(super) fn count(&mut self, group: ToolGroupId, front: bool, delta: i64) {
        if !self.changed.iter().any(|&(changed, _)| changed == group) {
            let reached = self.reached(group);
            self.changed.push((group, reached));
        }
        let counts = if front {
            &mut self.front
        } else {
            &mut self.upstream
        };
        counts[group] += delta;
    }

    /// Ends an event: true if a limit reached before it no longer is, which releases held lots.
    pub(super) fn released(&mut self) -> bool {
        let released = self
            .changed
            .iter()
            .any(|&(group, was)| was && !self.reached(group));
        self.changed.clear();
        released
    }
}

pub(super) struct Strategy {
    pub steppers: Vec<bool>,
    pub queue_time: QueueTime,
    pub stopping: Option<Stopping>,
    engineering: Engineering,
}

enum Engineering {
    Base,
    First,
    Cate { production: Time, cycle: Time },
    Cot(u32),
}

impl Strategy {
    /// Resolves `config` against `data`; `flow_factors` are the QTS flow factors.
    pub(super) fn new(
        data: &Dataset,
        config: &Config,
        flow_factors: Option<&[Vec<Option<f64>>]>,
    ) -> Result<Self, Error> {
        let group_id = |name: &str| {
            data.tool_groups
                .iter()
                .position(|group| group.name == name)
                .ok_or_else(|| Error(format!("no tool group {name}")))
        };
        let engineering = match config.engineering {
            EngineeringRule::Base => Engineering::Base,
            EngineeringRule::EngineeringFirst => Engineering::First,
            EngineeringRule::Cate {
                production,
                engineering,
            } if production > 0 && engineering > 0 => Engineering::Cate {
                production,
                cycle: production + engineering,
            },
            EngineeringRule::Cot { trigger } if trigger > 0 => Engineering::Cot(trigger),
            _ => {
                return Err(Error(
                    "CAtE intervals and CoT triggers must be positive".into(),
                ));
            }
        };
        // Steppers also split the CQT statistics, so a dataset may lack them unless CAtE/CoT run.
        let mut steppers = vec![false; data.tool_groups.len()];
        for name in STEPPERS {
            match group_id(name) {
                Ok(group) => steppers[group] = true,
                Err(error)
                    if matches!(engineering, Engineering::Cate { .. } | Engineering::Cot(_)) =>
                {
                    return Err(error);
                }
                Err(_) => {}
            }
        }
        if config.flow_factors.is_some() && config.queue_time != QueueTimeRule::Qts {
            return Err(Error("flow factors apply to the QTS rule only".into()));
        }
        let queue_time = match config.queue_time {
            QueueTimeRule::None => QueueTime::None,
            QueueTimeRule::Qtcr => QueueTime::Qtcr,
            QueueTimeRule::Qts => {
                let flow_factors = flow_factors.expect("QTS runs with flow factors");
                let fits = flow_factors.len() == data.routes.len()
                    && flow_factors
                        .iter()
                        .zip(&data.routes)
                        .all(|(ff, route)| ff.len() == route.steps.len());
                if !fits {
                    return Err(Error(
                        "QTS flow factors need one value per route step".into(),
                    ));
                }
                let mut resolved = Vec::with_capacity(flow_factors.len());
                for route in flow_factors {
                    let mut steps = Vec::with_capacity(route.len());
                    for &ff in route {
                        match ff {
                            None => steps.push(1.0),
                            Some(ff) if ff.is_finite() && ff >= 0.0 => steps.push(ff),
                            Some(ff) => {
                                return Err(Error(format!(
                                    "QTS flow factor {ff} is not finite and non-negative"
                                )));
                            }
                        }
                    }
                    resolved.push(steps);
                }
                QueueTime::Qts(resolved)
            }
        };
        let stopping = match &config.stopping {
            None => None,
            Some(stopping) => {
                // A zero limit would hold lots forever.
                if stopping
                    .limits
                    .values()
                    .chain([&stopping.default])
                    .any(|limits| limits.front == 0 || limits.total == 0)
                {
                    return Err(Error("stopping limits must be positive".into()));
                }
                let pair = |limits: &Limits| (i64::from(limits.front), i64::from(limits.total));
                let mut limits = vec![pair(&stopping.default); data.tool_groups.len()];
                for (name, given) in &stopping.limits {
                    limits[group_id(name)?] = pair(given);
                }
                let groups = data.tool_groups.len();
                Some(Stopping {
                    limits,
                    front: vec![0; groups],
                    upstream: vec![0; groups],
                    changed: Vec::new(),
                })
            }
        };
        Ok(Self {
            steppers,
            queue_time,
            stopping,
            engineering,
        })
    }

    /// Dispatch priority of a lot: the dataset's, or the EF priorities.
    pub(super) fn priority(&self, kind: LotKind, priority: u32) -> u32 {
        match (&self.engineering, kind) {
            (Engineering::First, LotKind::Ehl) => 25,
            (Engineering::First, LotKind::Erl) => 15,
            _ => priority,
        }
    }

    /// CAtE/CoT on the steppers: whether engineering lots are preferred over production lots now;
    /// `None` elsewhere.
    pub(super) fn prefer_engineering(
        &self,
        group: ToolGroupId,
        now: Time,
        campaign: u32,
    ) -> Option<bool> {
        if !self.steppers[group] {
            return None;
        }
        match self.engineering {
            Engineering::Cate { production, cycle } => Some(now.rem_euclid(cycle) >= production),
            Engineering::Cot(_) => Some(campaign > 0),
            Engineering::Base | Engineering::First => None,
        }
    }

    /// CoT trigger limit.
    pub(super) fn campaign_trigger(&self) -> Option<u32> {
        match self.engineering {
            Engineering::Cot(trigger) => Some(trigger),
            _ => None,
        }
    }
}
