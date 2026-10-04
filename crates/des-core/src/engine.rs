//! Simulation executive: clock, scheduling and the next-event loop.

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
        };
        model.init(&mut sched);
        Self {
            model,
            sched,
            events_processed: 0,
        }
    }

    /// Handles every event due at or before `end`, including events scheduled meanwhile, then
    /// advances the clock to `end`. Later events stay pending, so calls can be chained.
    ///
    /// # Panics
    /// If `end` is before now.
    pub fn run_until(&mut self, end: Time) {
        assert!(
            end >= self.sched.now,
            "run_until({end}) is before now {}",
            self.sched.now
        );
        while let Some((time, event)) = self.sched.queue.pop_due(end) {
            self.sched.now = time;
            self.model.handle(event, &mut self.sched);
            self.events_processed += 1;
        }
        self.sched.now = end;
    }

    pub fn now(&self) -> Time {
        self.sched.now
    }

    pub fn model(&self) -> &M {
        &self.model
    }

    /// Events handled so far (throughput metric).
    pub fn events_processed(&self) -> u64 {
        self.events_processed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DAY, HOUR};

    /// Logs every event; "spawn" schedules follow-ups, "rewind" breaks causality.
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
    fn run_until_is_inclusive_and_resumable() {
        let mut sim = simulation(&[(DAY + 1, "c"), (0, "a"), (DAY, "b")]);
        assert_eq!(sim.now(), 0);

        sim.run_until(DAY);
        assert_eq!(sim.model().log, [(0, "a"), (DAY, "b")]);
        assert_eq!(sim.now(), DAY);

        sim.run_until(2 * DAY);
        assert_eq!(sim.model().log, [(0, "a"), (DAY, "b"), (DAY + 1, "c")]);
        assert_eq!(sim.now(), 2 * DAY);
        assert_eq!(sim.events_processed(), 3);
    }

    #[test]
    fn events_scheduled_while_handling_follow_time_then_fifo_order() {
        let mut sim = simulation(&[(10, "spawn"), (10, "peer")]);
        sim.run_until(10);
        assert_eq!(
            sim.model().log,
            [(10, "spawn"), (10, "peer"), (10, "spawned now")]
        );

        sim.run_until(10 + HOUR);
        assert_eq!(sim.model().log.last(), Some(&(10 + HOUR, "spawned later")));
    }

    #[test]
    #[should_panic(expected = "scheduled in the past")]
    fn scheduling_in_the_past_panics() {
        simulation(&[(10, "rewind")]).run_until(10);
    }

    #[test]
    #[should_panic(expected = "before now")]
    fn running_backwards_panics() {
        let mut sim = simulation(&[]);
        sim.run_until(DAY);
        sim.run_until(DAY - 1);
    }
}
