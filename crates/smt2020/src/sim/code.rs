//! Strategy code: decisions a run leaves to functions the user writes (JavaScript in the page,
//! Python in the package, Rust here), asked at fixed points with views of the lot or batch:
//! `priority` when a lot arrives in a queue that ranks by code, `admit` when a lot could start the
//! step that begins a CQT segment, `start_batch` when a batch below its minimum size would wait.
//! A run asks in the same order every time, so code that answers from its arguments alone keeps
//! the run reproducible.

use des_core::Time;

use super::stats::LotKind;
use crate::data::{PartId, RouteId, StepIndex, ToolGroupId};

/// Strategy code. The run asks only the hooks [`hooks`](Self::hooks) names; an error stops the
/// run with it. Like the simulation that holds it, it may move to and be shared by other threads;
/// the hooks change it through `&mut self` only.
pub trait Code: Send + Sync {
    /// The hooks the code defines.
    fn hooks(&self) -> Hooks;

    /// Ranking value of `lot`, which arrives in a queue whose ranking has [`Criterion::Code`]
    /// (`queue_time: "code"` puts it before FIFO/CR): smaller first. Asked once per arrival, so a
    /// value that does not depend on `now` (a time, say) ranks the same at every dispatch.
    ///
    /// [`Criterion::Code`]: super::Criterion::Code
    fn priority(&mut self, lot: &LotView, now: Time) -> Result<f64, String> {
        let _ = (lot, now);
        Ok(0.0)
    }

    /// Whether `lot`, about to be dispatched at the step whose end starts `segment`, may start
    /// it now. Asked at every dispatch of the group that could start it; held lots are also
    /// asked again when a count of the segment's tool groups goes down.
    fn admit(&mut self, lot: &LotView, segment: &SegmentView, now: Time) -> Result<Admit, String> {
        let _ = (lot, segment, now);
        Ok(Admit::Now)
    }

    /// Whether `batch`, below its minimum size and still able to grow, starts now. Asked at
    /// every dispatch of the group that could start it.
    fn start_batch(&mut self, batch: &BatchView, now: Time) -> Result<Start, String> {
        let _ = (batch, now);
        Ok(Start::Wait)
    }
}

/// The hooks a strategy code defines.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Hooks {
    pub priority: bool,
    pub admit: bool,
    pub start_batch: bool,
}

/// An answer of `admit`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Admit {
    /// The lot may start the step.
    Now,
    /// The lot waits: asked again at the group's next dispatch or when a count goes down.
    Hold,
    /// The lot waits, and is asked again at this time at the latest.
    HoldUntil(Time),
}

/// An answer of `start_batch`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Start {
    /// The batch starts now.
    Now,
    /// The batch waits for more lots: asked again at the group's next dispatch.
    Wait,
    /// The batch waits, and is asked again at this time at the latest.
    WaitUntil(Time),
}

/// A lot as strategy code sees it, at the call.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LotView {
    /// Release number ([`LotStatus::id`](super::LotStatus::id)).
    pub id: u64,
    /// Index in [`DatasetInfo::parts`](crate::DatasetInfo::parts).
    pub part: PartId,
    pub kind: LotKind,
    /// Dispatching priority: the dataset's, or the engineering strategy's.
    pub priority: u32,
    pub wafers: u32,
    pub release: Time,
    pub due: Time,
    /// The part's route, the step the lot waits at and the step's tool group (indices of the
    /// dataset info).
    pub route: RouteId,
    pub step: StepIndex,
    pub tool_group: ToolGroupId,
    /// Expected work from this step to the end, and of this step (ms).
    pub remaining: f64,
    pub step_time: f64,
    /// The CQT segment the lot is in.
    pub cqt: Option<CqtView>,
}

/// The CQT segment a lot is in.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CqtView {
    /// Index in [`DatasetInfo::segments`](crate::DatasetInfo::segments).
    pub segment: usize,
    pub limit: Time,
    /// End of the entrance step; the exit step must start by `deadline` = `entered` + `limit`.
    pub entered: Time,
    pub deadline: Time,
    pub exit: StepIndex,
    /// Expected work from the lot's step until the exit step starts (ms).
    pub before_exit: f64,
    /// Queue-time slack: `deadline` − now − `before_exit` (ms).
    pub slack: f64,
}

/// The CQT segment a lot is about to start, with the counts stopping limits read.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SegmentView<'a> {
    /// Index in [`DatasetInfo::segments`](crate::DatasetInfo::segments).
    pub segment: usize,
    pub limit: Time,
    pub exit: StepIndex,
    /// Its tool groups after the entrance step, as the dataset info lists them.
    pub groups: &'a [GroupCount],
}

/// Lots in CQT segments at a tool group.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GroupCount {
    pub tool_group: ToolGroupId,
    /// Queued or processing at the group.
    pub front: u32,
    /// Those and the ones still to reach it in their segment.
    pub total: u32,
}

/// A batch below its minimum size: the best-ranked lot's batch kind, filled in rank order.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BatchView {
    pub tool_group: ToolGroupId,
    /// The route and step of its first lot.
    pub route: RouteId,
    pub step: StepIndex,
    pub lots: u32,
    pub wafers: u32,
    /// The batch size limits of the step, in wafers.
    pub min: u32,
    pub max: u32,
    /// The earliest arrival of its lots in the queue.
    pub oldest: Time,
    /// The least queue-time slack of its lots in CQT segments (ms).
    pub slack: Option<f64>,
}
