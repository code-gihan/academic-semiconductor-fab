//! Strategy code in JavaScript: an object with the functions `priority(lot, now)`,
//! `admit(lot, segment, now)` and `startBatch(batch, now)`, any of them, called as its methods
//! with views of the lot, the CQT segment it is about to start or the batch. A view is one object
//! per simulation, filled anew for each call: it is valid during the call only.

use std::cell::RefCell;
use std::rc::Rc;

use js_sys::{Array, Function, Object, Reflect};
use smt2020::sim::{self, Admit, BatchView, Code, GroupCount, Hooks, LotView, SegmentView, Start};
use smt2020::{Dataset, Time};
use wasm_bindgen::prelude::*;

thread_local! {
    /// The functions and views of each simulation's strategy code, by slot. The core holds code
    /// that may move between threads, which JavaScript values cannot: it holds the slot.
    static CODES: RefCell<Vec<Option<Rc<Functions>>>> = const { RefCell::new(Vec::new()) };
}

/// The strategy code `value` describes, if any.
pub(crate) fn code_of(
    dataset: &Dataset,
    value: &JsValue,
) -> Result<Option<Box<dyn Code>>, JsValue> {
    if value.is_undefined() || value.is_null() {
        return Ok(None);
    }
    let function = |name: &str| -> Result<Option<Function>, JsValue> {
        let property = Reflect::get(value, &JsValue::from_str(name))?;
        if property.is_undefined() || property.is_null() {
            return Ok(None);
        }
        property
            .dyn_into::<Function>()
            .map(Some)
            .map_err(|_| JsError::new(&format!("code.{name} is not a function")).into())
    };
    let (priority, admit, start_batch) = (
        function("priority")?,
        function("admit")?,
        function("startBatch")?,
    );
    let hooks = Hooks {
        priority: priority.is_some(),
        admit: admit.is_some(),
        start_batch: start_batch.is_some(),
    };
    let names = Rc::new(Names::of(dataset));
    let lot = Rc::new(RefCell::new(LotCell {
        view: None,
        cqt: JsValue::NULL,
        names: Rc::clone(&names),
    }));
    let cqt = JsValue::from(Cqt(Rc::clone(&lot)));
    lot.borrow_mut().cqt = cqt;
    let segment = Rc::new(RefCell::new(SegmentCell {
        view: None,
        groups: Vec::new(),
        names: Rc::clone(&names),
    }));
    let batch = Rc::new(RefCell::new(BatchCell { view: None, names }));
    let functions = Rc::new(Functions {
        this: value.clone(),
        priority,
        admit,
        start_batch,
        lot_js: Lot(Rc::clone(&lot)).into(),
        lot,
        segment_js: Segment(Rc::clone(&segment)).into(),
        segment,
        batch_js: Batch(Rc::clone(&batch)).into(),
        batch,
    });
    let slot = CODES.with_borrow_mut(|codes| match codes.iter().position(Option::is_none) {
        Some(free) => {
            codes[free] = Some(functions);
            free
        }
        None => {
            codes.push(Some(functions));
            codes.len() - 1
        }
    });
    Ok(Some(Box::new(JsCode { slot, hooks })))
}

/// The code the core holds: a slot of [`CODES`], emptied when the simulation drops it.
struct JsCode {
    slot: usize,
    hooks: Hooks,
}

struct Functions {
    /// The code object, `this` of its functions.
    this: JsValue,
    priority: Option<Function>,
    admit: Option<Function>,
    start_batch: Option<Function>,
    lot: Rc<RefCell<LotCell>>,
    lot_js: JsValue,
    segment: Rc<RefCell<SegmentCell>>,
    segment_js: JsValue,
    batch: Rc<RefCell<BatchCell>>,
    batch_js: JsValue,
}

impl JsCode {
    fn functions(&self) -> Rc<Functions> {
        CODES.with_borrow(|codes| Rc::clone(codes[self.slot].as_ref().expect("live strategy code")))
    }
}

impl Drop for JsCode {
    fn drop(&mut self) {
        CODES.with_borrow_mut(|codes| codes[self.slot] = None);
    }
}

