//! `validate`: replications of a dataset's own rules against its AutoSched reference run, over the
//! reference's last reporting period (the cumulative window ending with the run).

use std::error::Error;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use smt2020::report::{self, Measure, Scope, Summary};
use smt2020::sim::LotKind;
use smt2020::{Config, DAY, asd};

use crate::reference::Report;
use crate::run::{cell, replicate, threads};

#[derive(clap::Args)]
pub struct Args {
    /// AutoSched model folder: the model directory (`*.asd`) and its reference reports (`*.rep`).
    model: PathBuf,
    /// Horizon in days: the reference run's length.
    #[arg(long, default_value_t = 1460.0)]
    horizon: f64,
    #[arg(long, default_value_t = 3)]
    replications: u32,
    #[arg(long, default_value_t = 1)]
    seed: u64,
    /// Worker threads [default: available parallelism].
    #[arg(long)]
    threads: Option<usize>,
    /// Writes every compared value.
    #[arg(long)]
    csv: Option<PathBuf>,
}

/// A model value over replications against the reference value.
struct Comparison {
    section: &'static str,
    item: String,
    measure: &'static str,
    model: Summary,
    reference: f64,
}

impl Comparison {
    fn difference(&self) -> f64 {
        self.model.mean - self.reference
    }

    fn relative(&self) -> f64 {
        100.0 * self.difference() / self.reference
    }
}

pub fn run(args: &Args) -> Result<(), Box<dyn Error>> {
    let asd = model_directory(&args.model)?;
    let dataset = Arc::new(asd::load(&asd)?);
    let orders = asd::orders(&asd)?;
    let config = Config {
        seed: args.seed,
        ..Config::new((args.horizon * DAY as f64).round() as i64)
    };
    let replications = replicate(&dataset, &config, args.replications, threads(args.threads))?;
    let results: Vec<_> = replications.into_iter().map(|run| run.results).collect();
    let summary = report::summarize(&results);

    let reports = |file| Report::read(&args.model, file);
    let (order_rep, family_rep, group_rep, perf_rep) = (
        reports("order.rep")?,
        reports("stnfam.rep")?,
        reports("stngrp.rep")?,
        reports("perf.rep")?,
    );
    let period = order_rep.last_period()?.to_owned();
    let model = |scope, item: &str, kind, measure| {
        summary
            .iter()
            .find(|row| {
                row.period == period
                    && row.scope == scope
                    && row.item == item
                    && row.kind == kind
                    && row.measure == measure
            })
            .cloned()
    };
    if model(Scope::Fab, "", None, Measure::Wip).is_none() {
        return Err(format!("the runs report no period {period}: extend the horizon").into());
    }
    let mut comparisons = Vec::new();
    let mut compare = |section, item: &str, measure, model: Option<Summary>, reference| {
        if let Some(model) = model {
            comparisons.push(Comparison {
                section,
                item: item.to_owned(),
                measure,
                model,
                reference,
            });
        }
    };

    for (_, values) in perf_rep.table(&period, "PERIOD", &["LOTCOMPS", "WIPLOTAVG"])? {
        let fab = |measure| model(Scope::Fab, "", None, measure);
        compare("fab", "", "completed", fab(Measure::Completed), values[0]);
        compare("fab", "", "wip", fab(Measure::Wip), values[1]);
    }

    // Orders: the reference keeps lots per order, each of one part and kind.
    let mut kinds: Vec<(LotKind, f64, f64)> = Vec::new();
    let columns = ["CYCLEAVG", "CYCLESTD", "ONTIME%"];
    for (name, values) in order_rep.table(&period, "ORDER", &columns)? {
        let Some(order) = orders.iter().find(|order| order.name == name) else {
            continue;
        };
        let engineering = dataset
            .parts
            .iter()
            .any(|part| part.name == order.part && part.engineering);
        let kind = LotKind::of(engineering, order.priority)
            .ok_or_else(|| format!("order {name}: no lot kind"))?;
        let lot = |measure| model(Scope::Lot, &order.part, Some(kind), measure);
        let item = format!("{} {} {}", name, order.part, kind.name());
        compare(
            "order",
            &item,
            "ct_mean_d",
            lot(Measure::CtMeanD),
            values[0] / 24.0,
        );
        compare(
            "order",
            &item,
            "ct_std_d",
            lot(Measure::CtStdD),
            values[1] / 24.0,
        );
        compare(
            "order",
            &item,
            "on_time_pct",
            lot(Measure::OnTimePct),
            values[2],
        );
        // Kind means of the reference weight its order means by the model's completions.
        if let Some(completed) = lot(Measure::Completed) {
            match kinds.iter_mut().find(|(known, ..)| *known == kind) {
                Some((_, weight, sum)) => {
                    *weight += completed.mean;
                    *sum += completed.mean * values[0] / 24.0;
                }
                None => kinds.push((kind, completed.mean, completed.mean * values[0] / 24.0)),
            }
        }
    }
    for &(kind, weight, sum) in &kinds {
        let model = model(Scope::Kind, "", Some(kind), Measure::CtMeanD);
        compare("kind", kind.name(), "ct_mean_d", model, sum / weight);
    }

    let states = [
        ("DOWN%", Measure::DownPct),
        ("PM%", Measure::PmPct),
        ("SETUP%", Measure::SetupPct),
        ("PROC%", Measure::ProcessPct),
        ("UTIL%", Measure::UtilPct),
    ];
    let columns: Vec<&str> = states.iter().map(|(column, _)| *column).collect();
    for (group, values) in family_rep.table(&period, "STNFAM", &columns)? {
        for (&(_, measure), value) in states.iter().zip(values) {
            let model = model(Scope::ToolGroup, &group, None, measure);
            compare("tool_group", &group, measure.name(), model, value);
        }
    }
    for (area, values) in group_rep.table(&period, "STNGRP", &["DOWN%", "PM%", "UTIL%"])? {
        let measure = |measure| model(Scope::Area, &area, None, measure);
        let availability = 100.0 - values[0] - values[1];
        compare(
            "area",
            &area,
            "availability_pct",
            measure(Measure::AvailabilityPct),
            availability,
        );
        compare(
            "area",
            &area,
            "util_pct",
            measure(Measure::UtilPct),
            values[2],
        );
    }

    print(args, &period, results.len(), &comparisons);
    if let Some(path) = &args.csv {
        let mut csv = String::from("section,item,measure,n,model,ci95,reference,difference\n");
        for row in &comparisons {
            let ci95 = row.model.ci95.map(|ci| ci.to_string()).unwrap_or_default();
            let _ = writeln!(
                csv,
                "{},{},{},{},{},{ci95},{},{}",
                row.section,
                row.item,
                row.measure,
                row.model.n,
                row.model.mean,
                row.reference,
                row.difference()
            );
        }
        fs::write(path, csv)?;
    }
    Ok(())
}

