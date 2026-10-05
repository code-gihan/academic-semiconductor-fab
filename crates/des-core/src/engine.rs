//! Simulation executive: clock, scheduling and the next-event loop.

use std::mem;
use std::ops::ControlFlow;

use crate::Time;
use crate::queue::EventQueue;

/// System state plus the event routines that change it.
pub trait Model {
    type Event;

    /// Initialization routine, called once at t = 0 to schedule the first events.
    fn init(&mut self, sched: &mut Scheduler<Self::Event>);

    /// Event routine; `sched.now()` is the event's time.
    fn handle(&mut self, event: Self::Event, sched: &mut Scheduler<Self::Event>);
}

/// Clock and future event list as seen by the model. Events with equal times run in scheduling
/// order, so `schedule_in(0, e)` runs after every event already scheduled for now.
pub struct Scheduler<E> {
    now: Time,
    queue: EventQueue<E>,
    stop: bool,
}

impl<E> Scheduler<E> {
    pub fn now(&self) -> Time {
        self.now
    }

    /// Schedules `event` at absolute `time`.
    ///
    /// # Panics
    /// If `time` is before now (causality violation).
    pub fn schedule_at(&mut self, time: Time, event: E) {
        assert!(
            time >= self.now,
            "event scheduled in the past: {time} < now {}",
            self.now
        );
        self.queue.push(time, event);
    }

    /// Schedules `event` `delay` after now.
    ///
    /// # Panics
    /// If `delay` is negative.
    pub fn schedule_in(&mut self, delay: Time, event: E) {
        self.schedule_at(self.now + delay, event);
    }

    /// Ends the run once the current event is handled; pending events stay scheduled.
    pub fn stop(&mut self) {
        self.stop = true;
    }
}

/// How a run ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The model called [`Scheduler::stop`].
    Stopped,
    /// The clock reached the end time of the run.
    Reached,
    /// The observer broke the run off.
    Interrupted,
    /// No event was left.
    Exhausted,
}

/// Owns the model, the clock and the future event list.
pub struct Simulation<M: Model> {
    model: M,
    sched: Scheduler<M::Event>,
    events_processed: u64,
}

impl<M: Model> Simulation<M> {
    /// Starts the clock at 0 and runs the model's initialization routine.
    pub fn new(mut model: M) -> Self {
        let mut sched = Scheduler {
            now: 0,
            queue: EventQueue::new(),
            stop: false,
        };
        model.init(&mut sched);
        Self {
            model,
            sched,
            events_processed: 0,
        }
    }

    /// Handles the events up to `until` (inclusive) in (time, scheduling order), then sets the
    /// clock to `until`; ends earlier when the model stops the run or no event is left. A later
    /// run continues with the pending events; `Time::MAX` runs without end time.
    pub fn run(&mut self, until: Time) -> Outcome {
        self.run_observed(until, Time::MAX, |_, _| ControlFlow::Continue(()))
    }

    /// [`run`](Self::run) with an observation at every multiple of `interval` up to `until`: the
    /// observer sees the model after every event up to the observation time and may break the run
    /// off.
    ///
    /// # Panics
    /// If `interval` is not positive.
    pub fn run_observed(
        &mut self,
        until: Time,
        interval: Time,
        mut observe: impl FnMut(&M, Time) -> ControlFlow<()>,
    ) -> Outcome {
        assert!(
            interval > 0,
            "observation interval {interval} is not positive"
        );
        // The observations form a second event stream on the grid of `interval`, merged in time
        // order; a run resumed between grid points keeps the grid.
        let mut observation = (self.sched.now / interval + 1).saturating_mul(interval);
        loop {
            let Some(next) = self.sched.queue.next_time() else {
                return Outcome::Exhausted;
            };
            if observation < next && observation <= until {
                self.sched.now = observation;
                if observe(&self.model, observation).is_break() {
                    return Outcome::Interrupted;
                }
                observation = observation.saturating_add(interval);
                continue;
            }
            if next > until {
                self.sched.now = self.sched.now.max(until);
                return Outcome::Reached;
            }
            let (time, event) = self.sched.queue.pop().expect("a next event");
            self.sched.now = time;
            self.model.handle(event, &mut self.sched);
            self.events_processed += 1;
            if mem::take(&mut self.sched.stop) {
                return Outcome::Stopped;
            }
        }
    }

    pub fn now(&self) -> Time {
        self.sched.now
    }

    pub fn model(&self) -> &M {
        &self.model
    }

    /// Model events handled so far (throughput metric); observations do not count.
    pub fn events_processed(&self) -> u64 {
        self.events_processed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DAY, HOUR};