impl Code for JsCode {
    fn hooks(&self) -> Hooks {
        self.hooks
    }

    fn priority(&mut self, lot: &LotView, now: Time) -> Result<f64, String> {
        let code = self.functions();
        code.lot.borrow_mut().view = Some(*lot);
        let function = code.priority.as_ref().expect("priority");
        let reply = function
            .call2(&code.this, &code.lot_js, &JsValue::from_f64(now as f64))
            .map_err(|error| message(&error))?;
        reply
            .as_f64()
            .ok_or_else(|| format!("returned {}, not a number", type_of(&reply)))
    }

    fn admit(&mut self, lot: &LotView, segment: &SegmentView, now: Time) -> Result<Admit, String> {
        let code = self.functions();
        code.lot.borrow_mut().view = Some(*lot);
        {
            let mut cell = code.segment.borrow_mut();
            cell.view = Some((segment.segment, segment.limit, segment.exit));
            cell.groups.clear();
            cell.groups.extend_from_slice(segment.groups);
        }
        let function = code.admit.as_ref().expect("admit");
        let reply = function
            .call3(
                &code.this,
                &code.lot_js,
                &code.segment_js,
                &JsValue::from_f64(now as f64),
            )
            .map_err(|error| message(&error))?;
        match (reply.as_bool(), reply.as_f64()) {
            (Some(true), _) => Ok(Admit::Now),
            (Some(false), _) => Ok(Admit::Hold),
            (None, Some(at)) => Ok(Admit::HoldUntil(time(at)?)),
            _ => Err(format!(
                "returned {}, not true, false or a time",
                type_of(&reply)
            )),
        }
    }

    fn start_batch(&mut self, batch: &BatchView, now: Time) -> Result<Start, String> {
        let code = self.functions();
        code.batch.borrow_mut().view = Some(*batch);
        let function = code.start_batch.as_ref().expect("startBatch");
        let reply = function
            .call2(&code.this, &code.batch_js, &JsValue::from_f64(now as f64))
            .map_err(|error| message(&error))?;
        match (reply.as_bool(), reply.as_f64()) {
            (Some(true), _) => Ok(Start::Now),
            (Some(false), _) => Ok(Start::Wait),
            (None, Some(at)) => Ok(Start::WaitUntil(time(at)?)),
            _ => Err(format!(
                "returned {}, not true, false or a time",
                type_of(&reply)
            )),
        }
    }
}

/// A time the code returned (ms).
fn time(ms: f64) -> Result<Time, String> {
    sim::time(ms).map_err(|error| error.to_string())
}

/// What a thrown value says: an error's name and message, or the value.
fn message(error: &JsValue) -> String {
    match error.dyn_ref::<js_sys::Error>() {
        Some(error) => String::from(error.to_string()),
        None => error.as_string().unwrap_or_else(|| format!("{error:?}")),
    }
}

fn type_of(value: &JsValue) -> String {
    if value.is_null() {
        return "null".into();
    }
    value.js_typeof().as_string().unwrap_or_default()
}

/// Names of what the views index, as JavaScript strings made once.
struct Names {
    parts: Vec<JsValue>,
    tool_groups: Vec<JsValue>,
    kinds: Vec<JsValue>,
    /// Per route, its steps' names.
    steps: Vec<Vec<String>>,
}

impl Names {
    fn of(dataset: &Dataset) -> Self {
        Self {
            parts: dataset
                .parts
                .iter()
                .map(|part| JsValue::from_str(&part.name))
                .collect(),
            tool_groups: dataset
                .tool_groups
                .iter()
                .map(|group| JsValue::from_str(&group.name))
                .collect(),
            kinds: sim::LotKind::ALL
                .iter()
                .map(|kind| JsValue::from_str(kind.name()))
                .collect(),
            steps: dataset
                .routes
                .iter()
                .map(|route| route.steps.iter().map(|step| step.name.clone()).collect())
                .collect(),
        }
    }
}

struct LotCell {
    view: Option<LotView>,
    /// The `Cqt` object of the lot.
    cqt: JsValue,
    names: Rc<Names>,
}

