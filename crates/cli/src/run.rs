//! `run`: replications of one configuration in parallel threads, their summary table and the
//! JSON and CSV outputs.

use std::collections::BTreeMap;
use std::error::Error;
use std::fs;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering::Relaxed};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Instant;

use serde::Serialize;
use smt2020::report::{self, Measure, Scope, Summary};
use smt2020::sim::{self, EngineeringRule, Limits, QueueTimeRule, Stopping};
use smt2020::{Config, DAY, Dataset, HOUR, Results, Simulation, Time, asd};

use crate::heap::Counting;

#[derive(clap::Args)]
pub struct Args {
    /// Dataset file (`convert` output) or AutoSched model directory (`*.asd`).
    data: PathBuf,
    /// JSON configuration in the schema every interface shares; the options below override it.
    #[arg(long)]
    config: Option<PathBuf>,
    /// Horizon in days [default: 730].
    #[arg(long)]
    horizon: Option<f64>,
    /// Warm-up in days: reporting periods WarmUp and Period_1 instead of the dataset's.
    #[arg(long)]
    warm_up: Option<f64>,
    #[arg(long)]
    seed: Option<u64>,
    /// Release rate factor over the dataset plan.
    #[arg(long)]
    load: Option<f64>,
    /// Super hot lots reserve their next tool.
    #[arg(long)]
    reserve_super_hot: bool,
    /// Queue-time rule: none, qtcr or qts.
    #[arg(long)]
    queue_time: Option<String>,
    /// Hours of queue-time slack at which a batch below its minimum size starts.
    #[arg(long)]
    batch_start_within: Option<f64>,
    /// Stopping limits of a tool group, repeatable.
    #[arg(long, value_name = "GROUP=FRONT/TOTAL")]
    stopping: Vec<String>,
    /// Stopping limits of the other tool groups [default: 1000/1000].
    #[arg(long, value_name = "FRONT/TOTAL")]
    stopping_default: Option<String>,
    /// base, engineering_first, cate:PRODUCTION_H/ENGINEERING_H or cot:TRIGGER.
    #[arg(long)]
    engineering: Option<String>,
    /// Replications, numbered on from the configuration's replication.
    #[arg(long, default_value_t = 1)]
    replications: u32,
    /// Worker threads [default: available parallelism].
    #[arg(long)]
    threads: Option<usize>,
    /// Reporting period of the printed table [default: the last before Drain].
    #[arg(long)]
    period: Option<String>,
    /// Writes the configurations, results, digests, timings and summary.
    #[arg(long)]
    json: Option<PathBuf>,
    /// Writes the summary.
    #[arg(long)]
    csv: Option<PathBuf>,
}

pub fn run(args: &Args) -> Result<(), Box<dyn Error>> {
    let dataset = Arc::new(load(&args.data)?);
    let config = config(args)?;
    let threads = threads(args.threads);
    let started = Instant::now();
    let replications = replicate(&dataset, &config, args.replications, threads)?;
    let seconds = started.elapsed().as_secs_f64();
    let results: Vec<Results> = replications
        .iter()
        .map(|replication| replication.results.clone())
        .collect();
    let summary = report::summarize(&results);

    println!(
        "{}: {} replication(s), {threads} thread(s), {seconds:.1} s, peak heap {:.1} MB",
        args.data.display(),
        replications.len(),
        Counting::peak() as f64 / 1e6
    );
    for replication in &replications {
        let results = &replication.results;
        println!(
            "  replication {}: {:.1} s, {} events ({:.2} M/s), digest {}",
            replication.config.replication,
            replication.seconds,
            results.events,
            results.events as f64 / replication.seconds / 1e6,
            results.digest()
        );
    }
    let period = match &args.period {
        Some(period) => period.clone(),
        None => last_period(&results[0]).to_owned(),
    };
    print_table(&summary, &period);

    if let Some(path) = &args.json {
        let output = Output {
            data: args.data.display().to_string(),
            threads,
            peak_heap_bytes: Counting::peak(),
            replications: replications
                .iter()
                .map(|replication| ReplicationOutput {
                    config: &replication.config,
                    digest: replication.results.digest(),
                    seconds: replication.seconds,
                    results: &replication.results,
                })
                .collect(),
            summary: &summary,
        };
        let mut writer = BufWriter::new(fs::File::create(path)?);
        serde_json::to_writer(&mut writer, &output)?;
        writer.flush()?;
    }
    if let Some(path) = &args.csv {
        fs::write(path, report::csv(&summary))?;
    }
    Ok(())
}

