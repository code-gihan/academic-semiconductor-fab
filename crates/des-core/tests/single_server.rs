//! End-to-end check of the DES core on a FIFO single-server queue, whose departure times are
//! known exactly: d_k = max(a_k, d_{k-1}) + s (Lindley recursion).

use std::collections::VecDeque;

use des_core::{MINUTE, Model, Outcome, Scheduler, Simulation, Time};

enum Event {
    Arrival(usize),
    Departure,
}

struct SingleServer {
    arrivals: Vec<Time>,
    service: Time,
    waiting: VecDeque<usize>,
    in_service: Option<usize>,
    departures: Vec<(usize, Time)>,
}

impl SingleServer {
    fn start(&mut self, id: usize, sched: &mut Scheduler<Event>) {
        self.in_service = Some(id);
        sched.schedule_in(self.service, Event::Departure);
    }
}

impl Model for SingleServer {
    type Event = Event;

    fn init(&mut self, sched: &mut Scheduler<Event>) {
        for (id, &time) in self.arrivals.iter().enumerate() {
            sched.schedule_at(time, Event::Arrival(id));
        }
    }

    fn handle(&mut self, event: Event, sched: &mut Scheduler<Event>) {
        match event {
            Event::Arrival(id) if self.in_service.is_none() => self.start(id, sched),
            Event::Arrival(id) => self.waiting.push_back(id),
            Event::Departure => {
                let id = self
                    .in_service
                    .take()
                    .expect("departure from an idle server");
                self.departures.push((id, sched.now()));
                if let Some(next) = self.waiting.pop_front() {
                    self.start(next, sched);
                }
            }
        }
    }
}

#[test]
fn departures_match_lindley_recursion() {
    // Simultaneous arrivals, a backlog, an arrival exactly at a departure, and idle gaps.
    let arrivals: Vec<Time> = [0, 0, 3, 4, 20, 30, 30, 31, 60]
        .iter()
        .map(|&m| m * MINUTE)
        .collect();
    let service = 5 * MINUTE;
    let mut sim = Simulation::new(SingleServer {
        arrivals: arrivals.clone(),
        service,
        waiting: VecDeque::new(),
        in_service: None,
        departures: Vec::new(),
    });
    assert_eq!(sim.run(), Outcome::Exhausted);

    let mut last_departure = 0;
    let expected: Vec<(usize, Time)> = arrivals
        .iter()
        .enumerate()
        .map(|(id, &arrival)| {
            last_departure = arrival.max(last_departure) + service;
            (id, last_departure)
        })
        .collect();
    assert_eq!(sim.model().departures, expected);
    assert_eq!(sim.events_processed(), 2 * arrivals.len() as u64);
    assert_eq!(sim.now(), last_departure);
}