    /// Logs every event; "spawn" schedules follow-ups, "stop" ends the run, "rewind" breaks
    /// causality.
    struct Recorder {
        initial: Vec<(Time, &'static str)>,
        log: Vec<(Time, &'static str)>,
    }

    impl Model for Recorder {
        type Event = &'static str;

        fn init(&mut self, sched: &mut Scheduler<Self::Event>) {
            for &(time, event) in &self.initial {
                sched.schedule_at(time, event);
            }
        }

        fn handle(&mut self, event: Self::Event, sched: &mut Scheduler<Self::Event>) {
            self.log.push((sched.now(), event));
            match event {
                "spawn" => {
                    sched.schedule_in(0, "spawned now");
                    sched.schedule_in(HOUR, "spawned later");
                }
                "stop" => sched.stop(),
                "rewind" => sched.schedule_at(sched.now() - 1, "never"),
                _ => {}
            }
        }
    }

    fn simulation(initial: &[(Time, &'static str)]) -> Simulation<Recorder> {
        Simulation::new(Recorder {
            initial: initial.to_vec(),
            log: Vec::new(),
        })
    }

    #[test]
    fn runs_in_time_then_scheduling_order_until_stopped() {
        let mut sim = simulation(&[(DAY, "stop"), (10, "spawn"), (10, "peer"), (2 * DAY, "c")]);
        assert_eq!(sim.run(Time::MAX), Outcome::Stopped);
        assert_eq!(
            sim.model().log,
            [
                (10, "spawn"),
                (10, "peer"),
                (10, "spawned now"),
                (10 + HOUR, "spawned later"),
                (DAY, "stop")
            ]
        );
        assert_eq!((sim.now(), sim.events_processed()), (DAY, 5));
        // Pending events stay: the next run continues with them.
        assert_eq!(sim.run(Time::MAX), Outcome::Exhausted);
        assert_eq!(sim.model().log.last(), Some(&(2 * DAY, "c")));
    }

    #[test]
    fn runs_end_at_their_end_time_and_continue_from_it() {
        let mut sim = simulation(&[(0, "a"), (DAY, "b"), (DAY, "c"), (2 * DAY, "d")]);
        // Events at the end time are handled; the clock stops there.
        assert_eq!(sim.run(DAY), Outcome::Reached);
        assert_eq!((sim.now(), sim.model().log.len()), (DAY, 3));
        // An end time already passed leaves the clock as it is.
        assert_eq!(sim.run(DAY / 2), Outcome::Reached);
        assert_eq!((sim.now(), sim.model().log.len()), (DAY, 3));
        assert_eq!(sim.run(DAY + HOUR), Outcome::Reached);
        assert_eq!(sim.now(), DAY + HOUR);
        assert_eq!(sim.run(Time::MAX), Outcome::Exhausted);
        assert_eq!((sim.now(), sim.events_processed()), (2 * DAY, 4));
    }

    #[test]
    fn observations_follow_every_event_up_to_their_time() {
        let mut sim = simulation(&[(0, "a"), (DAY, "b"), (DAY + 1, "c"), (3 * DAY, "d")]);
        let mut seen = Vec::new();
        let outcome = sim.run_observed(Time::MAX, DAY, |model, now| {
            seen.push((now, model.log.len()));
            ControlFlow::Continue(())
        });
        // The run ends with the last event, before the observation it coincides with.
        assert_eq!(outcome, Outcome::Exhausted);
        assert_eq!(seen, [(DAY, 2), (2 * DAY, 3)]);
        assert_eq!(sim.events_processed(), 4);
    }

    #[test]
    fn observations_keep_their_grid_across_runs() {
        let mut sim = simulation(&[(0, "a"), (5 * DAY, "b")]);
        let mut seen = Vec::new();
        let mut observe = |_: &Recorder, now| {
            seen.push(now);
            ControlFlow::Continue(())
        };
        // Observed up to and at the end time, then on the same grid after it.
        assert_eq!(
            sim.run_observed(2 * DAY, DAY, &mut observe),
            Outcome::Reached
        );
        assert_eq!(sim.run(2 * DAY + HOUR), Outcome::Reached);
        assert_eq!(
            sim.run_observed(Time::MAX, DAY, &mut observe),
            Outcome::Exhausted
        );
        assert_eq!(seen, [DAY, 2 * DAY, 3 * DAY, 4 * DAY]);
    }

    #[test]
    fn observer_breaks_the_run_off() {
        let mut sim = simulation(&[(0, "a"), (5 * DAY, "b")]);
        let outcome = sim.run_observed(Time::MAX, DAY, |_, now| {
            if now == 2 * DAY {
                ControlFlow::Break(())
            } else {
                ControlFlow::Continue(())
            }
        });
        assert_eq!(
            (outcome, sim.now(), sim.events_processed()),
            (Outcome::Interrupted, 2 * DAY, 1)
        );
    }

    #[test]
    #[should_panic(expected = "scheduled in the past")]
    fn scheduling_in_the_past_panics() {
        simulation(&[(10, "rewind")]).run(Time::MAX);
    }
}
