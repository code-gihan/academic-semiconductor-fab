//! Transports and idle vehicles: a transport goes to an idle vehicle at once (the nearest, or the
//! SMAT2022 simulator's bay rings) or waits for the next vehicle freed, oldest first. A vehicle
//! drives to the pickup port, hoists the FOUP up, drives to the drop-off port (which may change
//! until it gets there) and hoists it down. Idle vehicles roam their intrabay between its two
//! roaming points; past the bay's limit the longest roaming one moves to the neighbor bay roamed
//! least.

use des_core::{Scheduler, Time};

use super::control::ceil;
use super::motion::run_time;
use super::stats::{self, Delivered, End};
use super::{Amhs, Dispatch, Goal, Job, Notice, Task};
use crate::layout::{BayId, LinkId, PortId, PortKind};
use crate::sim::fab::Event;

impl Amhs {
    /// Starts the vehicles at time 0, each roaming its bay.
    pub(crate) fn init(&mut self, sched: &mut Scheduler<Event>) {
        for id in 0..self.vehicles.len() {
            self.become_idle(id, 0.0);
        }
        self.flush(sched.now(), sched);
    }

    /// A transport of `lot` from port `from` to port `to`, from now.
    pub(crate) fn request(
        &mut self,
        lot: usize,
        from: PortId,
        to: PortId,
        sched: &mut Scheduler<Event>,
    ) -> usize {
        let now = sched.now() as f64;
        let job = Job {
            lot,
            from,
            to,
            vehicle: None,
            created: now,
            assigned: now,
            reached: now,
            loaded: now,
            arrived: now,
            trail: Vec::new(),
        };
        let id = match self.free_jobs.pop() {
            Some(id) => {
                self.jobs[id] = Some(job);
                id
            }
            None => {
                self.jobs.push(Some(job));
                self.jobs.len() - 1
            }
        };
        match self.choose(from, now) {
            Some(vehicle) => self.assign(vehicle, id, now),
            None => self.waiting.push_back(id),
        }
        self.flush(sched.now(), sched);
        id
    }

    /// Sends a transport to port `to` instead; false once its drop-off has begun.
    pub(crate) fn retarget(
        &mut self,
        job: usize,
        to: PortId,
        sched: &mut Scheduler<Event>,
    ) -> bool {
        let now = sched.now() as f64;
        let state = self.jobs[job].as_mut().expect("job");
        let vehicle = state.vehicle;
        match vehicle.map(|vehicle| self.vehicles[vehicle].task) {
            Some(Task::Unloading(_)) => return false,
            Some(Task::ToDropoff(_)) => {
                state.to = to;
                let vehicle = vehicle.expect("vehicle");
                let (rail, offset) = self.port(to);
                self.set_path(vehicle, now, rail, offset, Goal::Port);
                self.pending.push_back(vehicle);
            }
            _ => state.to = to,
        }
        self.flush(sched.now(), sched);
        true
    }

    /// Drops a transport whose FOUP is still at its pickup port; false once the pickup has
    /// begun.
    pub(crate) fn cancel(&mut self, job: usize, sched: &mut Scheduler<Event>) -> bool {
        let now = sched.now() as f64;
        let vehicle = self.jobs[job].as_ref().expect("job").vehicle;
        match vehicle {
            None => self.waiting.retain(|&waiting| waiting != job),
            Some(vehicle) if self.vehicles[vehicle].task == Task::ToPickup(job) => {
                self.become_idle(vehicle, now);
            }
            Some(_) => return false,
        }
        self.jobs[job] = None;
        self.free_jobs.push(job);
        self.flush(sched.now(), sched);
        true
    }

