//! Strategy code on the web page's DS1: the papers' rules written as code run as the built-in
//! rules do, and the code's answers and errors reach the run.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

use smt2020::sim::{
    Admit, BatchView, Code, Hooks, Limits, LotView, QueueTimeRule, Recording, SegmentView, Start,
    Stopping,
};
use smt2020::{Config, Criterion, DAY, Dataset, HOUR, Simulation, Time};

type Priority = Box<dyn FnMut(&LotView, Time) -> Result<f64, String> + Send + Sync>;
type Admission =
    Box<dyn FnMut(&LotView, &SegmentView, Time) -> Result<Admit, String> + Send + Sync>;
type BatchStart = Box<dyn FnMut(&BatchView, Time) -> Result<Start, String> + Send + Sync>;

/// Strategy code from closures; a hook is defined when its closure is.
#[derive(Default)]
struct Hooked {
    priority: Option<Priority>,
    admit: Option<Admission>,
    start_batch: Option<BatchStart>,
}

impl Code for Hooked {
    fn hooks(&self) -> Hooks {
        Hooks {
            priority: self.priority.is_some(),
            admit: self.admit.is_some(),
            start_batch: self.start_batch.is_some(),
        }
    }

    fn priority(&mut self, lot: &LotView, now: Time) -> Result<f64, String> {
        (self.priority.as_mut().expect("priority"))(lot, now)
    }

    fn admit(&mut self, lot: &LotView, segment: &SegmentView, now: Time) -> Result<Admit, String> {
        (self.admit.as_mut().expect("admit"))(lot, segment, now)
    }

    fn start_batch(&mut self, batch: &BatchView, now: Time) -> Result<Start, String> {
        (self.start_batch.as_mut().expect("start_batch"))(batch, now)
    }
}

fn ds1() -> Arc<Dataset> {
    let path: PathBuf = [env!("CARGO_MANIFEST_DIR"), "../../www/data/ds1.bin"]
        .iter()
        .collect();
    Arc::new(Dataset::from_bytes(&fs::read(path).unwrap()).unwrap())
}

/// The state of `simulation` after `days`: progress, lots, tools and CQT segments.
fn state(mut simulation: Simulation, days: i64) -> String {
    simulation.run(Some(days * DAY)).unwrap();
    format!(
        "{:?}\n{:?}\n{:?}\n{:?}",
        simulation.progress(),
        simulation.lots(),
        simulation.tools(),
        simulation.segments()
    )
}

fn plain(data: &Arc<Dataset>, config: Config) -> Simulation {
    Simulation::new(Arc::clone(data), config).unwrap()
}

fn coded(data: &Arc<Dataset>, config: Config, code: Hooked) -> Simulation {
    Simulation::with_code(
        Arc::clone(data),
        config,
        Recording::default(),
        Box::new(code),
    )
    .unwrap()
}

/// The exit tool groups of the dataset's CQT segments, by name.
fn exit_groups(data: &Dataset) -> Vec<String> {
    let info = data.info();
    let mut names: Vec<String> = info
        .segments
        .iter()
        .map(|segment| {
            let step = &info.routes[segment.route].steps[segment.exit];
            info.tool_groups[step.tool_group].name.clone()
        })
        .collect();
    names.sort();
    names.dedup();
    names
}

#[test]
fn code_ranks_where_the_configuration_says() {
    let data = ds1();
    let calls = Arc::new(AtomicUsize::new(0));
    // The same value everywhere ranks nothing: the queue-time slot changes no order.
    let counted = Arc::clone(&calls);
    let constant = Hooked {
        priority: Some(Box::new(move |_, _| {
            counted.fetch_add(1, Relaxed);
            Ok(0.0)
        })),
        ..Hooked::default()
    };
    let code_rule = Config {
        queue_time: QueueTimeRule::Code,
        ..Config::new(30 * DAY)
    };
    assert_eq!(
        state(coded(&data, code_rule, constant), 5),
        state(plain(&data, Config::new(30 * DAY)), 5)
    );
    assert!(calls.load(Relaxed) > 10_000);
    // The end of the lot's segment, as code, ranks as the built-in criterion.
    let ranking = |criterion: Criterion| Config {
        ranking: exit_groups(&data)
            .into_iter()
            .map(|group| (group, vec![criterion, Criterion::Fifo]))
            .collect(),
        ..Config::new(30 * DAY)
    };
    let deadline = Hooked {
        priority: Some(Box::new(|lot, _| {
            Ok(lot.cqt.map_or(f64::INFINITY, |cqt| cqt.deadline as f64))
        })),
        ..Hooked::default()
    };
    assert_eq!(
        state(coded(&data, ranking(Criterion::Code), deadline), 5),
        state(plain(&data, ranking(Criterion::QtDeadline)), 5)
    );
}

