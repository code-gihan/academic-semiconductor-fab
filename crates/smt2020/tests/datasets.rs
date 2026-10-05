//! Loads and runs the four SMT2020 AutoSched datasets from `data/raw` (the extracted
//! `SMT_2020 - Final` folder, not committed): `cargo test -p smt2020 --release -- --ignored`.
//! Expected values were read off the raw files. The web page's dataset files (`www/data`) are
//! checked, and run in steps, without the raw data.

use std::collections::BTreeMap;
use std::fs;
use std::ops::ControlFlow::{Break, Continue};
use std::path::PathBuf;
use std::sync::Arc;

use smt2020::asd::{self, Order};
use smt2020::data::{
    BatchCriterion, BatchSize, Breakdown, Cqt, Dataset, Dist, LotRelease, Pm, PmTrigger, Rank,
    ReleaseStream, Rule, ToolGroup, Unit,
};
use smt2020::sim::{Config, EngineeringRule, Limits, QueueTimeRule, Results, Simulation, Stopping};
use smt2020::{DAY, HOUR, MINUTE, SECOND};

fn model_dir(dataset: &str, model: &str) -> PathBuf {
    [
        env!("CARGO_MANIFEST_DIR"),
        "../../data/raw/AutoSched",
        dataset,
        model,
        &format!("{model}.asd"),
    ]
    .iter()
    .collect()
}

fn load(dataset: &str, model: &str) -> Dataset {
    asd::load(&model_dir(dataset, model)).unwrap_or_else(|error| panic!("{error}"))
}

fn group<'a>(dataset: &'a Dataset, name: &str) -> &'a ToolGroup {
    dataset
        .tool_groups
        .iter()
        .find(|group| group.name == name)
        .unwrap()
}

/// Tools outside the virtual `Delay_32` group.
fn tools(dataset: &Dataset) -> u32 {
    dataset
        .tool_groups
        .iter()
        .filter(|group| group.name != "Delay_32")
        .map(|group| group.tools)
        .sum()
}

fn route_lengths(dataset: &Dataset) -> Vec<usize> {
    dataset
        .parts
        .iter()
        .map(|part| dataset.routes[part.route].steps.len())
        .collect()
}