impl LotCell {
    fn view(&self) -> LotView {
        self.view.expect("a lot filled for the call")
    }
}

/// A lot as strategy code sees it, during the call ([`LotView`]); times and work in ms.
#[wasm_bindgen]
pub struct Lot(Rc<RefCell<LotCell>>);

#[wasm_bindgen]
impl Lot {
    /// Release number, as `lots()` lists the lot.
    #[wasm_bindgen(getter)]
    pub fn id(&self) -> f64 {
        self.0.borrow().view().id as f64
    }

    #[wasm_bindgen(getter)]
    pub fn part(&self) -> JsValue {
        let cell = self.0.borrow();
        cell.names.parts[cell.view().part].clone()
    }

    /// PRL, PHL, SHL, ERL or EHL.
    #[wasm_bindgen(getter)]
    pub fn kind(&self) -> JsValue {
        let cell = self.0.borrow();
        cell.names.kinds[cell.view().kind as usize].clone()
    }

    /// Dispatching priority: the dataset's, or the engineering strategy's.
    #[wasm_bindgen(getter)]
    pub fn priority(&self) -> u32 {
        self.0.borrow().view().priority
    }

    #[wasm_bindgen(getter)]
    pub fn wafers(&self) -> u32 {
        self.0.borrow().view().wafers
    }

    #[wasm_bindgen(getter)]
    pub fn release(&self) -> f64 {
        self.0.borrow().view().release as f64
    }

    #[wasm_bindgen(getter)]
    pub fn due(&self) -> f64 {
        self.0.borrow().view().due as f64
    }

    /// The step the lot waits at: its index in the route, its name and its tool group.
    #[wasm_bindgen(getter)]
    pub fn step(&self) -> usize {
        self.0.borrow().view().step
    }

    #[wasm_bindgen(getter = stepName)]
    pub fn step_name(&self) -> String {
        let cell = self.0.borrow();
        let view = cell.view();
        cell.names.steps[view.route][view.step].clone()
    }

    #[wasm_bindgen(getter = toolGroup)]
    pub fn tool_group(&self) -> JsValue {
        let cell = self.0.borrow();
        cell.names.tool_groups[cell.view().tool_group].clone()
    }

    /// Expected work from this step to the end, and of this step.
    #[wasm_bindgen(getter)]
    pub fn remaining(&self) -> f64 {
        self.0.borrow().view().remaining
    }

    #[wasm_bindgen(getter = stepTime)]
    pub fn step_time(&self) -> f64 {
        self.0.borrow().view().step_time
    }

    /// The CQT segment the lot is in, or null.
    #[wasm_bindgen(getter)]
    pub fn cqt(&self) -> JsValue {
        let cell = self.0.borrow();
        if cell.view().cqt.is_some() {
            cell.cqt.clone()
        } else {
            JsValue::NULL
        }
    }
}

/// The CQT segment a lot is in, during the call ([`sim::CqtView`]); times and work in ms.
#[wasm_bindgen]
pub struct Cqt(Rc<RefCell<LotCell>>);

impl Cqt {
    fn view(&self) -> sim::CqtView {
        self.0.borrow().view().cqt.expect("a lot in a CQT segment")
    }
}

#[wasm_bindgen]
impl Cqt {
    /// Index in the dataset info's segments.
    #[wasm_bindgen(getter)]
    pub fn segment(&self) -> usize {
        self.view().segment
    }

    #[wasm_bindgen(getter)]
    pub fn limit(&self) -> f64 {
        self.view().limit as f64
    }

    /// End of the entrance step; the exit step must start by `deadline` = `entered` + `limit`.
    #[wasm_bindgen(getter)]
    pub fn entered(&self) -> f64 {
        self.view().entered as f64
    }

    #[wasm_bindgen(getter)]
    pub fn deadline(&self) -> f64 {
        self.view().deadline as f64
    }

    /// The exit step's index in the route.
    #[wasm_bindgen(getter)]
    pub fn exit(&self) -> usize {
        self.view().exit
    }

    /// Expected work from the lot's step until the exit step starts.
    #[wasm_bindgen(getter = beforeExit)]
    pub fn before_exit(&self) -> f64 {
        self.view().before_exit
    }