#[test]
fn admission_code_holds_as_stopping_does() {
    let data = ds1();
    let steppers = ["LithoTrack_FE_95", "LithoTrack_FE_115"];
    let limits = Limits {
        front: 5,
        total: 10,
    };
    let stopping = Config {
        stopping: Some(Stopping {
            limits: steppers
                .iter()
                .map(|&name| (name.to_string(), limits))
                .collect::<BTreeMap<_, _>>(),
            default: Limits::default(),
        }),
        ..Config::new(30 * DAY)
    };
    let info = data.info();
    let stepper: Vec<bool> = info
        .tool_groups
        .iter()
        .map(|group| steppers.contains(&group.name.as_str()))
        .collect();
    let held = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&held);
    let code = Hooked {
        admit: Some(Box::new(move |_, segment, _| {
            let reached = segment.groups.iter().any(|group| {
                let (front, total) = if stepper[group.tool_group] {
                    (5, 10)
                } else {
                    (1_000, 1_000)
                };
                group.front >= front || group.total >= total
            });
            counted.fetch_add(usize::from(reached), Relaxed);
            Ok(if reached { Admit::Hold } else { Admit::Now })
        })),
        ..Hooked::default()
    };
    assert_eq!(
        state(coded(&data, Config::new(30 * DAY), code), 10),
        state(plain(&data, stopping), 10)
    );
    assert!(held.load(Relaxed) > 0, "the limits never held a lot");
}

#[test]
fn batch_start_code_starts_as_the_slack_threshold_does() {
    let data = ds1();
    let within = HOUR;
    let threshold = Config {
        batch_start_within: Some(within),
        ..Config::new(30 * DAY)
    };
    let started = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&started);
    let code = Hooked {
        start_batch: Some(Box::new(move |batch, now| {
            Ok(match batch.slack {
                Some(slack) if slack <= within as f64 => {
                    counted.fetch_add(1, Relaxed);
                    Start::Now
                }
                // When the slack reaches the threshold, as a whole ms.
                Some(slack) => {
                    Start::WaitUntil((now as f64 + slack - within as f64).ceil() as Time)
                }
                None => Start::Wait,
            })
        })),
        ..Hooked::default()
    };
    assert_eq!(
        state(coded(&data, Config::new(30 * DAY), code), 10),
        state(plain(&data, threshold), 10)
    );
    assert!(started.load(Relaxed) > 0, "no batch started early");
}

#[test]
fn code_errors_stop_the_run_for_good() {
    let data = ds1();
    let failing = Hooked {
        priority: Some(Box::new(|lot, _| {
            if lot.id == 3_000 {
                Err("no lot 3000".into())
            } else {
                Ok(0.0)
            }
        })),
        ..Hooked::default()
    };
    let config = Config {
        queue_time: QueueTimeRule::Code,
        ..Config::new(30 * DAY)
    };
    let mut simulation = coded(&data, config.clone(), failing);
    let error = simulation.run(None).unwrap_err().to_string();
    assert_eq!(error, "strategy code: priority: no lot 3000");
    assert_eq!(simulation.run(None).unwrap_err().to_string(), error);
    let nan = Hooked {
        priority: Some(Box::new(|_, _| Ok(f64::NAN))),
        ..Hooked::default()
    };
    assert_eq!(
        coded(&data, config.clone(), nan)
            .run(None)
            .unwrap_err()
            .to_string(),
        "strategy code: priority returned NaN"
    );
    // A configuration that ranks by code needs code with a priority.
    assert_eq!(
        plain(&data, config.clone())
            .run(Some(DAY))
            .unwrap_err()
            .to_string(),
        "the configuration ranks by code, but no strategy code was given"
    );
    let admit_only = Hooked {
        admit: Some(Box::new(|_, _, _| Ok(Admit::Now))),
        ..Hooked::default()
    };
    let refused = Simulation::with_code(
        Arc::clone(&data),
        config,
        Recording::default(),
        Box::new(admit_only),
    );
    assert_eq!(
        refused.err().map(|error| error.to_string()).as_deref(),
        Some("the configuration ranks by code, but the strategy code has no priority")
    );
    let empty = Simulation::with_code(
        Arc::clone(&data),
        Config::new(DAY),
        Recording::default(),
        Box::new(Hooked::default()),
    );
    assert_eq!(
        empty.err().map(|error| error.to_string()).as_deref(),
        Some("the strategy code defines no hook")
    );
}

#[test]
fn the_code_stays_through_resets_and_skips_the_qts_first_pass() {
    let data = ds1();
    let calls = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&calls);
    let code = Hooked {
        priority: Some(Box::new(move |lot, _| {
            counted.fetch_add(1, Relaxed);
            Ok(-f64::from(lot.wafers))
        })),
        ..Hooked::default()
    };
    // QTS needs a first pass, which ranks without the code.
    let config = Config {
        queue_time: QueueTimeRule::Code,
        ranking: BTreeMap::from([("Diffusion_FE_120".into(), vec![Criterion::Qts])]),
        ..Config::new(2 * DAY)
    };
    let mut simulation = coded(&data, config.clone(), code);
    assert_eq!(simulation.progress().passes, 2);
    simulation
        .run_observed(None, |progress| {
            assert!(progress.pass == 1 || calls.load(Relaxed) == 0);
            std::ops::ControlFlow::Continue(())
        })
        .unwrap();
    let first = simulation.results().unwrap();
    assert!(calls.load(Relaxed) > 0);
    // A reset keeps the code, and a rejected configuration leaves it in place.
    assert!(
        simulation
            .reset(Config {
                horizon: 0,
                ..config.clone()
            })
            .is_err()
    );
    simulation.reset(config).unwrap();
    simulation.run(None).unwrap();
    assert_eq!(simulation.results().unwrap(), first);
}
