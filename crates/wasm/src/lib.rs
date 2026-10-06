//! JavaScript API of the SMT2020 simulator: the `smt2020` crate's dataset, simulation and
//! summaries. Configurations, progress, states and results cross as plain objects of the crate's
//! serialized schema (times in ms); strategy code is an object of JavaScript functions (`code`).
//! A run blocks its thread: pages run simulations in Web Workers.

mod code;

use std::ops::ControlFlow;
use std::sync::Arc;

use js_sys::Function;
use serde::Serialize;
use serde::de::DeserializeOwned;
use smt2020::report::{self, Comparison, Summary};
use smt2020::sim::{self, Recording, Results};
use wasm_bindgen::prelude::*;

/// A decoded dataset file, shared by the simulations of it.
#[wasm_bindgen]
pub struct Dataset(Arc<smt2020::Dataset>);

#[wasm_bindgen]
impl Dataset {
    /// Decodes a dataset file (`smt2020 convert` output) of this build's format version.
    #[wasm_bindgen(constructor)]
    pub fn new(bytes: &[u8]) -> Result<Dataset, JsError> {
        smt2020::Dataset::from_bytes(bytes)
            .map(|dataset| Dataset(Arc::new(dataset)))
            .map_err(|error| JsError::new(&error.to_string()))
    }

    /// Its areas, tool groups, parts, routes, CQT segments and periods, by name and index.
    pub fn info(&self) -> Result<JsValue, JsValue> {
        to_js(&self.0.info())
    }
}

/// One run of a configuration on a dataset from time 0, advanced in steps (`smt2020::Simulation`).
#[wasm_bindgen]
pub struct Simulation(smt2020::Simulation);

#[wasm_bindgen]
impl Simulation {
    /// The simulation of `config` on `dataset` at time 0, recording what `recording` (optional)
    /// asks, with the strategy `code` (optional): an object with any of the functions
    /// `priority(lot, now)`, `admit(lot, segment, now)` and `startBatch(batch, now)`.
    #[wasm_bindgen(constructor)]
    pub fn new(
        dataset: &Dataset,
        config: JsValue,
        recording: JsValue,
        code: JsValue,
    ) -> Result<Simulation, JsValue> {
        let recording = if recording.is_undefined() || recording.is_null() {
            Recording::default()
        } else {
            json_of(recording)?
        };
        let config = json_of(config)?;
        let data = Arc::clone(&dataset.0);
        match code::code_of(&dataset.0, &code)? {
            Some(code) => smt2020::Simulation::with_code(data, config, recording, code),
            None => smt2020::Simulation::with_recording(data, config, recording),
        }
        .map(Simulation)
        .map_err(error)
    }

    pub fn config(&self) -> Result<JsValue, JsValue> {
        to_js(self.0.config())
    }

    pub fn recording(&self) -> Result<JsValue, JsValue> {
        to_js(self.0.recording())
    }

    /// The tables recorded so far.
    pub fn records(&self) -> Result<JsValue, JsValue> {
        to_js(self.0.records())
    }

    /// The QTS flow factors of the configured run (given, or measured by the first pass), or
    /// null.
    #[wasm_bindgen(js_name = flowFactors)]
    pub fn flow_factors(&self) -> Result<JsValue, JsValue> {
        to_js(&self.0.flow_factors())
    }

    /// Starts over at time 0 with `config`, or with the same configuration, and the same code.
    pub fn reset(&mut self, config: JsValue) -> Result<(), JsValue> {
        let config = if config.is_undefined() || config.is_null() {
            self.0.config().clone()
        } else {
            json_of(config)?
        };
        self.0.reset(config).map_err(error)
    }

    /// Runs up to `until` (ms, events at it included) or to the end and returns the progress.
    /// `onProgress(progress)`, if given, is called once per simulated day: returning `false`
    /// pauses the run there, and a thrown error pauses it and is thrown on.
    pub fn run(
        &mut self,
        until: Option<f64>,
        #[wasm_bindgen(js_name = onProgress)] on_progress: Option<Function>,
    ) -> Result<JsValue, JsValue> {
        let until = until.map(sim::time).transpose().map_err(error)?;
        let mut thrown = None;
        let progress = self.0.run_observed(until, |progress| {
            let Some(callback) = &on_progress else {
                return ControlFlow::Continue(());
            };
            match to_js(progress).and_then(|progress| callback.call1(&JsValue::NULL, &progress)) {
                Ok(reply) if reply.as_bool() == Some(false) => ControlFlow::Break(()),
                Ok(_) => ControlFlow::Continue(()),
                Err(error) => {
                    thrown = Some(error);
                    ControlFlow::Break(())
                }
            }
        });
        if let Some(error) = thrown {
            return Err(error);
        }
        to_js(&progress.map_err(error)?)
    }

