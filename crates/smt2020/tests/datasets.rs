//! Loads the four SMT2020 AutoSched datasets from `data/raw` (the extracted `SMT_2020 - Final`
//! folder, not committed): `cargo test -p smt2020 -- --ignored`. Expected values were read off
//! the raw files.

use std::path::PathBuf;

use des_core::{DAY, HOUR, MINUTE, SECOND};
use smt2020::asd;
use smt2020::data::{
    BatchCriterion, BatchSize, Breakdown, Cqt, Dataset, Dist, LotRelease, Pm, PmTrigger, Rank,
    ReleaseStream, Rule, ToolGroup, Unit,
};

fn load(dataset: &str, model: &str) -> Dataset {
    let dir: PathBuf = [
        env!("CARGO_MANIFEST_DIR"),
        "../../data/raw/AutoSched",
        dataset,
        model,
        &format!("{model}.asd"),
    ]
    .iter()
    .collect();
    asd::load(&dir).unwrap_or_else(|error| panic!("{error}"))
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
