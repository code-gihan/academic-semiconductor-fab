//! Operating strategies of the papers resolved against a dataset.

use des_core::Time;

use super::stats::LotKind;
use super::{Config, EngineeringRule, Error, QueueTimeRule};
use crate::data::{Dataset, ToolGroupId};

/// Stepper tool groups of the CAtE/CoT ([P1] §V) and complex-CQT ([P2] §4.1) experiments.
const STEPPERS: [&str; 2] = ["LithoTrack_FE_95", "LithoTrack_FE_115"];

pub(super) enum QueueTime {
    None,
    Qtcr,
    Qts(Vec<Vec<f64>>),
}

/// Stopping limits and the constrained-lot counts they apply to, per tool group.
pub(super) struct Stopping {
    limits: Vec<(i64, i64)>,
    /// Constrained lots queued or processing at the group.
    pub front: Vec<i64>,
    /// Constrained lots that will still reach the group in their segment.
    pub upstream: Vec<i64>,
}

impl Stopping {
    pub(super) fn reached(&self, group: ToolGroupId) -> bool {
        let (front, both) = self.limits[group];
        self.front[group] >= front || self.front[group] + self.upstream[group] >= both
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
    pub(super) fn new(data: &Dataset, config: &Config) -> Result<Self, Error> {
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
        let queue_time = match &config.queue_time {
            QueueTimeRule::None => QueueTime::None,
            QueueTimeRule::Qtcr => QueueTime::Qtcr,
            QueueTimeRule::Qts { flow_factors } => {
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
                QueueTime::Qts(flow_factors.clone())
            }
        };
        let stopping = match &config.stopping {
            None => None,
            Some(stopping) => {
                // A zero limit would hold lots forever.
                let given = stopping
                    .limits
                    .iter()
                    .map(|&(_, front, both)| (front, both));
                if given
                    .chain([stopping.default])
                    .any(|(front, both)| front == 0 || both == 0)
                {
                    return Err(Error("stopping limits must be positive".into()));
                }
                let (front, both) = stopping.default;
                let mut limits = vec![(i64::from(front), i64::from(both)); data.tool_groups.len()];
                for (name, front, both) in &stopping.limits {
                    limits[group_id(name)?] = (i64::from(*front), i64::from(*both));
                }
                let groups = data.tool_groups.len();
                Some(Stopping {
                    limits,
                    front: vec![0; groups],
                    upstream: vec![0; groups],
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

    /// CAtE/CoT on the steppers: 0 for lots of the preferred class, 1 otherwise; `None` elsewhere.
    pub(super) fn class_rank(
        &self,
        group: ToolGroupId,
        kind: LotKind,
        now: Time,
        campaign: u32,
    ) -> Option<f64> {
        if !self.steppers[group] {
            return None;
        }
        let prefer_engineering = match self.engineering {
            Engineering::Cate { production, cycle } => now.rem_euclid(cycle) >= production,
            Engineering::Cot(_) => campaign > 0,
            Engineering::Base | Engineering::First => return None,
        };
        Some(if kind.engineering() == prefer_engineering {
            0.0
        } else {
            1.0
        })
    }

    /// CoT trigger limit.
    pub(super) fn campaign_trigger(&self) -> Option<u32> {
        match self.engineering {
            Engineering::Cot(trigger) => Some(trigger),
            _ => None,
        }
    }
}