    pub fn progress(&self) -> Result<JsValue, JsValue> {
        to_js(&self.0.progress())
    }

    /// The lots in the fab, by id.
    pub fn lots(&self) -> Result<JsValue, JsValue> {
        to_js(&self.0.lots())
    }

    /// Every tool, by id.
    pub fn tools(&self) -> Result<JsValue, JsValue> {
        to_js(&self.0.tools())
    }

    /// Every tool group, in dataset order.
    #[wasm_bindgen(js_name = toolGroups)]
    pub fn tool_groups(&self) -> Result<JsValue, JsValue> {
        to_js(&self.0.tool_groups())
    }

    /// Every CQT segment, in the order of the dataset info's segments: the lots in it and its
    /// completions so far.
    pub fn segments(&self) -> Result<JsValue, JsValue> {
        to_js(&self.0.segments())
    }

    /// Results of the finished run.
    pub fn results(&self) -> Result<JsValue, JsValue> {
        to_js(&self.0.results().map_err(error)?)
    }
}

/// Measures of results of one configuration's replications, with their means and 95%
/// confidence intervals.
#[wasm_bindgen]
pub fn summarize(results: JsValue) -> Result<JsValue, JsValue> {
    let results: Vec<Results> = from_js(results)?;
    to_js(&report::summarize(&results))
}

/// Day-by-day measures of results of one configuration's replications, with their means and 95%
/// confidence intervals.
#[wasm_bindgen]
pub fn daily(results: JsValue) -> Result<JsValue, JsValue> {
    let results: Vec<Results> = from_js(results)?;
    to_js(&report::daily(&results))
}

/// Summaries as CSV.
#[wasm_bindgen]
pub fn csv(summaries: JsValue) -> Result<String, JsValue> {
    let summaries: Vec<Summary> = from_js(summaries)?;
    Ok(report::csv(&summaries))
}

/// The measures of `other` against `baseline`, results of two configurations' replications
/// paired by seed and replication: the means and the mean difference with its 95% confidence
/// interval.
#[wasm_bindgen]
pub fn compare(baseline: JsValue, other: JsValue) -> Result<JsValue, JsValue> {
    let baseline: Vec<Results> = from_js(baseline)?;
    let other: Vec<Results> = from_js(other)?;
    to_js(&report::compare(&baseline, &other).map_err(error)?)
}

/// Comparisons as CSV.
#[wasm_bindgen(js_name = comparisonCsv)]
pub fn comparison_csv(comparisons: JsValue) -> Result<String, JsValue> {
    let comparisons: Vec<Comparison> = from_js(comparisons)?;
    Ok(report::comparison_csv(&comparisons))
}

/// Digest of results: equal digests mean bit-identical results.
#[wasm_bindgen]
pub fn digest(results: JsValue) -> Result<String, JsValue> {
    let results: Results = from_js(results)?;
    Ok(results.digest())
}

fn error(error: sim::Error) -> JsValue {
    JsError::new(&error.to_string()).into()
}

/// An input, read through a JSON value: struct deserialization reads only the known properties of
/// an object, so an unknown field would pass unnoticed instead of failing as it does in the other
/// interfaces.
fn json_of<T: DeserializeOwned>(value: JsValue) -> Result<T, JsValue> {
    let value: serde_json::Value = from_js(value)?;
    serde_json::from_value(value).map_err(|error| JsError::new(&error.to_string()).into())
}

fn from_js<T: DeserializeOwned>(value: JsValue) -> Result<T, JsValue> {
    serde_wasm_bindgen::from_value(value).map_err(JsValue::from)
}

/// Plain objects, `null` for none: the JSON form of the value.
fn to_js<T: Serialize + ?Sized>(value: &T) -> Result<JsValue, JsValue> {
    value
        .serialize(&serde_wasm_bindgen::Serializer::json_compatible())
        .map_err(JsValue::from)
}
