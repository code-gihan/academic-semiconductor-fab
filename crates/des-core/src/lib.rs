//! Model-independent discrete-event simulation core: event-scheduling world view with
//! next-event time advance.
//!
//! A [`Model`] owns the system state and its event routines. [`Simulation`] owns the clock and
//! the future event list and hands every event, in (time, scheduling order) order, to
//! [`Model::handle`], which may schedule further events or stop the run through the
//! [`Scheduler`]. Observers get periodic observation events without taking part in the model.

mod engine;
mod queue;
mod time;

pub use engine::{Model, Outcome, Scheduler, Simulation};
pub use time::{DAY, HOUR, MINUTE, SECOND, Time};
