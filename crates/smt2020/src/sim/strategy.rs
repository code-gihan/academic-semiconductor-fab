//! Operating strategies resolved against a dataset: the papers' rules and the configured lot
//! rankings.

use std::mem::discriminant;

use des_core::Time;

use super::stats::LotKind;
use super::{Config, Criterion, EngineeringRule, Error, Limits, QueueTimeRule};
use crate::data::{Dataset, Rank, ToolGroupId};

/// Stepper tool groups of the CAtE/CoT ([P1] §V) and complex-CQT ([P2] §4.1) experiments.
pub(crate) const STEPPERS: [&str; 2] = ["LithoTrack_FE_95", "LithoTrack_FE_115"];

/// Most criteria of a configured ranking.
pub const MAX_CRITERIA: usize = 6;

/// Stopping limits per tool group: constrained lots in front, and in front or upstream.
pub(super) struct Stopping {
    limits: Vec<(i64, i64)>,
}

impl Stopping {
    pub(super) fn reached(&self, counts: &SegmentCounts, group: ToolGroupId) -> bool {
        self.reached_at(group, counts.front[group], counts.upstream[group])
    }

    fn reached_at(&self, group: ToolGroupId, front: i64, upstream: i64) -> bool {
        let (front_limit, total_limit) = self.limits[group];
        front >= front_limit || front + upstream >= total_limit
    }
}

/// Constrained lots (in CQT segments) per tool group, which stopping limits and admission code
/// read.
pub(super) struct SegmentCounts {
    /// Queued or processing at the group.
    pub front: Vec<i64>,
    /// Still to reach the group in their segment.
    pub upstream: Vec<i64>,
    /// Groups whose counts changed during the current event, with their counts before it.
    changed: Vec<(ToolGroupId, i64, i64)>,
}

impl SegmentCounts {
    pub(super) fn new(groups: usize) -> Self {
        Self {
            front: vec![0; groups],
            upstream: vec![0; groups],
            changed: Vec::new(),
        }
    }

    /// Adds `delta` constrained lots in front of `group` (`front`) or upstream of it.
    pub(super) fn count(&mut self, group: ToolGroupId, front: bool, delta: i64) {
        if !self.changed.iter().any(|&(changed, _, _)| changed == group) {
            self.changed
                .push((group, self.front[group], self.upstream[group]));
        }
        let counts = if front {
            &mut self.front
        } else {
            &mut self.upstream
        };
        counts[group] += delta;
    }

    /// Ends an event: true if a `stopping` limit reached before it no longer is, which releases
    /// held lots; the groups with fewer lots in front or in total than before it go to `fewer`.
    pub(super) fn settle(
        &mut self,
        stopping: Option<&Stopping>,
        fewer: &mut Vec<ToolGroupId>,
    ) -> bool {
        let mut released = false;
        for &(group, front, upstream) in &self.changed {
            let (now_front, now_upstream) = (self.front[group], self.upstream[group]);
            released |= stopping.is_some_and(|stopping| {
                stopping.reached_at(group, front, upstream)
                    && !stopping.reached_at(group, now_front, now_upstream)
            });
            if now_front < front || now_front + now_upstream < front + upstream {
                fewer.push(group);
            }
        }
        self.changed.clear();
        released
    }
}

