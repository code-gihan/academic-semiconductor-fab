//! JavaScript API of the SMT2020 simulator: decode a dataset file, run a configuration, summarize
//! replications. Configurations, progress, results and summaries cross as plain objects in the
//! serialized schema of the `smt2020` crate (times in ms). A run blocks its thread: pages run it in
//! a Web Worker, one replication per worker.

use std::ops::ControlFlow;

use js_sys::Function;
use serde::Serialize;
use serde::de::DeserializeOwned;
use smt2020::report::{self, Summary};
use smt2020::sim::{self, Results};
use wasm_bindgen::prelude::*;

/// A decoded dataset file.
#[wasm_bindgen]
pub struct Dataset(smt2020::Dataset);

#[wasm_bindgen]
impl Dataset {
    /// Decodes a dataset file (`smt2020 convert` output) of this build's format version.
    #[wasm_bindgen(constructor)]
    pub fn new(bytes: &[u8]) -> Result<Dataset, JsError> {
        smt2020::Dataset::from_bytes(bytes)
            .map(Dataset)
            .map_err(|error| JsError::new(&error.to_string()))
    }
}

/// Runs `config` on `dataset` until every released lot is complete and returns the results.
/// `onProgress(progress)`, if given, is called once per simulated day; returning `false` cancels
/// the run, and a thrown error ends it.
#[wasm_bindgen]
pub fn run(
    dataset: &Dataset,
    config: JsValue,
    #[wasm_bindgen(js_name = onProgress)] on_progress: Option<Function>,
) -> Result<JsValue, JsValue> {
    let config: smt2020::Config = from_js(config)?;
    let mut thrown = None;
    let outcome = sim::run_observed(&dataset.0, &config, |progress| {
        let Some(callback) = &on_progress else {
            return ControlFlow::Continue(());
        };
        let reply = to_js(progress).and_then(|progress| callback.call1(&JsValue::NULL, &progress));
        match reply {
            Ok(reply) if reply.as_bool() == Some(false) => ControlFlow::Break(()),
            Ok(_) => ControlFlow::Continue(()),
            Err(error) => {
                thrown = Some(error);
                ControlFlow::Break(())
            }
        }
    });
    match (outcome, thrown) {
        (_, Some(error)) => Err(error),
        (Ok(results), None) => to_js(&results),
        (Err(error), None) => Err(JsError::new(&error.to_string()).into()),
    }
}

/// Measures of results of one configuration's replications, with their means and 95%
/// confidence intervals.
#[wasm_bindgen]
pub fn summarize(results: JsValue) -> Result<JsValue, JsValue> {
    let results: Vec<Results> = from_js(results)?;
    to_js(&report::summarize(&results))
}

/// Summaries as CSV.
#[wasm_bindgen]
pub fn csv(summaries: JsValue) -> Result<String, JsValue> {
    let summaries: Vec<Summary> = from_js(summaries)?;
    Ok(report::csv(&summaries))
}

/// Digest of results: equal digests mean bit-identical results.
#[wasm_bindgen]
pub fn digest(results: JsValue) -> Result<String, JsValue> {
    let results: Results = from_js(results)?;
    Ok(results.digest())
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
