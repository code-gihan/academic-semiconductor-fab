//! Python module `smt2020`: the `smt2020` crate's dataset, simulation and summaries, as the
//! JavaScript module has them. Configurations, progress, states and results cross as dicts and
//! lists of the crate's serialized schema (times in ms). A run releases the GIL, so simulations
//! in several threads run in parallel, and Ctrl-C pauses it.

use std::fmt::Display;
use std::ops::ControlFlow;
use std::path::PathBuf;
use std::sync::Arc;

use pyo3::exceptions::{PyRuntimeError, PyValueError};
use pyo3::prelude::*;
use pythonize::{depythonize, pythonize};
use serde::Serialize;
use smt2020::report::{self, Comparison, Summary};
use smt2020::sim::{self, Config, Recording, Results};
use smt2020::{DAY, HOUR, MINUTE, SECOND, asd};

/// The web page's dataset files, bundled.
const BUNDLED: [(&str, &[u8]); 4] = [
    ("ds1", include_bytes!("../../../www/data/ds1.bin")),
    ("ds2", include_bytes!("../../../www/data/ds2.bin")),
    ("ds3", include_bytes!("../../../www/data/ds3.bin")),
    ("ds4", include_bytes!("../../../www/data/ds4.bin")),
];

/// A decoded dataset, shared by the simulations of it.
#[pyclass(frozen, module = "smt2020")]
struct Dataset(Arc<smt2020::Dataset>);

#[pymethods]
impl Dataset {
    /// Decodes a dataset file (`smt2020 convert` output) of this build's format version.
    #[new]
    fn new(bytes: &[u8]) -> PyResult<Self> {
        smt2020::Dataset::from_bytes(bytes)
            .map(|dataset| Self(Arc::new(dataset)))
            .map_err(value_error)
    }

    /// Its areas, tool groups, parts, routes, CQT segments and periods, by name and index.
    fn info<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        to_py(py, &self.0.info())
    }
}

/// The dataset `source` names: "ds1" to "ds4" are the bundled datasets, any other source is the
/// path of a dataset file or of an AutoSched model directory (`*.asd`).
#[pyfunction]
fn load_dataset(source: PathBuf) -> PyResult<Dataset> {
    if let Some((_, bytes)) = BUNDLED
        .iter()
        .find(|(name, _)| source.to_str() == Some(name))
    {
        return Dataset::new(bytes);
    }
    let dataset = if source.is_dir() {
        asd::load(&source).map_err(value_error)?
    } else {
        smt2020::Dataset::from_bytes(&std::fs::read(&source)?).map_err(value_error)?
    };
    Ok(Dataset(Arc::new(dataset)))
}

/// One run of a configuration on a dataset from time 0, advanced in steps (`smt2020::Simulation`).
#[pyclass(module = "smt2020")]
struct Simulation(smt2020::Simulation);

#[pymethods]
impl Simulation {
    /// The simulation of `config` on `dataset` at time 0, recording what `recording` asks.
    #[new]
    #[pyo3(signature = (dataset, config, recording=None))]
    fn new(
        dataset: &Dataset,
        config: &Bound<'_, PyAny>,
        recording: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Self> {
        let recording: Recording = match recording {
            Some(recording) => depythonize(recording).map_err(value_error)?,
            None => Recording::default(),
        };
        smt2020::Simulation::with_recording(Arc::clone(&dataset.0), config_of(config)?, recording)
            .map(Self)
            .map_err(value_error)
    }

    fn config<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        to_py(py, self.0.config())
    }

    fn recording<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        to_py(py, self.0.recording())
    }

    /// The tables recorded so far, as dicts of columns.
    fn records<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        to_py(py, self.0.records())
    }

    /// The QTS flow factors of the configured run (given, or measured by the first pass), or
    /// None.
    fn flow_factors<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        to_py(py, &self.0.flow_factors())
    }

    /// Starts over at time 0 with `config`, or with the same configuration.
    #[pyo3(signature = (config=None))]
    fn reset(&mut self, config: Option<&Bound<'_, PyAny>>) -> PyResult<()> {
        let config = match config {
            Some(config) => config_of(config)?,
            None => self.0.config().clone(),
        };
        self.0.reset(config).map_err(value_error)
    }

    /// Runs up to `until` (ms, events at it included) or to the end and returns the progress.
    /// `on_progress(progress)`, if given, is called once per simulated day: returning False pauses
    /// the run there, and an exception pauses it and is raised on, as Ctrl-C is.
    #[pyo3(signature = (until=None, on_progress=None))]
    fn run<'py>(
        &mut self,
        py: Python<'py>,
        until: Option<f64>,
        on_progress: Option<Py<PyAny>>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let until = until.map(sim::time).transpose().map_err(value_error)?;
        let simulation = &mut self.0;
        let mut raised = None;
        let progress = py.detach(|| {
            simulation.run_observed(until, |progress| {
                Python::attach(|py| {
                    let paused = py.check_signals().and_then(|()| match &on_progress {
                        Some(callback) => {
                            let reply = callback.call1(py, (to_py(py, progress)?,))?;
                            Ok(reply.bind(py).extract::<bool>().is_ok_and(|go_on| !go_on))
                        }
                        None => Ok(false),
                    });
                    match paused {
                        Ok(false) => ControlFlow::Continue(()),
                        Ok(true) => ControlFlow::Break(()),
                        Err(error) => {
                            raised = Some(error);
                            ControlFlow::Break(())
                        }
                    }
                })
            })
        });
        if let Some(error) = raised {
            return Err(error);
        }
        to_py(py, &progress.map_err(runtime_error)?)
    }

    fn progress<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        to_py(py, &self.0.progress())
    }

    /// The lots in the fab, by id.
    fn lots<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        to_py(py, &self.0.lots())
    }

    /// Every tool, by id.
    fn tools<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        to_py(py, &self.0.tools())
    }

    /// Every tool group, in dataset order.
    fn tool_groups<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        to_py(py, &self.0.tool_groups())
    }

    /// Every CQT segment, in the order of the dataset info's segments: the lots in it and its
    /// completions so far.
    fn segments<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        to_py(py, &self.0.segments())
    }

    /// Results of the finished run.
    fn results<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        to_py(py, &self.0.results().map_err(runtime_error)?)
    }
}