#[test]
#[ignore = "needs the SMT2020 data in data/raw"]
fn dataset_1_hvlm() {
    let ds = load("dataset 1", "HVLM_Model");
    assert_eq!((ds.tool_groups.len(), tools(&ds)), (106, 1_043));
    assert_eq!(route_lengths(&ds), [583, 343]);
    assert_eq!((ds.streams.len(), ds.lots.len()), (5, 2_256));
    assert_eq!((ds.setups.len(), ds.setup_changes.len()), (57, 13));

    let steps = &ds.routes[ds.parts[0].route].steps;
    assert_eq!(
        (steps[0].unit, steps[0].batch),
        (Unit::Batch, Some(BatchSize { min: 125, max: 150 }))
    );
    // 501.33 ± 25.0665 min
    assert_eq!(
        steps[0].time,
        Dist::Uniform {
            mean: 30_079_800,
            half_width: 1_503_990
        }
    );
    // Step 13 dedicates its lens to step 113.
    assert_eq!(steps[12].dedicate_to, Some(112));
    // Step 67 sends 1.7 % of lots back to step 65.
    let rework = steps[66].rework.unwrap();
    assert_eq!(rework.to, 64);
    assert!((rework.probability - 0.017).abs() < 1e-12);
    // Step 96: 82.584 ± 4.1292 min, cascading every 70.1964 min.
    assert_eq!(ds.tool_groups[steps[95].tool_group].name, "TF_FE_131");
    assert_eq!(
        steps[95].time,
        Dist::Uniform {
            mean: 4_955_040,
            half_width: 247_752
        }
    );
    assert_eq!(steps[95].cascade_interval, Some(4_211_784));

    let implant = group(&ds, "Implant_128");
    assert_eq!(
        (
            implant.tools,
            implant.cascading,
            implant.rule,
            implant.wake_least_setup
        ),
        (10, true, Rule::SetupRun(0), true)
    );
    assert_eq!(
        implant.ranks,
        [Rank::Priority, Rank::LeastSetup, Rank::Fifo]
    );
    let week = Dist::Exponential { mean: 7 * DAY };
    // Implant area MTTR 604.8 min.
    let ttr = Dist::Exponential { mean: 36_288_000 };
    assert_eq!(
        implant.breakdowns,
        [Breakdown {
            first: week,
            ttf: week,
            ttr
        }]
    );
    assert_eq!(implant.pms.len(), 3);
    // attach.txt: one breakdown calendar per area (all groups but Delay_32), 292 PM calendars.
    let breakdowns: usize = ds
        .tool_groups
        .iter()
        .map(|group| group.breakdowns.len())
        .sum();
    let pms: usize = ds.tool_groups.iter().map(|group| group.pms.len()).sum();
    assert_eq!((breakdowns, pms), (105, 292));
    assert_eq!(ds.setup_groups[0].name, "Implant_Gas");
    assert!(ds.setup_groups[0].min_run.iter().all(|&(_, run)| run == 7));
    assert_eq!(ds.setup_groups[0].min_run.len(), 9);

    let delay = group(&ds, "Delay_32");
    assert_eq!(
        (
            delay.tools,
            ds.locations[delay.location].as_str(),
            delay.load
        ),
        (400, "Delay", 0)
    );
    assert!(delay.breakdowns.is_empty() && delay.pms.is_empty());
    assert_eq!(
        group(&ds, "Diffusion_FE_120").batching,
        Some(BatchCriterion::SameRouteStep)
    );
    // Monthly PM: FOA 27.3 days, 13.76 ± 2.75 h.
    assert!(group(&ds, "DefMet_BE_33").pms.contains(&Pm {
        trigger: PmTrigger::Calendar {
            interval: 30 * DAY,
            first: 2_358_720_000
        },
        duration: Dist::Uniform {
            mean: 49_536_000,
            half_width: 9_900_000
        },
    }));
    // Weekly counter PM: 8.74 ± 1.75 h.
    assert!(group(&ds, "DE_BE_11").pms.contains(&Pm {
        trigger: PmTrigger::Wafers {
            interval: 2_000,
            first: 1_880
        },
        duration: Dist::Uniform {
            mean: 31_464_000,
            half_width: 6_300_000
        },
    }));

    // Lot_3: every 51.69 min, due 02/23/18 20:07:47.
    assert_eq!(
        ds.streams[0],
        ReleaseStream {
            part: 0,
            priority: 10,
            wafers: 25,
            start: 0,
            interval: 3_101_400,
            count: 200_000,
            lots: 1,
            due_offset: 53 * DAY + 20 * HOUR + 7 * MINUTE + 47 * SECOND,
            reserve: false,
        }
    );
    // Init_Lot_3_1 waits at step 581.
    assert_eq!(
        ds.lots[0],
        LotRelease {
            part: 0,
            priority: 10,
            wafers: 25,
            start: 0,
            due: 0,
            reserve: false,
            step: Some(580)
        }
    );
    let change = ds.setup_changes[0];
    assert_eq!(
        (
            ds.setups[change.from.unwrap()].as_str(),
            ds.setups[change.to].as_str(),
            change.time
        ),
        ("DE_BE_13_1", "DE_BE_13_2", Dist::Constant(7 * MINUTE))
    );
    let transport = ds.transports[0];
    assert_eq!(ds.transports.len(), 1);
    assert_eq!(
        (
            ds.locations[transport.from].as_str(),
            ds.locations[transport.to].as_str()
        ),
        ("Fab", "Fab")
    );
    assert_eq!(
        transport.time,
        Dist::Uniform {
            mean: 450_000,
            half_width: 150_000
        }
    );
    let periods: Vec<_> = ds.periods.iter().map(|p| (p.start, p.reset)).collect();
    assert_eq!(periods.len(), 8);
    assert_eq!(periods[..2], [(0, true), (365 * DAY, false)]);

    // Order labels of the AutoSched reports; the initial WIP mixes parts and is left out.
    let orders = asd::orders(&model_dir("dataset 1", "HVLM_Model")).unwrap();
    assert_eq!(orders.len(), 5);
    assert!(orders.contains(&Order {
        name: "O_SuperHotLot_3".into(),
        part: "part_3".into(),
        priority: 30,
    }));
}

#[test]
#[ignore = "needs the SMT2020 data in data/raw"]
fn dataset_2_lvhm() {
    let ds = load("dataset 2", "LVHM_Model");
    assert_eq!((ds.tool_groups.len(), tools(&ds)), (106, 913));
    assert_eq!(
        route_lengths(&ds),
        [521, 529, 583, 343, 242, 293, 353, 375, 384, 390]
    );
    assert_eq!((ds.streams.len(), ds.lots.len()), (0, 169_285));
    assert_eq!((ds.setups.len(), ds.setup_changes.len()), (177, 13));
    assert!(
        ds.tool_groups
            .iter()
            .all(|group| group.ranks == [Rank::Priority, Rank::LeastSetup, Rank::CriticalRatio])
    );
    let segments = ds
        .routes
        .iter()
        .flat_map(|route| &route.steps)
        .filter(|step| step.cqt.is_some());
    assert_eq!(segments.count(), 264);
    // Product 3: 2 h from the end of step 30 to the start of step 31.
    assert_eq!(
        ds.routes[ds.parts[2].route].steps[29].cqt,
        Some(Cqt {
            until: 30,
            limit: 2 * HOUR
        })
    );
    // Lot_1, due 02/13/18 04:23:56.
    assert_eq!(
        ds.lots[0],
        LotRelease {
            part: 0,
            priority: 10,
            wafers: 25,
            start: 0,
            due: 43 * DAY + 4 * HOUR + 23 * MINUTE + 56 * SECOND,
            reserve: false,
            step: None,
        }
    );

    // The inactive periodic plan: 21 streams (10 regular, 10 hot, 1 super hot) and the WIP.
    let periodic = asd::load_with_orders(
        &model_dir("dataset 2", "LVHM_Model"),
        &["order.txt", "WIP.txt"],
    )
    .unwrap();
    assert_eq!((periodic.streams.len(), periodic.lots.len()), (21, 2_156));
}

