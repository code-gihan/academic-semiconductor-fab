//! `smt2020` command line: converts AutoSched models into dataset files, runs replications of a
//! configuration, and validates against the AutoSched reference runs.

mod heap;
mod reference;
mod run;
mod validate;

use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use clap::{Parser, Subcommand};
use smt2020::asd;

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
    },
    /// Runs replications of a configuration and summarizes them.
    Run(Box<run::Args>),
    /// Compares replications of a dataset's own rules with its AutoSched reference run.
    Validate(validate::Args),
}

fn main() -> ExitCode {
    let outcome = match Cli::parse().command {
        Command::Convert { asd, out, orders } => convert(&asd, &out, &orders),
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

fn convert(dir: &Path, out: &Path, orders: &[String]) -> Result<(), Box<dyn Error>> {
    let started = Instant::now();
    let dataset = if orders.is_empty() {
        asd::load(dir)?
    } else {
        let orders: Vec<&str> = orders.iter().map(String::as_str).collect();
        asd::load_with_orders(dir, &orders)?
    };
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
    Ok(())
}