pub(super) struct Strategy {
    pub steppers: Vec<bool>,
    /// Ranking criteria per tool group, most significant first; release order breaks ties.
    pub ranking: Vec<Vec<Criterion>>,
    /// Tool groups ranking by the strategy code's priority.
    pub code_ranked: Vec<bool>,
    /// QTS flow factors per route and step (never measured counts as 1), when QTS ranks.
    pub flow_factors: Option<Vec<Vec<f64>>>,
    pub batch_start_within: Option<Time>,
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
        let mut ranking: Vec<Vec<Criterion>> = data
            .tool_groups
            .iter()
            .map(|group| dataset_ranking(&group.ranks, config.queue_time))
            .collect();
        for (name, criteria) in &config.ranking {
            check_ranking(name, criteria)?;
            ranking[group_id(name)?] = criteria.clone();
        }
        if config.batch_start_within.is_some_and(|within| within <= 0) {
            return Err(Error("queue-time thresholds must be positive".into()));
        }
        let uses_qts = config.uses_qts();
        if config.flow_factors.is_some() && !uses_qts {
            return Err(Error("flow factors apply to the QTS rule only".into()));
        }
        let flow_factors = if uses_qts {
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
            Some(resolved)
        } else {
            None
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
                Some(Stopping { limits })
            }
        };
        let code_ranked = ranking
            .iter()
            .map(|criteria| criteria.contains(&Criterion::Code))
            .collect();
        Ok(Self {
            steppers,
            ranking,
            code_ranked,
            flow_factors,
            batch_start_within: config.batch_start_within,
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

/// A tool group's dataset ranks with the queue-time rule right before FIFO/CR ([P2] §3.1).
fn dataset_ranking(ranks: &[Rank], rule: QueueTimeRule) -> Vec<Criterion> {
    let mut criteria = Vec::with_capacity(ranks.len() + 1);
    for &rank in ranks {
        if matches!(rank, Rank::Fifo | Rank::CriticalRatio) {
            match rule {
                QueueTimeRule::None => {}
                QueueTimeRule::Qtcr => criteria.push(Criterion::Qtcr),
                QueueTimeRule::Qts => criteria.push(Criterion::Qts),
                QueueTimeRule::Code => criteria.push(Criterion::Code),
            }
        }
        criteria.push(rank.into());
    }
    criteria
}

/// A configured ranking: 1 to [`MAX_CRITERIA`] distinct criteria, positive thresholds.
fn check_ranking(group: &str, criteria: &[Criterion]) -> Result<(), Error> {
    if criteria.is_empty() || criteria.len() > MAX_CRITERIA {
        return Err(Error(format!(
            "the ranking of {group} needs 1 to {MAX_CRITERIA} criteria"
        )));
    }
    for (index, criterion) in criteria.iter().enumerate() {
        if criteria[..index]
            .iter()
            .any(|earlier| discriminant(earlier) == discriminant(criterion))
        {
            return Err(Error(format!(
                "the ranking of {group} repeats {}",
                criterion.name()
            )));
        }
        if matches!(criterion, Criterion::QtWithin(within) if *within <= 0) {
            return Err(Error("queue-time thresholds must be positive".into()));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dataset_ranks_take_the_queue_time_rule_before_fifo_and_cr() {
        use Criterion as C;
        let ranks = [Rank::Priority, Rank::LeastSetup, Rank::Fifo];
        assert_eq!(
            dataset_ranking(&ranks, QueueTimeRule::None),
            [C::Priority, C::LeastSetup, C::Fifo]
        );
        assert_eq!(
            dataset_ranking(&ranks, QueueTimeRule::Qtcr),
            [C::Priority, C::LeastSetup, C::Qtcr, C::Fifo]
        );
        assert_eq!(
            dataset_ranking(&[Rank::CriticalRatio], QueueTimeRule::Qts),
            [C::Qts, C::CriticalRatio]
        );
        assert_eq!(
            dataset_ranking(&ranks, QueueTimeRule::Code),
            [C::Priority, C::LeastSetup, C::Code, C::Fifo]
        );
    }

    #[test]
    fn configured_rankings_are_checked() {
        let error = |criteria: &[Criterion]| check_ranking("G", criteria).unwrap_err().to_string();
        assert_eq!(error(&[]), "the ranking of G needs 1 to 6 criteria");
        assert_eq!(
            error(&[Criterion::Fifo; MAX_CRITERIA + 1]),
            "the ranking of G needs 1 to 6 criteria"
        );
        assert_eq!(
            error(&[
                Criterion::QtWithin(1),
                Criterion::Fifo,
                Criterion::QtWithin(2)
            ]),
            "the ranking of G repeats qt_within"
        );
        assert_eq!(
            error(&[Criterion::QtWithin(0)]),
            "queue-time thresholds must be positive"
        );
        assert!(check_ranking("G", &[Criterion::QtWithin(1), Criterion::Qts]).is_ok());
    }
}