#[test]
#[ignore = "needs the SMT2020 data in data/raw"]
fn dataset_3_hvlm_e() {
    let ds = load("dataset 3", "HVLM_E_Model");
    assert_eq!((ds.tool_groups.len(), tools(&ds)), (106, 1_135));
    assert_eq!(route_lengths(&ds), [583, 343, 583]);
    let parts: Vec<_> = ds
        .parts
        .iter()
        .map(|p| (p.family.as_str(), p.engineering))
        .collect();
    assert_eq!(
        parts,
        [
            ("product_3", false),
            ("product_4", false),
            ("product_3", true)
        ]
    );
    assert_eq!((ds.streams.len(), ds.lots.len()), (5, 19_449));
    assert_eq!((ds.setups.len(), ds.setup_changes.len()), (93, 89));
    assert_eq!(
        group(&ds, "Diffusion_FE_120").batching,
        Some(BatchCriterion::SameFamilyStepName)
    );
    let litho = group(&ds, "LithoTrack_FE_95");
    assert_eq!((litho.tools, litho.wake_least_setup), (53, true));
    assert_eq!(litho.ranks, [Rank::Priority, Rank::Fifo]);
    // Engineering calibration: setups on 36 litho and planarization steps of route E3 are always done.
    let engineering = &ds.routes[ds.parts[2].route].steps;
    let always = engineering
        .iter()
        .filter(|step| step.setup.is_some_and(|setup| setup.always));
    assert_eq!(always.count(), 36);
}

#[test]
#[ignore = "needs the SMT2020 data in data/raw"]
fn dataset_4_lvhm_e() {
    let ds = load("dataset 4", "LVHM_E_Model");
    assert_eq!((ds.tool_groups.len(), tools(&ds)), (106, 1_068));
    assert_eq!(
        route_lengths(&ds),
        [
            521, 529, 583, 343, 242, 293, 353, 375, 384, 390, 521, 529, 583
        ]
    );
    assert_eq!((ds.streams.len(), ds.lots.len()), (0, 203_744));
    assert_eq!((ds.setups.len(), ds.setup_changes.len()), (276, 272));
    // Dataset files decode to the same dataset.
    assert_eq!(Dataset::from_bytes(&ds.to_bytes()).unwrap(), ds);
    // First active order file is E_order_high_SL_92PCTL.txt: E_Lot_1_1 of part_E1, due
    // 02/19/18 01:01:04.
    assert_eq!(
        ds.lots[0],
        LotRelease {
            part: 10,
            priority: 10,
            wafers: 1,
            start: 8 * HOUR,
            due: 49 * DAY + HOUR + MINUTE + 4 * SECOND,
            reserve: false,
            step: None,
        }
    );
}