    /// Queue-time slack: `deadline` − now − `beforeExit`.
    #[wasm_bindgen(getter)]
    pub fn slack(&self) -> f64 {
        self.view().slack
    }
}

struct SegmentCell {
    /// Index, limit and exit step.
    view: Option<(usize, Time, usize)>,
    groups: Vec<GroupCount>,
    names: Rc<Names>,
}

impl SegmentCell {
    fn view(&self) -> (usize, Time, usize) {
        self.view.expect("a segment filled for the call")
    }
}

/// The CQT segment a lot is about to start, during the call ([`SegmentView`]).
#[wasm_bindgen]
pub struct Segment(Rc<RefCell<SegmentCell>>);

#[wasm_bindgen]
impl Segment {
    /// Index in the dataset info's segments.
    #[wasm_bindgen(getter)]
    pub fn segment(&self) -> usize {
        self.0.borrow().view().0
    }

    #[wasm_bindgen(getter)]
    pub fn limit(&self) -> f64 {
        self.0.borrow().view().1 as f64
    }

    /// The exit step's index in the route.
    #[wasm_bindgen(getter)]
    pub fn exit(&self) -> usize {
        self.0.borrow().view().2
    }

    /// Its tool groups after the entrance step: `{toolGroup, front, total}`, the lots in CQT
    /// segments queued or processing there, and those plus the ones still to reach it.
    #[wasm_bindgen(getter)]
    pub fn groups(&self) -> Result<Array, JsValue> {
        let cell = self.0.borrow();
        cell.groups
            .iter()
            .map(|group| {
                let object = Object::new();
                Reflect::set(
                    &object,
                    &"toolGroup".into(),
                    &cell.names.tool_groups[group.tool_group],
                )?;
                Reflect::set(&object, &"front".into(), &group.front.into())?;
                Reflect::set(&object, &"total".into(), &group.total.into())?;
                Ok(JsValue::from(object))
            })
            .collect()
    }
}

struct BatchCell {
    view: Option<BatchView>,
    names: Rc<Names>,
}

impl BatchCell {
    fn view(&self) -> BatchView {
        self.view.expect("a batch filled for the call")
    }
}

/// A batch below its minimum size, during the call ([`BatchView`]); times in ms.
#[wasm_bindgen]
pub struct Batch(Rc<RefCell<BatchCell>>);

#[wasm_bindgen]
impl Batch {
    #[wasm_bindgen(getter = toolGroup)]
    pub fn tool_group(&self) -> JsValue {
        let cell = self.0.borrow();
        cell.names.tool_groups[cell.view().tool_group].clone()
    }

    /// The step of its first lot: index in the route, and name.
    #[wasm_bindgen(getter)]
    pub fn step(&self) -> usize {
        self.0.borrow().view().step
    }

    #[wasm_bindgen(getter = stepName)]
    pub fn step_name(&self) -> String {
        let cell = self.0.borrow();
        let view = cell.view();
        cell.names.steps[view.route][view.step].clone()
    }

    #[wasm_bindgen(getter)]
    pub fn lots(&self) -> u32 {
        self.0.borrow().view().lots
    }

    #[wasm_bindgen(getter)]
    pub fn wafers(&self) -> u32 {
        self.0.borrow().view().wafers
    }

    /// The step's batch size limits, in wafers.
    #[wasm_bindgen(getter)]
    pub fn min(&self) -> u32 {
        self.0.borrow().view().min
    }

    #[wasm_bindgen(getter)]
    pub fn max(&self) -> u32 {
        self.0.borrow().view().max
    }

    /// The earliest arrival of its lots in the queue.
    #[wasm_bindgen(getter)]
    pub fn oldest(&self) -> f64 {
        self.0.borrow().view().oldest as f64
    }

    /// The least queue-time slack of its lots in CQT segments, or null.
    #[wasm_bindgen(getter)]
    pub fn slack(&self) -> JsValue {
        self.0
            .borrow()
            .view()
            .slack
            .map_or(JsValue::NULL, JsValue::from_f64)
    }
}
