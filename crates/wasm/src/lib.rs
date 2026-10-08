//! JavaScript API of the SMT2020 simulator: the `smt2020` crate's dataset, simulation and
//! summaries. Configurations, progress, states and results cross as plain objects of the crate's
//! serialized schema (times in ms); strategy code is an object of JavaScript functions (`code`).
//! A run blocks its thread: pages run simulations in Web Workers.

mod code;

use std::ops::ControlFlow;
use std::sync::Arc;

use js_sys::{Array, Float64Array, Function, Object, Reflect, Uint8Array, Uint32Array};
use serde::Serialize;
use serde::de::DeserializeOwned;
use smt2020::report::{self, Comparison, Summary};
use smt2020::sim::{
    self, Activity, FoupPlace, Frame, LotKind, Player, Recording, Replay, Results, ToolState,
};
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

    /// Its AMHS layout for drawing (mm): nodes, rails, bays, tool and station footprints and port
    /// points; null without one.
    pub fn layout(&self) -> Result<JsValue, JsValue> {
        to_js(&self.0.layout.as_ref().map(|layout| layout.drawing()))
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

    /// The AMHS: its vehicles, the FOUPs at ports, in commit stations and in batch tools, the
    /// ports kept for FOUPs on their way, every tool's state and the transports waiting for a
    /// vehicle; null without a layout.
    pub fn amhs(&self) -> Result<JsValue, JsValue> {
        to_js(&self.0.amhs())
    }

    /// The AMHS replay recorded so far (`recording.replay`) in its compact form (a Uint8Array) for
    /// a `ReplayPlayer`; null without one or before its window began.
    pub fn replay(&self) -> JsValue {
        self.0.replay().map_or(JsValue::NULL, |replay| {
            Uint8Array::from(replay.bytes()).into()
        })
    }

    /// Results of the finished run.
    pub fn results(&self) -> Result<JsValue, JsValue> {
        to_js(&self.0.results().map_err(error)?)
    }
}

/// Plays a replay (`Simulation.replay()`) on the dataset it was recorded on: the fab at any
/// instant of its window.
#[wasm_bindgen]
pub struct ReplayPlayer {
    player: Player,
    frame: Frame,
    /// The names of activities, lot kinds, FOUP places and tool states, made once.
    activities: Vec<JsValue>,
    kinds: Vec<JsValue>,
    places: Vec<JsValue>,
    states: Vec<JsValue>,
}

#[wasm_bindgen]
impl ReplayPlayer {
    /// Reads and checks `replay`, recorded on `dataset`.
    #[wasm_bindgen(constructor)]
    pub fn new(dataset: &Dataset, replay: Vec<u8>) -> Result<ReplayPlayer, JsValue> {
        let replay = Replay::from_bytes(replay).map_err(error)?;
        let player = Player::new(Arc::clone(&dataset.0), replay).map_err(error)?;
        let names = |names: Vec<&str>| names.into_iter().map(JsValue::from_str).collect();
        Ok(Self {
            player,
            frame: Frame::default(),
            activities: names(Activity::ALL.iter().map(|each| each.name()).collect()),
            kinds: names(LotKind::ALL.iter().map(|each| each.name()).collect()),
            places: names(FoupPlace::ALL.iter().map(|each| each.name()).collect()),
            states: names(ToolState::ALL.iter().map(|each| each.name()).collect()),
        })
    }

    /// Its window: `{from, until}` (ms).
    pub fn window(&self) -> Result<JsValue, JsValue> {
        to_js(&self.player.window())
    }

    /// The fab at `time` (ms, within the window) in the core's schema, its number columns as
    /// typed arrays: `{time, vehicles: {x, y, heading, tail_x, tail_y, speed, activity}, foups:
    /// {lot, kind, place, index}, tools, delivered, tool_to_tool, carried}`.
    pub fn frame(&mut self, time: f64) -> Result<JsValue, JsValue> {
        self.player.frame_into(time, &mut self.frame);
        let frame = &self.frame;
        let floats = |column: &[f64]| JsValue::from(Float64Array::from(column));
        let vehicles = &frame.vehicles;
        let foups = &frame.foups;
        let lots: Vec<f64> = foups.lot.iter().map(|&lot| lot as f64).collect();
        // Enum variants are in their declaration order, as their names.
        object(&[
            ("time", JsValue::from_f64(frame.time)),
            (
                "vehicles",
                object(&[
                    ("x", floats(&vehicles.x)),
                    ("y", floats(&vehicles.y)),
                    ("heading", floats(&vehicles.heading)),
                    ("tail_x", floats(&vehicles.tail_x)),
                    ("tail_y", floats(&vehicles.tail_y)),
                    ("speed", floats(&vehicles.speed)),
                    (
                        "activity",
                        named(&vehicles.activity, &self.activities, |each| each as usize),
                    ),
                ])?,
            ),
            (
                "foups",
                object(&[
                    ("lot", floats(&lots)),
                    (
                        "kind",
                        named(&foups.kind, &self.kinds, |each| each as usize),
                    ),
                    (
                        "place",
                        named(&foups.place, &self.places, |each| each as usize),
                    ),
                    ("index", Uint32Array::from(&foups.index[..]).into()),
                ])?,
            ),
            (
                "tools",
                named(&frame.tools, &self.states, |each| each as usize),
            ),
            ("delivered", JsValue::from_f64(frame.delivered as f64)),
            ("tool_to_tool", JsValue::from_f64(frame.tool_to_tool as f64)),
            ("carried", JsValue::from_f64(frame.carried)),
        ])
    }
}

/// An array of the names of `values`, each by its `index` among `names`.
fn named<T: Copy>(values: &[T], names: &[JsValue], index: impl Fn(T) -> usize) -> JsValue {
    let array = Array::new();
    for &value in values {
        array.push(&names[index(value)]);
    }
    array.into()
}

/// A plain object of `fields`.
fn object(fields: &[(&str, JsValue)]) -> Result<JsValue, JsValue> {
    let object = Object::new();
    for (name, value) in fields {
        Reflect::set(&object, &JsValue::from_str(name), value)?;
    }
    Ok(object.into())
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
