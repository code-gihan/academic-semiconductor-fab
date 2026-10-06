//! Strategy code in Python: an object with any of the methods `priority(lot, now)`,
//! `admit(lot, segment, now)` and `start_batch(batch, now)` (a module of such functions works
//! too), called with views of the lot, the CQT segment it is about to start or the batch.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use pyo3::exceptions::{PyKeyboardInterrupt, PyTypeError};
use pyo3::prelude::*;
use pyo3::types::{PyBool, PyString};
use smt2020::sim::{self, Admit, BatchView, Code, Hooks, LotView, SegmentView, Start};
use smt2020::{Dataset, Time};

/// What the code raised for the run to raise again: the first exception (the run fails), and
/// Ctrl-C while the code ran (the run pauses at the next simulated day).
#[derive(Default)]
pub(crate) struct Raised {
    pub exception: Mutex<Option<PyErr>>,
    pub interrupted: AtomicBool,
}

/// The strategy code `code` describes, reporting what it raises to `raised`.
pub(crate) fn code_of(
    py: Python<'_>,
    dataset: &Dataset,
    code: &Bound<'_, PyAny>,
    raised: Arc<Raised>,
) -> PyResult<Box<dyn Code>> {
    let method = |name: &str| -> PyResult<Option<Py<PyAny>>> {
        if !code.hasattr(name)? {
            return Ok(None);
        }
        let method = code.getattr(name)?;
        if method.is_none() {
            return Ok(None);
        }
        if !method.is_callable() {
            return Err(PyTypeError::new_err(format!("code.{name} is not callable")));
        }
        Ok(Some(method.unbind()))
    };
    Ok(Box::new(PyCode {
        priority: method("priority")?,
        admit: method("admit")?,
        start_batch: method("start_batch")?,
        names: Names::of(py, dataset),
        raised,
    }))
}

struct PyCode {
    priority: Option<Py<PyAny>>,
    admit: Option<Py<PyAny>>,
    start_batch: Option<Py<PyAny>>,
    names: Names,
    raised: Arc<Raised>,
}

impl PyCode {
    /// Makes a call of the code; Ctrl-C during it calls again (the code answers as it would
    /// have) and pauses the run later. Another exception is kept for the run to raise.
    fn call<'py>(
        &self,
        py: Python<'py>,
        call: impl Fn() -> PyResult<Py<PyAny>>,
    ) -> Result<Bound<'py, PyAny>, String> {
        let reply = match call() {
            Err(error) if error.is_instance_of::<PyKeyboardInterrupt>(py) => {
                self.raised.interrupted.store(true, Ordering::Relaxed);
                call()
            }
            reply => reply,
        };
        reply.map(|reply| reply.into_bound(py)).map_err(|error| {
            let message = error.to_string();
            let mut kept = self.raised.exception.lock().expect("unpoisoned");
            kept.get_or_insert(error);
            message
        })
    }

    fn lot(&self, py: Python<'_>, view: &LotView) -> PyResult<Py<Lot>> {
        let names = &self.names;
        let cqt = view
            .cqt
            .map(|cqt| {
                Py::new(
                    py,
                    Cqt {
                        segment: cqt.segment,
                        limit: cqt.limit,
                        entered: cqt.entered,
                        deadline: cqt.deadline,
                        exit: cqt.exit,
                        before_exit: cqt.before_exit,
                        slack: cqt.slack,
                    },
                )
            })
            .transpose()?;
        Py::new(
            py,
            Lot {
                id: view.id,
                part: names.parts[view.part].clone_ref(py),
                kind: names.kinds[view.kind as usize].clone_ref(py),
                priority: view.priority,
                wafers: view.wafers,
                release: view.release,
                due: view.due,
                step: view.step,
                step_name: names.steps[view.route][view.step].clone_ref(py),
                tool_group: names.tool_groups[view.tool_group].clone_ref(py),
                remaining: view.remaining,
                step_time: view.step_time,
                cqt,
            },
        )
    }
}

impl Code for PyCode {
    fn hooks(&self) -> Hooks {
        Hooks {
            priority: self.priority.is_some(),
            admit: self.admit.is_some(),
            start_batch: self.start_batch.is_some(),
        }
    }

    fn priority(&mut self, lot: &LotView, now: Time) -> Result<f64, String> {
        Python::attach(|py| {
            let method = self.priority.as_ref().expect("priority");
            let reply = self.call(py, || method.call1(py, (self.lot(py, lot)?, now)))?;
            reply
                .extract::<f64>()
                .map_err(|_| format!("returned {}, not a number", type_name(&reply)))
        })
    }