/// The JSON output, shared with the web page's download.
#[derive(Serialize)]
struct Output<'a> {
    data: String,
    threads: usize,
    peak_heap_bytes: usize,
    replications: Vec<ReplicationOutput<'a>>,
    summary: &'a [Summary],
}

#[derive(Serialize)]
struct ReplicationOutput<'a> {
    config: &'a Config,
    digest: String,
    seconds: f64,
    results: &'a Results,
}

/// A dataset file, or an AutoSched model directory.
fn load(path: &Path) -> Result<Dataset, Box<dyn Error>> {
    if path.is_dir() {
        Ok(asd::load(path)?)
    } else {
        Ok(Dataset::from_bytes(&fs::read(path)?)?)
    }
}

pub fn threads(requested: Option<usize>) -> usize {
    requested.unwrap_or_else(|| thread::available_parallelism().map_or(1, usize::from))
}

/// The configuration file, or the dataset's own rules over two years, with the options applied.
fn config(args: &Args) -> Result<Config, Box<dyn Error>> {
    let mut config = match &args.config {
        Some(path) => serde_json::from_str(&fs::read_to_string(path)?)?,
        None => Config::new(730 * DAY),
    };
    if let Some(horizon) = args.horizon {
        config.horizon = duration(horizon, DAY)?;
    }
    if let Some(warm_up) = args.warm_up {
        config.warm_up = Some(duration(warm_up, DAY)?);
    }
    if let Some(seed) = args.seed {
        config.seed = seed;
    }
    if let Some(load) = args.load {
        config.load = load;
    }
    config.reserve_super_hot |= args.reserve_super_hot;
    if let Some(rule) = &args.queue_time {
        config.queue_time = match rule.as_str() {
            "none" => QueueTimeRule::None,
            "qtcr" => QueueTimeRule::Qtcr,
            "qts" => QueueTimeRule::Qts,
            _ => return Err(format!("unknown queue-time rule {rule}").into()),
        };
    }
    if let Some(hours) = args.batch_start_within {
        config.batch_start_within = Some(duration(hours, HOUR)?);
    }
    if !args.stopping.is_empty() || args.stopping_default.is_some() {
        let mut limits = BTreeMap::new();
        for spec in &args.stopping {
            let (group, pair) = spec
                .split_once('=')
                .ok_or_else(|| format!("stopping {spec} is not GROUP=FRONT/TOTAL"))?;
            limits.insert(group.to_owned(), self::limits(pair)?);
        }
        let default = match &args.stopping_default {
            Some(pair) => self::limits(pair)?,
            None => Limits::default(),
        };
        config.stopping = Some(Stopping { limits, default });
    }
    if let Some(rule) = &args.engineering {
        config.engineering = engineering(rule)?;
    }
    Ok(config)
}

/// `value` units of `unit` in whole ms.
fn duration(value: f64, unit: Time) -> Result<Time, String> {
    let ms = value * unit as f64;
    if ms.is_finite() && ms.abs() < Time::MAX as f64 {
        Ok(ms.round() as Time)
    } else {
        Err(format!("{value} is out of range"))
    }
}

fn limits(pair: &str) -> Result<Limits, String> {
    let parsed = pair
        .split_once('/')
        .and_then(|(front, total)| Some((front.parse().ok()?, total.parse().ok()?)));
    let (front, total) =
        parsed.ok_or_else(|| format!("stopping limits {pair} are not FRONT/TOTAL"))?;
    Ok(Limits { front, total })
}

fn engineering(rule: &str) -> Result<EngineeringRule, String> {
    let invalid = || format!("unknown engineering rule {rule}");
    Ok(match rule.split_once(':') {
        None if rule == "base" => EngineeringRule::Base,
        None if rule == "engineering_first" => EngineeringRule::EngineeringFirst,
        Some(("cate", hours)) => {
            let (production, engineering) = hours.split_once('/').ok_or_else(invalid)?;
            let hours = |text: &str| text.parse::<f64>().map_err(|_| invalid());
            EngineeringRule::Cate {
                production: duration(hours(production)?, HOUR)?,
                engineering: duration(hours(engineering)?, HOUR)?,
            }
        }
        Some(("cot", trigger)) => EngineeringRule::Cot {
            trigger: trigger.parse().map_err(|_| invalid())?,
        },
        _ => return Err(invalid()),
    })
}