    /// The idle vehicle to take a transport from `port`.
    fn choose(&self, port: PortId, now: f64) -> Option<usize> {
        let (rail, offset) = self.port(port);
        let distance = |vehicle: usize| {
            let (_, stop_rail, stop_offset) = self.stop_position(vehicle, now);
            self.track.distance(stop_rail, stop_offset, rail, offset)
        };
        let nearest = |vehicles: &mut dyn Iterator<Item = usize>| {
            vehicles
                .map(|vehicle| (distance(vehicle), vehicle))
                .min_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)))
                .map(|(_, vehicle)| vehicle)
        };
        match self.dispatch {
            Dispatch::Nearest => nearest(
                &mut (0..self.vehicles.len()).filter(|&id| self.vehicles[id].task == Task::Idle),
            ),
            Dispatch::Bay => {
                // Bays breadth first from the pickup's: the longest roaming vehicle of the first
                // with one that can still stop before the pickup.
                let mut seen = vec![false; self.bays.len()];
                let mut queue = std::collections::VecDeque::from([self.track.rails[rail].bay]);
                seen[self.track.rails[rail].bay] = true;
                while let Some(bay) = queue.pop_front() {
                    if let Some(&vehicle) = self.bays[bay]
                        .roaming
                        .iter()
                        .find(|&&vehicle| !self.overshoots(vehicle, rail, offset, now))
                    {
                        return Some(vehicle);
                    }
                    for &neighbor in &self.layout().bays[bay].neighbors {
                        if !seen[neighbor] {
                            seen[neighbor] = true;
                            queue.push_back(neighbor);
                        }
                    }
                }
                // Bays apart from the others: the nearest idle vehicle.
                nearest(
                    &mut (0..self.vehicles.len())
                        .filter(|&id| self.vehicles[id].task == Task::Idle),
                )
            }
        }
    }

    /// Whether the vehicle approaches `offset` on `rail` too fast to stop there.
    fn overshoots(&self, id: usize, rail: LinkId, offset: f64, now: f64) -> bool {
        let vehicle = &self.vehicles[id];
        let (front, start) = vehicle.path[0];
        let (s, _) = vehicle.plan.state(now.max(vehicle.plan.start()));
        let (_, stop_rail, stop_offset) = self.stop_position(id, now);
        front == rail && s - start <= offset && (stop_rail != rail || stop_offset > offset)
    }

    /// Where the vehicle would come to rest braking now: path index, rail and offset.
    fn stop_position(&self, id: usize, now: f64) -> (usize, LinkId, f64) {
        let vehicle = &self.vehicles[id];
        let stop = vehicle.plan.stop_point(now, vehicle.dynamics.decel);
        let last = vehicle.path.len() - 1;
        for (index, &(rail, start)) in vehicle.path.iter().enumerate() {
            let length = self.track.rails[rail].length;
            if stop <= start + length || index == last {
                return (index, rail, (stop - start).clamp(0.0, length));
            }
        }
        unreachable!("paths are not empty")
    }

    /// The vehicle does `task` from `now` on.
    fn set_task(&mut self, id: usize, task: Task, now: f64) {
        self.vehicles[id].task = task;
        self.log_task(id, now);
    }

    /// The vehicle takes transport `job`.
    fn assign(&mut self, id: usize, job: usize, now: f64) {
        let bay = self.vehicles[id].bay;
        self.bays[bay].roaming.retain(|&vehicle| vehicle != id);
        self.account(id, now);
        self.set_task(id, Task::ToPickup(job), now);
        let state = self.jobs[job].as_mut().expect("job");
        state.vehicle = Some(id);
        state.assigned = now;
        let from = state.from;
        let (rail, offset) = self.port(from);
        self.set_path(id, now, rail, offset, Goal::Port);
        self.pending.push_back(id);
    }

    /// The vehicle's path from where it can stop to `offset` into rail `to`.
    fn set_path(&mut self, id: usize, now: f64, to: LinkId, offset: f64, goal: Goal) {
        let (index, rail, from) = self.stop_position(id, now);
        let mut rails = Vec::new();
        self.track.route(rail, from, to, offset, &mut rails);
        let vehicle = &mut self.vehicles[id];
        vehicle.path.truncate(index + 1);
        for rail in rails {
            let &(last, start) = vehicle.path.back().expect("a rail");
            vehicle
                .path
                .push_back((rail, start + self.track.rails[last].length));
        }
        let &(_, start) = vehicle.path.back().expect("a rail");
        vehicle.goal = start + offset;
        vehicle.goal_kind = goal;
        self.tidy_zones(id, now);
        self.notify_followers(id);
    }

    /// The vehicle is free: it takes the oldest waiting transport, or roams.
    fn become_idle(&mut self, id: usize, now: f64) {
        self.account(id, now);
        self.set_task(id, Task::Idle, now);
        match self.waiting.pop_front() {
            Some(job) => self.assign(id, job, now),
            None => self.roam(id, now),
        }
    }

    /// Sends an idle vehicle roaming: its present intrabay, or from an interbay the neighbor
    /// intrabay roamed least (nearest among equals).
    fn roam(&mut self, id: usize, now: f64) {
        let (_, rail, offset) = self.stop_position(id, now);
        let here = self.track.rails[rail].bay;
        let bay = if self.bays[here].points.is_some() {
            Some(here)
        } else {
            let neighbors = &self.layout().bays[here].neighbors;
            let candidates: Vec<BayId> =
                if neighbors.iter().any(|&bay| self.bays[bay].points.is_some()) {
                    neighbors.clone()
                } else {
                    (0..self.bays.len()).collect()
                };
            candidates
                .into_iter()
                .filter_map(|bay| {
                    let [(point, at), _] = self.bays[bay].points?;
                    Some((
                        self.bays[bay].roaming.len(),
                        self.track.distance(rail, offset, point, at),
                        bay,
                    ))
                })
                .min_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)).then(a.2.cmp(&b.2)))
                .map(|(_, _, bay)| bay)
        };
        match bay {
            Some(bay) => self.join_bay(id, bay, now),
            None => {
                // No bay to roam: stand where it can stop.
                let (_, rail, offset) = self.stop_position(id, now);
                self.set_path(id, now, rail, offset, Goal::Port);
                self.pending.push_back(id);
            }
        }
    }

    /// The vehicle roams `bay`, heading for the nearer of its points; past the limit the bay's
    /// longest roaming vehicle moves to the neighbor bay roamed least that has room.
    fn join_bay(&mut self, id: usize, bay: BayId, now: f64) {
        self.vehicles[id].bay = bay;
        self.bays[bay].roaming.push_back(id);
        let points = self.bays[bay].points.expect("roaming points");
        let (_, rail, offset) = self.stop_position(id, now);
        let distance = |(point, at): (LinkId, f64)| self.track.distance(rail, offset, point, at);
        let point = usize::from(distance(points[1]) < distance(points[0]));
        self.vehicles[id].point = point;
        let (rail, at) = points[point];
        self.set_path(id, now, rail, at, Goal::Waypoint);
        self.pending.push_back(id);
        if self.bays[bay].roaming.len() > self.roam_limit as usize {
            let neighbor = self.layout().bays[bay]
                .neighbors
                .iter()
                .copied()
                .filter(|&neighbor| {
                    self.bays[neighbor].points.is_some()
                        && self.bays[neighbor].roaming.len() < self.roam_limit as usize
                })
                .min_by_key(|&neighbor| self.bays[neighbor].roaming.len());
            if let Some(neighbor) = neighbor {
                let oldest = self.bays[bay].roaming.pop_front().expect("roaming");
                self.join_bay(oldest, neighbor, now);
            }
        }
    }

    /// At the braking point for its roaming point the vehicle carries on to the other one.
    pub(super) fn extend_roam(&mut self, id: usize, now: f64) {
        let vehicle = &self.vehicles[id];
        let Some(points) = self.bays[vehicle.bay].points else {
            return;
        };
        let point = 1 - vehicle.point;
        let (to, offset) = points[point];
        let &(last, start) = vehicle.path.back().expect("a rail");
        let mut rails = Vec::new();
        self.track
            .route(last, vehicle.goal - start, to, offset, &mut rails);
        if rails.is_empty() && offset <= vehicle.goal - start {
            // The other point lies right here: once around.
            self.track
                .route(last, vehicle.goal - start + 1.0, to, offset, &mut rails);
        }
        let vehicle = &mut self.vehicles[id];
        vehicle.point = point;
        for rail in rails {
            let &(last, start) = vehicle.path.back().expect("a rail");
            vehicle
                .path
                .push_back((rail, start + self.track.rails[last].length));
        }
        let &(_, start) = vehicle.path.back().expect("a rail");
        vehicle.goal = start + offset;
        self.tidy_zones(id, now);
        self.notify_followers(id);
    }

    /// The vehicle stands at its port: the hoist begins.
    pub(super) fn arrive(&mut self, id: usize, now: f64) {
        self.account(id, now);
        let task = match self.vehicles[id].task {
            Task::ToPickup(job) => {
                self.jobs[job].as_mut().expect("job").reached = now;
                Task::Loading(job)
            }
            Task::ToDropoff(job) => {
                self.jobs[job].as_mut().expect("job").arrived = now;
                Task::Unloading(job)
            }
            task => unreachable!("arrived while {task:?}"),
        };
        self.set_task(id, task, now);
        self.vehicles[id].checks.hoist_at = ceil(now) + self.hoist;
        self.pending.push_back(id);
    }

    /// A hoist ends: the FOUP is up (drive to the drop-off) or down (delivered; the vehicle is
    /// free).
    pub(super) fn hoisted(&mut self, id: usize, now: f64) {
        self.account(id, now);
        self.vehicles[id].checks.hoist_at = Time::MAX;
        match self.vehicles[id].task {
            Task::Loading(job) => {
                let state = self.jobs[job].as_mut().expect("job");
                state.loaded = now;
                state.trail.clear();
                state.trail.push(self.vehicles[id].path[0].0);
                let (lot, from, to) = (state.lot, state.from, state.to);
                self.notices.push(Notice::PickedUp {
                    job,
                    lot,
                    port: from,
                });
                self.set_task(id, Task::ToDropoff(job), now);
                let (rail, offset) = self.port(to);
                self.set_path(id, now, rail, offset, Goal::Port);
                self.pending.push_back(id);
            }
            Task::Unloading(job) => {
                let state = self.jobs[job].take().expect("job");
                self.free_jobs.push(job);
                self.delivered(&state, now);
                self.notices.push(Notice::Delivered {
                    job,
                    lot: state.lot,
                    port: state.to,
                });
                self.become_idle(id, now);
            }
            task => unreachable!("hoisted while {task:?}"),
        }
    }

    /// Counts a delivery done at `now`.
    fn delivered(&mut self, job: &Job, now: f64) {
        let layout = self.layout();
        let end = |port: PortId| match layout.ports[port].kind {
            PortKind::Tool(_) => End::Tool,
            PortKind::Buffer(_) | PortKind::Stocker(_) => End::Buffer,
            PortKind::Commit(_) => End::Commit,
            PortKind::Complete(_) => End::Complete,
        };
        let (from_rail, from_offset) = self.port(job.from);
        let (to_rail, to_offset) = self.port(job.to);
        let vehicle = &self.vehicles[job.vehicle.expect("vehicle")];
        // The loaded drive alone on the rails: from rest at the pickup to rest at the drop-off.
        let mut zones: Vec<(f64, f64)> = Vec::with_capacity(job.trail.len());
        let mut start = 0.0;
        for (index, &rail) in job.trail.iter().enumerate() {
            let end = if index + 1 == job.trail.len() {
                start + to_offset
            } else {
                start + self.track.rails[rail].length
            };
            zones.push((end, self.track.rails[rail].limit.min(vehicle.top)));
            start += self.track.rails[rail].length;
        }
        let unobstructed = run_time(from_offset, &zones, vehicle.dynamics);
        let distance = layout.bay_distance(
            self.track.rails[from_rail].bay,
            self.track.rails[to_rail].bay,
        );
        let round = |time: f64| time.round() as Time;
        self.stats.delivered(&Delivered {
            from: end(job.from),
            to: end(job.to),
            vehicle_wait: round(job.assigned - job.created),
            empty_drive: round(job.reached - job.assigned),
            loaded_drive: round(job.arrived - job.loaded),
            unobstructed: round(unobstructed),
            delivery: round(now - job.created),
            class: stats::class(distance),
        });
    }
}

impl Amhs {
    /// Transports waiting for a vehicle.
    pub(crate) fn backlog(&self) -> usize {
        self.waiting.len()
    }

    /// Shortest rail distance from port `from` to port `to` (mm).
    pub(crate) fn distance(&self, from: PortId, to: PortId) -> f64 {
        let (from_rail, from_offset) = self.port(from);
        let (to_rail, to_offset) = self.port(to);
        self.track
            .distance(from_rail, from_offset, to_rail, to_offset)
    }

    /// The vehicle holding the FOUP of transport `job`: from the end of its pickup to the end of
    /// its drop-off.
    pub(crate) fn carrier(&self, job: usize) -> Option<usize> {
        let vehicle = self.jobs[job].as_ref()?.vehicle?;
        matches!(
            self.vehicles[vehicle].task,
            Task::ToDropoff(_) | Task::Unloading(_)
        )
        .then_some(vehicle)
    }
}