/// The web page's file of dataset `n` (`www/data/ds{n}.bin`).
fn page_dataset(n: usize) -> Vec<u8> {
    let path: PathBuf = [
        env!("CARGO_MANIFEST_DIR"),
        "../../www/data",
        &format!("ds{n}.bin"),
    ]
    .iter()
    .collect();
    fs::read(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

/// The page's dataset files decode with this build and encode back to the same bytes.
#[test]
fn page_datasets_decode() {
    for n in 1..=4 {
        let bytes = page_dataset(n);
        let dataset =
            Dataset::from_bytes(&bytes).unwrap_or_else(|error| panic!("ds{n}.bin: {error}"));
        assert!(
            dataset.to_bytes() == bytes,
            "ds{n}.bin re-encodes differently"
        );
    }
}

/// The page's dataset files are the conversions of the raw models.
#[test]
#[ignore = "needs the SMT2020 data in data/raw"]
fn page_datasets_match_raw() {
    let models = [
        ("dataset 1", "HVLM_Model"),
        ("dataset 2", "LVHM_Model"),
        ("dataset 3", "HVLM_E_Model"),
        ("dataset 4", "LVHM_E_Model"),
    ];
    for (n, (dataset, model)) in (1..).zip(models) {
        assert!(
            load(dataset, model).to_bytes() == page_dataset(n),
            "ds{n}.bin is not the conversion of {dataset}: run smt2020 convert again"
        );
    }
}

/// Pausing leaves a run unchanged: the page's DS1 at day 5, reached at once or in steps.
#[test]
fn paused_runs_reach_the_same_state() {
    let data = Arc::new(Dataset::from_bytes(&page_dataset(1)).unwrap());
    let config = Config::new(30 * DAY);
    let mut straight = Simulation::new(Arc::clone(&data), config.clone()).unwrap();
    straight.run(Some(5 * DAY)).unwrap();
    let mut stepped = Simulation::new(data, config).unwrap();
    stepped.run(Some(DAY + 7 * HOUR)).unwrap();
    stepped
        .run_observed(None, |progress| {
            if progress.now == 3 * DAY {
                Break(())
            } else {
                Continue(())
            }
        })
        .unwrap();
    stepped.run(Some(5 * DAY)).unwrap();
    assert_eq!(stepped.progress(), straight.progress());
    assert!(stepped.progress().wip > 2_000);
    assert_eq!(stepped.lots(), straight.lots());
    assert_eq!(stepped.tools(), straight.tools());
    assert_eq!(stepped.tool_groups(), straight.tool_groups());
}

/// Runs `config`; every lot of the plan must complete.
fn complete(dataset: &Arc<Dataset>, config: &Config) -> Results {
    let results = Simulation::new(Arc::clone(dataset), config.clone())
        .and_then(|mut simulation| {
            simulation.run(None)?;
            simulation.results()
        })
        .unwrap_or_else(|error| panic!("{error}"));
    assert!(results.released > 0);
    assert_eq!(results.completed, results.released);
    assert_eq!(
        results.periods.last().map(|period| period.name.as_str()),
        Some("Drain")
    );
    results
}

/// The papers' two-year runs ([P1] §V, [P2] §4.2).
fn plan_completes(dataset: &str, model: &str) {
    complete(&Arc::new(load(dataset, model)), &Config::new(730 * DAY));
}

#[test]
#[ignore = "needs the SMT2020 data in data/raw"]
fn dataset_1_plan_completes() {
    plan_completes("dataset 1", "HVLM_Model");
}

#[test]
#[ignore = "needs the SMT2020 data in data/raw"]
fn dataset_2_plan_completes() {
    plan_completes("dataset 2", "LVHM_Model");
}

#[test]
#[ignore = "needs the SMT2020 data in data/raw"]
fn dataset_3_plan_completes() {
    plan_completes("dataset 3", "HVLM_E_Model");
}

#[test]
#[ignore = "needs the SMT2020 data in data/raw"]
fn dataset_4_plan_completes() {
    plan_completes("dataset 4", "LVHM_E_Model");
}

/// Every strategy on the dataset with CQT segments and engineering lots.
#[test]
#[ignore = "needs the SMT2020 data in data/raw"]
fn strategies_complete() {
    let ds = Arc::new(load("dataset 4", "LVHM_E_Model"));
    let base = Config::new(180 * DAY);
    let flow_factors = complete(&ds, &base).step_flow_factors;
    // Stepper limits low enough to hold lots in the default CQT segments ([P2] Table 3 shape).
    let limits = Limits {
        front: 5,
        total: 10,
    };
    let stopping = Stopping {
        limits: BTreeMap::from([
            ("LithoTrack_FE_95".into(), limits),
            ("LithoTrack_FE_115".into(), limits),
        ]),
        default: Limits::default(),
    };
    let strategies = [
        Config {
            queue_time: QueueTimeRule::Qtcr,
            stopping: Some(stopping),
            engineering: EngineeringRule::EngineeringFirst,
            ..base.clone()
        },
        Config {
            queue_time: QueueTimeRule::Qts,
            flow_factors: Some(flow_factors.clone()),
            engineering: EngineeringRule::Cate {
                production: 19 * HOUR + 12 * MINUTE,
                engineering: 4 * HOUR + 48 * MINUTE,
            },
            ..base.clone()
        },
        Config {
            engineering: EngineeringRule::Cot { trigger: 10 },
            ..base.clone()
        },
    ];
    for config in &strategies {
        complete(&ds, config);
    }
    // QTS without flow factors takes them from its own pass without the rule: the BASE run's.
    let qts = Config {
        queue_time: QueueTimeRule::Qts,
        ..base
    };
    let given = Config {
        flow_factors: Some(flow_factors),
        ..qts.clone()
    };
    assert_eq!(complete(&ds, &qts).digest(), complete(&ds, &given).digest());
}