/// Measures of results of one configuration's replications, with their means and 95%
/// confidence intervals.
#[pyfunction]
fn summarize<'py>(py: Python<'py>, results: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyAny>> {
    let results: Vec<Results> = depythonize(results).map_err(value_error)?;
    to_py(py, &report::summarize(&results))
}

/// Day-by-day measures of results of one configuration's replications, with their means and 95%
/// confidence intervals.
#[pyfunction]
fn daily<'py>(py: Python<'py>, results: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyAny>> {
    let results: Vec<Results> = depythonize(results).map_err(value_error)?;
    to_py(py, &report::daily(&results))
}

/// Summaries as CSV.
#[pyfunction]
fn csv(summaries: &Bound<'_, PyAny>) -> PyResult<String> {
    let summaries: Vec<Summary> = depythonize(summaries).map_err(value_error)?;
    Ok(report::csv(&summaries))
}

/// The measures of `other` against `baseline`, results of two configurations' replications
/// paired by seed and replication: the means and the mean difference with its 95% confidence
/// interval.
#[pyfunction]
fn compare<'py>(
    py: Python<'py>,
    baseline: &Bound<'py, PyAny>,
    other: &Bound<'py, PyAny>,
) -> PyResult<Bound<'py, PyAny>> {
    let baseline: Vec<Results> = depythonize(baseline).map_err(value_error)?;
    let other: Vec<Results> = depythonize(other).map_err(value_error)?;
    to_py(
        py,
        &report::compare(&baseline, &other).map_err(value_error)?,
    )
}

/// Comparisons as CSV.
#[pyfunction]
fn comparison_csv(comparisons: &Bound<'_, PyAny>) -> PyResult<String> {
    let comparisons: Vec<Comparison> = depythonize(comparisons).map_err(value_error)?;
    Ok(report::comparison_csv(&comparisons))
}

/// Digest of results: equal digests mean bit-identical results.
#[pyfunction]
fn digest(results: &Bound<'_, PyAny>) -> PyResult<String> {
    let results: Results = depythonize(results).map_err(value_error)?;
    Ok(results.digest())
}

#[pymodule]
#[pyo3(name = "smt2020")]
fn smt2020_module(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add("__version__", env!("CARGO_PKG_VERSION"))?;
    module.add("DAY", DAY)?;
    module.add("HOUR", HOUR)?;
    module.add("MINUTE", MINUTE)?;
    module.add("SECOND", SECOND)?;
    module.add_class::<Dataset>()?;
    module.add_class::<Simulation>()?;
    module.add_function(wrap_pyfunction!(load_dataset, module)?)?;
    module.add_function(wrap_pyfunction!(summarize, module)?)?;
    module.add_function(wrap_pyfunction!(daily, module)?)?;
    module.add_function(wrap_pyfunction!(csv, module)?)?;
    module.add_function(wrap_pyfunction!(compare, module)?)?;
    module.add_function(wrap_pyfunction!(comparison_csv, module)?)?;
    module.add_function(wrap_pyfunction!(digest, module)?)?;
    Ok(())
}

fn config_of(config: &Bound<'_, PyAny>) -> PyResult<Config> {
    depythonize(config).map_err(value_error)
}

/// Dicts, lists and None: the JSON form of the value.
fn to_py<'py, T: Serialize + ?Sized>(py: Python<'py>, value: &T) -> PyResult<Bound<'py, PyAny>> {
    Ok(pythonize(py, value)?)
}

/// An invalid argument: configuration, time or dataset.
fn value_error(error: impl Display) -> PyErr {
    PyValueError::new_err(error.to_string())
}

/// A run that could not go on.
fn runtime_error(error: sim::Error) -> PyErr {
    PyRuntimeError::new_err(error.to_string())
}
