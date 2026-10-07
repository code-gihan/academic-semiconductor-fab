//! `smt2020` command line: converts AutoSched models (with a SMAT2022 AMHS layout) into dataset
//! files, runs replications of a configuration, and validates against the AutoSched reference
//! runs.

mod heap;
mod reference;
mod run;
mod validate;

use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use calamine::{Data, Reader, Xlsx, open_workbook};
use clap::{Parser, Subcommand};
use smt2020::{asd, smat};

#[global_allocator]
static ALLOCATOR: heap::Counting = heap::Counting;

#[derive(Parser)]
#[command(version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Converts an AutoSched model directory (`*.asd`) into a dataset file.
    Convert {
        /// AutoSched model directory.
        asd: PathBuf,
        /// Dataset file to write.
        out: PathBuf,
        /// Order files of the model directory replacing its active ORDER_FILES.
        #[arg(long, num_args = 1..)]
        orders: Vec<String>,
        /// SMAT2022 model file (xlsx) whose AMHS layout the dataset gets.
        #[arg(long)]
        layout: Option<PathBuf>,
    },
    /// Runs replications of a configuration and summarizes them.
    Run(Box<run::Args>),
    /// Compares replications of a dataset's own rules with its AutoSched reference run.
    Validate(validate::Args),
}

fn main() -> ExitCode {
    let outcome = match Cli::parse().command {
        Command::Convert {
            asd,
            out,
            orders,
            layout,
        } => convert(&asd, &out, &orders, layout.as_deref()),
        Command::Run(args) => run::run(&args),
        Command::Validate(args) => validate::run(&args),
    };
    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn convert(
    dir: &Path,
    out: &Path,
    orders: &[String],
    layout: Option<&Path>,
) -> Result<(), Box<dyn Error>> {
    let started = Instant::now();
    let mut dataset = if orders.is_empty() {
        asd::load(dir)?
    } else {
        let orders: Vec<&str> = orders.iter().map(String::as_str).collect();
        asd::load_with_orders(dir, &orders)?
    };
    if let Some(path) = layout {
        let sheets = smat_sheets(path)?;
        dataset.layout = Some(smat::layout(&dataset, &sheets)?);
    }
    let bytes = dataset.to_bytes();
    fs::write(out, &bytes)?;
    let tools: u32 = dataset.tool_groups.iter().map(|group| group.tools).sum();
    println!(
        "{}: {} parts, {} tool groups ({tools} tools), {} release streams, {} listed lots; {:.2} MB in {:.1} s",
        out.display(),
        dataset.parts.len(),
        dataset.tool_groups.len(),
        dataset.streams.len(),
        dataset.lots.len(),
        bytes.len() as f64 / 1e6,
        started.elapsed().as_secs_f64()
    );
    if let Some(layout) = &dataset.layout {
        println!(
            "AMHS layout: {} nodes, {} rails ({:.2} km), {} ZCUs, {} bays, {} ports, {} vehicles",
            layout.nodes.len(),
            layout.links.len(),
            layout.links.iter().map(|link| link.length).sum::<f64>() / 1e6,
            layout.zones.len(),
            layout.bays.len(),
            layout.ports.len(),
            layout.vehicles.len()
        );
    }
    Ok(())
}

/// The SMAT2022 sheets of the xlsx at `path`.
fn smat_sheets(path: &Path) -> Result<Vec<smat::Sheet>, Box<dyn Error>> {
    let mut workbook: Xlsx<_> = open_workbook(path)?;
    let mut sheets = Vec::new();
    for name in smat::SHEETS {
        let range = workbook
            .worksheet_range(name)
            .map_err(|error| format!("{}: sheet {name}: {error}", path.display()))?;
        let mut rows = Vec::with_capacity(range.height());
        for row in range.rows() {
            let mut cells = Vec::with_capacity(row.len());
            for cell in row {
                cells.push(match cell {
                    Data::Empty => smat::Cell::Empty,
                    Data::String(text) => smat::Cell::Text(text.clone()),
                    Data::Float(number) => smat::Cell::Number(*number),
                    Data::Int(number) => smat::Cell::Number(*number as f64),
                    Data::Bool(flag) => smat::Cell::Number(f64::from(u8::from(*flag))),
                    other => return Err(format!("{name}: unsupported cell {other:?}").into()),
                });
            }
            rows.push(cells);
        }
        sheets.push(smat::Sheet {
            name: name.to_owned(),
            rows,
        });
    }
    Ok(sheets)
}