/// The single model directory (`*.asd`) in the model folder.
fn model_directory(folder: &Path) -> Result<PathBuf, Box<dyn Error>> {
    let mut found = Vec::new();
    for entry in fs::read_dir(folder)? {
        let path = entry?.path();
        if path.is_dir() && path.extension().is_some_and(|extension| extension == "asd") {
            found.push(path);
        }
    }
    match found.as_slice() {
        [directory] => Ok(directory.clone()),
        _ => Err(format!("{}: needs exactly one *.asd directory", folder.display()).into()),
    }
}

fn print(args: &Args, period: &str, replications: usize, comparisons: &[Comparison]) {
    let rows = |section: &'static str| comparisons.iter().filter(move |row| row.section == section);
    println!(
        "{}: {period}, {replications} replication(s) of seed {} / AutoSched reference",
        args.model.display(),
        args.seed
    );
    for row in rows("fab") {
        println!(
            "  {:<10}{:>20}{:>12.1}{:>+9.1}%",
            row.measure,
            cell(Some(&row.model), 1),
            row.reference,
            row.relative()
        );
    }
    println!("\n  cycle time (d) by lot kind");
    for row in rows("kind") {
        println!(
            "  {:<10}{:>20}{:>12.2}{:>+9.1}%",
            row.item,
            cell(Some(&row.model), 2),
            row.reference,
            row.relative()
        );
    }
    println!("\n  orders: cycle time (d), its standard deviation (d), on time (%)");
    for row in rows("order") {
        println!(
            "  {:<34}{:<12}{:>18}{:>10.2}{:>+9.2}",
            row.item,
            row.measure,
            cell(Some(&row.model), 2),
            row.reference,
            row.difference()
        );
    }
    println!("\n  tool groups: difference in percentage points, mean and largest");
    let groups: Vec<&Comparison> = rows("tool_group").collect();
    for measure in ["down_pct", "pm_pct", "setup_pct", "process_pct", "util_pct"] {
        let of_measure: Vec<&&Comparison> =
            groups.iter().filter(|row| row.measure == measure).collect();
        let Some(largest) = of_measure
            .iter()
            .max_by(|a, b| a.difference().abs().total_cmp(&b.difference().abs()))
        else {
            continue;
        };
        let mean =
            of_measure.iter().map(|row| row.difference()).sum::<f64>() / of_measure.len() as f64;
        println!(
            "  {:<12}{:>+8.2}{:>+8.2} {} ({:.1} / {:.1})",
            measure,
            mean,
            largest.difference(),
            largest.item,
            largest.model.mean,
            largest.reference
        );
    }
    println!("\n  areas: availability (%), utilization (%)");
    for row in rows("area") {
        println!(
            "  {:<12}{:<18}{:>12.2}{:>10.2}{:>+8.2}",
            row.item,
            row.measure,
            row.model.mean,
            row.reference,
            row.difference()
        );
    }
}