    fn admit(&mut self, lot: &LotView, segment: &SegmentView, now: Time) -> Result<Admit, String> {
        Python::attach(|py| {
            let method = self.admit.as_ref().expect("admit");
            let reply = self.call(py, || {
                let groups = segment
                    .groups
                    .iter()
                    .map(|group| {
                        Py::new(
                            py,
                            SegmentGroup {
                                tool_group: self.names.tool_groups[group.tool_group].clone_ref(py),
                                front: group.front,
                                total: group.total,
                            },
                        )
                    })
                    .collect::<PyResult<Vec<_>>>()?;
                let segment = Py::new(
                    py,
                    Segment {
                        segment: segment.segment,
                        limit: segment.limit,
                        exit: segment.exit,
                        groups,
                    },
                )?;
                method.call1(py, (self.lot(py, lot)?, segment, now))
            })?;
            match answer(&reply)? {
                Answer::Yes => Ok(Admit::Now),
                Answer::No => Ok(Admit::Hold),
                Answer::At(at) => Ok(Admit::HoldUntil(at)),
            }
        })
    }

    fn start_batch(&mut self, batch: &BatchView, now: Time) -> Result<Start, String> {
        Python::attach(|py| {
            let method = self.start_batch.as_ref().expect("start_batch");
            let names = &self.names;
            let reply = self.call(py, || {
                let batch = Py::new(
                    py,
                    Batch {
                        tool_group: names.tool_groups[batch.tool_group].clone_ref(py),
                        step: batch.step,
                        step_name: names.steps[batch.route][batch.step].clone_ref(py),
                        lots: batch.lots,
                        wafers: batch.wafers,
                        min: batch.min,
                        max: batch.max,
                        oldest: batch.oldest,
                        slack: batch.slack,
                    },
                )?;
                method.call1(py, (batch, now))
            })?;
            match answer(&reply)? {
                Answer::Yes => Ok(Start::Now),
                Answer::No => Ok(Start::Wait),
                Answer::At(at) => Ok(Start::WaitUntil(at)),
            }
        })
    }
}

/// An answer of `admit` or `start_batch`: True, False or a time (ms).
enum Answer {
    Yes,
    No,
    At(Time),
}

fn answer(reply: &Bound<'_, PyAny>) -> Result<Answer, String> {
    if let Ok(flag) = reply.cast::<PyBool>() {
        return Ok(if flag.is_true() {
            Answer::Yes
        } else {
            Answer::No
        });
    }
    match reply.extract::<f64>() {
        Ok(at) => sim::time(at)
            .map(Answer::At)
            .map_err(|error| error.to_string()),
        Err(_) => Err(format!(
            "returned {}, not True, False or a time",
            type_name(reply)
        )),
    }
}

fn type_name(value: &Bound<'_, PyAny>) -> String {
    value
        .get_type()
        .name()
        .map_or_else(|_| "?".into(), |name| name.to_string())
}

/// Names of what the views index, as Python strings made once.
struct Names {
    parts: Vec<Py<PyString>>,
    tool_groups: Vec<Py<PyString>>,
    kinds: Vec<Py<PyString>>,
    /// Per route, its steps' names.
    steps: Vec<Vec<Py<PyString>>>,
}

impl Names {
    fn of(py: Python<'_>, dataset: &Dataset) -> Self {
        let string = |text: &str| PyString::new(py, text).unbind();
        Self {
            parts: dataset
                .parts
                .iter()
                .map(|part| string(&part.name))
                .collect(),
            tool_groups: dataset
                .tool_groups
                .iter()
                .map(|group| string(&group.name))
                .collect(),
            kinds: sim::LotKind::ALL
                .iter()
                .map(|kind| string(kind.name()))
                .collect(),
            steps: dataset
                .routes
                .iter()
                .map(|route| route.steps.iter().map(|step| string(&step.name)).collect())
                .collect(),
        }
    }
}

/// A lot as strategy code sees it (`smt2020.sim.LotView`); times and work in ms.
#[pyclass(frozen, get_all, module = "smt2020")]
pub(crate) struct Lot {
    id: u64,
    part: Py<PyString>,
    kind: Py<PyString>,
    priority: u32,
    wafers: u32,
    release: Time,
    due: Time,
    step: usize,
    step_name: Py<PyString>,
    tool_group: Py<PyString>,
    remaining: f64,
    step_time: f64,
    cqt: Option<Py<Cqt>>,
}

/// The CQT segment a lot is in; times and work in ms.
#[pyclass(frozen, get_all, module = "smt2020")]
pub(crate) struct Cqt {
    segment: usize,
    limit: Time,
    entered: Time,
    deadline: Time,
    exit: usize,
    before_exit: f64,
    slack: f64,
}

/// The CQT segment a lot is about to start.
#[pyclass(frozen, get_all, module = "smt2020")]
pub(crate) struct Segment {
    segment: usize,
    limit: Time,
    exit: usize,
    groups: Vec<Py<SegmentGroup>>,
}

/// Lots in CQT segments at a tool group of a segment: queued or processing there (`front`), and
/// those plus the ones still to reach it (`total`).
#[pyclass(frozen, get_all, module = "smt2020")]
pub(crate) struct SegmentGroup {
    tool_group: Py<PyString>,
    front: u32,
    total: u32,
}

/// A batch below its minimum size; times in ms.
#[pyclass(frozen, get_all, module = "smt2020")]
pub(crate) struct Batch {
    tool_group: Py<PyString>,
    step: usize,
    step_name: Py<PyString>,
    lots: u32,
    wafers: u32,
    min: u32,
    max: u32,
    oldest: Time,
    slack: Option<f64>,
}