/// One replication's configuration, results and run time.
pub struct Replication {
    pub config: Config,
    pub results: Results,
    pub seconds: f64,
}

/// Runs replications `config.replication`.. `+ count` on `threads` threads, each taking the next
/// replication when done; reports every completed replication on stderr.
pub fn replicate(
    dataset: &Arc<Dataset>,
    config: &Config,
    count: u32,
    threads: usize,
) -> Result<Vec<Replication>, sim::Error> {
    let next = AtomicU32::new(0);
    let failed = AtomicBool::new(false);
    let done = Mutex::new(Vec::new());
    thread::scope(|scope| {
        for _ in 0..threads.clamp(1, count.max(1) as usize) {
            scope.spawn(|| {
                while !failed.load(Relaxed) {
                    let index = next.fetch_add(1, Relaxed);
                    if index >= count {
                        break;
                    }
                    let config = Config {
                        replication: config.replication + index,
                        ..config.clone()
                    };
                    let started = Instant::now();
                    let outcome = Simulation::new(Arc::clone(dataset), config.clone()).and_then(
                        |mut simulation| {
                            simulation.run(None)?;
                            simulation.results()
                        },
                    );
                    let seconds = started.elapsed().as_secs_f64();
                    failed.fetch_or(outcome.is_err(), Relaxed);
                    eprintln!("replication {} done in {seconds:.1} s", config.replication);
                    let replication = outcome.map(|results| Replication {
                        config,
                        results,
                        seconds,
                    });
                    done.lock().expect("no panics").push((index, replication));
                }
            });
        }
    });
    let mut done = done.into_inner().expect("no panics");
    done.sort_by_key(|(index, _)| *index);
    done.into_iter()
        .map(|(_, replication)| replication)
        .collect()
}

/// The window ending at the horizon: the last reported period before the drain.
fn last_period(results: &Results) -> &str {
    let periods = &results.periods;
    let before_drain = periods.len().saturating_sub(2);
    periods.get(before_drain).map_or("", |period| &period.name)
}

/// Mean, with the 95% confidence interval half-width of several replications.
pub fn cell(summary: Option<&Summary>, decimals: usize) -> String {
    match summary {
        None => "-".into(),
        Some(Summary {
            mean,
            ci95: Some(ci95),
            ..
        }) => format!("{mean:.decimals$} ±{ci95:.decimals$}"),
        Some(Summary { mean, .. }) => format!("{mean:.decimals$}"),
    }
}

fn print_table(summary: &[Summary], period: &str) {
    let find = |scope, item: &str, kind, measure| {
        summary.iter().find(|row| {
            row.period == period
                && row.scope == scope
                && row.item == item
                && row.kind == kind
                && row.measure == measure
        })
    };
    println!("\n{period}");
    println!(
        "  {:<6}{:>12}{:>16}{:>14}{:>14}{:>14}",
        "kind", "completed", "ACT (d)", "CT std (d)", "on time (%)", "FF mean"
    );
    for row in summary
        .iter()
        .filter(|row| row.period == period && row.scope == Scope::Kind)
        .filter(|row| row.measure == Measure::Completed)
    {
        let measure = |measure| find(Scope::Kind, "", row.kind, measure);
        println!(
            "  {:<6}{:>12}{:>16}{:>14}{:>14}{:>14}",
            row.kind.map_or("", |kind| kind.name()),
            cell(Some(row), 0),
            cell(measure(Measure::CtMeanD), 2),
            cell(measure(Measure::CtStdD), 2),
            cell(measure(Measure::OnTimePct), 1),
            cell(measure(Measure::FfMean), 3),
        );
    }
    println!(
        "  WIP {} lots",
        cell(find(Scope::Fab, "", None, Measure::Wip), 0)
    );
    if find(Scope::Cqt, "total", None, Measure::VlPct).is_some() {
        println!(
            "  {:<6}{:>12}{:>16}{:>14}{:>14}",
            "CQT", "completed", "VL (%)", "AVL (h)", "AONT (h)"
        );
        for item in ["litho", "rest", "total"] {
            let measure = |measure| find(Scope::Cqt, item, None, measure);
            println!(
                "  {:<6}{:>12}{:>16}{:>14}{:>14}",
                item,
                cell(measure(Measure::Completed), 0),
                cell(measure(Measure::VlPct), 2),
                cell(measure(Measure::AvlH), 3),
                cell(measure(Measure::AontH), 3),
            );
        }
    }
}
