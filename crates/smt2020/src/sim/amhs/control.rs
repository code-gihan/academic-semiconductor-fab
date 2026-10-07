//! Vehicle control: each vehicle plans to its movement authority, the nearest of its goal, the
//! stop node of a zone it does not hold, and the stop point of the vehicle ahead (where that one
//! would come to rest braking now) less that one's length and the gap. Stop points never fall
//! back, so such a plan is safe whatever the vehicle ahead does next. A follower reaching the
//! braking point of that limit while the vehicle ahead still gains ground matches its speed
//! (braking) and from then copies its motion at a fixed distance. Zones are exclusive: a vehicle
//! asks at the braking point for the stop node, waits there if the zone is held, and frees it
//! passing a reset node; waiting vehicles hold no zone, so zones cannot deadlock.

use des_core::{Scheduler, Time};

use super::motion::{EPS, Phase, Plan, SPEED_EPS, profile};
use super::{Amhs, Checks, Decision, Goal, Leader, Mode, Task};
use crate::layout::ZoneRole;
use crate::sim::fab::Event;

/// Events of one vehicle at one instant beyond which its control is taken to loop.
const BURST: u32 = 64;

/// Odometer beyond which a vehicle counts it again from its present rail (mm).
const REBASE: f64 = 1e7;

/// The nearest limit of a vehicle's own: its goal or a stop node of a zone it does not hold.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Limit {
    odo: f64,
    zone: Option<usize>,
}

/// A plan with the decision at its braking point and the mode it follows the vehicle ahead in.
type Planned = (Plan, Option<(Time, Decision)>, Mode);

pub(super) fn ceil(t: f64) -> Time {
    if t.is_finite() {
        t.ceil() as Time
    } else {
        Time::MAX
    }
}

fn floor(t: f64) -> Time {
    if t.is_finite() {
        t.floor() as Time
    } else {
        Time::MAX
    }
}

impl Amhs {
    /// The event of vehicle `id` with `epoch`, at the scheduler's time.
    pub(crate) fn handle(&mut self, id: usize, epoch: u32, sched: &mut Scheduler<Event>) {
        if self.vehicles[id].epoch != epoch || self.failure.is_some() {
            return;
        }
        self.vehicles[id].scheduled = Time::MAX;
        let now = sched.now();
        let burst = &mut self.vehicles[id].burst;
        *burst = if burst.0 == now {
            (now, burst.1 + 1)
        } else {
            (now, 1)
        };
        if burst.1 > BURST {
            self.failure = Some(format!(
                "AMHS: vehicle {id} keeps deciding at {now} ms without moving on"
            ));
            return;
        }
        self.pass_nodes(id, now);
        let checks = self.vehicles[id].checks;
        let at = now as f64;
        if checks.hoist_at <= now {
            self.hoisted(id, at);
        } else if checks.arrive_at <= now {
            self.arrive(id, at);
        } else if let Some((due, decision)) = checks.decision
            && due <= now
        {
            self.decide(id, decision, at, sched);
        } else if checks.leave_at <= now {
            self.pending.push_back(id);
        } else {
            self.schedule(id, sched);
        }
        self.flush(now, sched);
    }

    /// Plans the pending vehicles again at `now`, and the followers of each changed plan after
    /// it.
    pub(super) fn flush(&mut self, now: Time, sched: &mut Scheduler<Event>) {
        while let Some(id) = self.pending.pop_front() {
            if self.failure.is_some() {
                self.pending.clear();
                return;
            }
            self.pass_nodes(id, now);
            let planned = self.plan(id, now as f64);
            self.install(id, now as f64, planned, sched);
        }
    }

    pub(super) fn notify_followers(&mut self, id: usize) {
        for index in 0..self.vehicles[id].followers.len() {
            let follower = self.vehicles[id].followers[index];
            if !self.pending.contains(&follower) {
                self.pending.push_back(follower);
            }
        }
    }

    /// Schedules the vehicle's next event (none while it waits on others), keeping its pending
    /// one if that falls at the same instant.
    pub(super) fn schedule(&mut self, id: usize, sched: &mut Scheduler<Event>) {
        let vehicle = &mut self.vehicles[id];
        let next = vehicle.checks.next();
        let at = if next == Time::MAX {
            Time::MAX
        } else {
            next.max(sched.now())
        };
        if at == vehicle.scheduled {
            return;
        }
        vehicle.epoch = vehicle.epoch.wrapping_add(1);
        vehicle.scheduled = at;
        if at != Time::MAX {
            sched.schedule_at(
                at,
                Event::Vehicle {
                    vehicle: id as u32,
                    epoch: vehicle.epoch,
                },
            );
        }
    }

    /// Moves the vehicle's front over the nodes it passed by `now`.
    pub(super) fn pass_nodes(&mut self, id: usize, now: Time) {
        while self.vehicles[id].checks.node_at <= now {
            let passed = self.vehicles[id].checks.node;
            let (rail, _) = self.vehicles[id].path.pop_front().expect("a rail to leave");
            let (next, _) = *self.vehicles[id].path.front().expect("a rail to enter");
            let node = self.track.rails[rail].to;
            let front = self.on_rail[rail].pop_front();
            debug_assert_eq!(front, Some(id), "the frontmost vehicle leaves first");
            self.on_rail[next].push_back(id);
            let behind = self.track.rails[rail].from;
            self.clearing[behind].retain(|&other| other != id);
            if self.track.nodes[node].diverging {
                self.clearing[node].push(id);
            }
            match self.track.nodes[node].zone {
                Some((zone, ZoneRole::Reset)) if self.vehicles[id].held.contains(&zone) => {
                    self.vehicles[id].held.retain(|&held| held != zone);
                    self.release(zone, passed);
                }
                Some((zone, ZoneRole::Stop)) => debug_assert!(
                    self.vehicles[id].held.contains(&zone),
                    "vehicle {id} entered zone {zone} unheld"
                ),
                _ => {}
            }
            if let Task::ToDropoff(job) = self.vehicles[id].task {
                self.jobs[job].as_mut().expect("job").trail.push(next);
            }
            if self.vehicles[id].path[0].1 > REBASE {
                self.rebase(id);
            }
            self.log_rail(id, now);
            let node = self.node_time(id, passed);
            let checks = &mut self.vehicles[id].checks;
            checks.node = node;
            checks.node_at = ceil(node);
        }
    }

    /// Keeps the vehicle's odometer small, so positions stay exact to well within [`EPS`] however
    /// far it drives: counts it from the start of its present rail again. The subtractions are
    /// exact (Sterbenz) for the positions its path still holds.
    fn rebase(&mut self, id: usize) {
        let shift = self.vehicles[id].path[0].1;
        let vehicle = &mut self.vehicles[id];
        for phase in &mut vehicle.plan.phases {
            phase.s -= shift;
        }
        for entry in &mut vehicle.path {
            entry.1 -= shift;
        }
        vehicle.goal -= shift;
        vehicle.odometer -= shift;
        if let Some(log) = &mut self.log {
            log.base[id] += shift;
        }
        // A leader's front at its odometer x is at x + delta on its follower's.
        if let Some(leader) = &mut vehicle.leader {
            leader.delta -= shift;
        }
        for index in 0..self.vehicles[id].followers.len() {
            let follower = self.vehicles[id].followers[index];
            if let Some(leader) = &mut self.vehicles[follower].leader
                && leader.id == id
            {
                leader.delta += shift;
            }
        }
    }

    /// When the vehicle's front, at `from` or later, passes the end of its rail onto the next of
    /// its path: never if its plan ends there (a goal, a stop node, behind a vehicle) or on the
    /// last rail of its path.
    fn node_time(&self, id: usize, from: f64) -> f64 {
        let vehicle = &self.vehicles[id];
        let (rail, start) = vehicle.path[0];
        let end = start + self.track.rails[rail].length;
        if vehicle.path.len() < 2 || vehicle.plan.end() <= end + EPS {
            return f64::INFINITY;
        }
        vehicle.plan.time_at(end, from).unwrap_or(f64::INFINITY)
    }

    /// Frees `zone` at `at` and grants it to the vehicle waiting longest.
    pub(super) fn release(&mut self, zone: usize, at: f64) {
        let state = &mut self.zones[zone];
        state.holder = None;
        if let Some((next, asked)) = state.queue.pop_front() {
            state.holder = Some(next);
            let vehicle = &mut self.vehicles[next];
            vehicle.held.push(zone);
            vehicle.waiting = None;
            let report = &mut self.stats.report;
            report.zone_waits += 1;
            report.zone_wait += (at - asked).max(0.0).round() as Time;
            self.pending.push_back(next);
        }
    }

    /// Asks for `zone`: granted if free, else queued.
    fn ask_zone(&mut self, id: usize, zone: usize, now: f64) {
        let state = &mut self.zones[zone];
        if state.holder.is_none() && state.queue.is_empty() {
            state.holder = Some(id);
            self.vehicles[id].held.push(zone);
        } else if !state.queue.iter().any(|&(waiting, _)| waiting == id) {
            state.queue.push_back((id, now));
            self.vehicles[id].waiting = Some(zone);
        }
        self.pending.push_back(id);
    }

    /// After a path change: withdraws a request the path no longer needs first and frees the
    /// zones it no longer enters.
    pub(super) fn tidy_zones(&mut self, id: usize, at: f64) {
        let vehicle = &self.vehicles[id];
        let inside = self.track.rails[vehicle.path[0].0].zone;
        let dropped: Vec<usize> = vehicle
            .held
            .iter()
            .copied()
            .filter(|&zone| {
                inside != Some(zone)
                    && !vehicle
                        .path
                        .iter()
                        .any(|&(rail, _)| self.track.rails[rail].zone == Some(zone))
            })
            .collect();
        let first = self.first_stop(id);
        let withdraw = vehicle.waiting.filter(|&zone| first != Some(zone));
        for zone in dropped {
            self.vehicles[id].held.retain(|&held| held != zone);
            self.release(zone, at);
        }
        if let Some(zone) = withdraw {
            self.zones[zone].queue.retain(|&(waiting, _)| waiting != id);
            self.vehicles[id].waiting = None;
        }
    }

    /// Zone of the first stop node ahead whose zone the vehicle does not hold.
    fn first_stop(&self, id: usize) -> Option<usize> {
        let vehicle = &self.vehicles[id];
        let last = vehicle.path.len() - 1;
        vehicle.path.iter().take(last).find_map(|&(rail, _)| {
            match self.track.nodes[self.track.rails[rail].to].zone {
                Some((zone, ZoneRole::Stop)) if !vehicle.held.contains(&zone) => Some(zone),
                _ => None,
            }
        })
    }

    /// The vehicle's own limit: its goal, or the first stop node ahead of a zone it does not
    /// hold.
    fn own_limit(&self, id: usize) -> Limit {
        let vehicle = &self.vehicles[id];
        let last = vehicle.path.len() - 1;
        for &(rail, start) in vehicle.path.iter().take(last) {
            if let Some((zone, ZoneRole::Stop)) = self.track.nodes[self.track.rails[rail].to].zone
                && !vehicle.held.contains(&zone)
            {
                return Limit {
                    odo: start + self.track.rails[rail].length,
                    zone: Some(zone),
                };
            }
        }
        Limit {
            odo: vehicle.goal,
            zone: None,
        }
    }

    /// The nearest vehicle ahead whose tail lies before `limit` and the longest vehicle's reach:
    /// on the vehicle's path, or with its tail still covering a diverging node of it or the end
    /// of it.
    fn find_leader(&self, id: usize, s: f64, limit: f64, now: f64) -> Option<Leader> {
        let vehicle = &self.vehicles[id];
        let horizon = limit + self.reach;
        let (rail, start) = vehicle.path[0];
        let list = &self.on_rail[rail];
        let position = list.iter().position(|&other| other == id).expect("listed");
        if position > 0 {
            let other = list[position - 1];
            return Some(Leader {
                id: other,
                delta: start - self.vehicles[other].path[0].1,
            });
        }
        let mut best: Option<(f64, Leader)> = None;
        // A vehicle whose front is on a rail from the node at `start` on this one's odometer;
        // off the path only while its tail still covers the node (clear at its leave time).
        let consider =
            |other: usize, start: f64, off_path: bool, best: &mut Option<(f64, Leader)>| {
                let ahead = &self.vehicles[other];
                let delta = start - ahead.path[0].1;
                let tail = ahead.plan.state(now).0 + delta - ahead.length;
                if other != id
                    && (!off_path || (tail < start - EPS && tail > s))
                    && best.is_none_or(|(best, _)| tail < best)
                {
                    *best = Some((tail, Leader { id: other, delta }));
                }
            };
        for &(rail, start) in vehicle.path.iter().skip(1) {
            if start > horizon {
                break;
            }
            for &other in &self.clearing[self.track.rails[rail].from] {
                if self.vehicles[other].path[0].0 != rail {
                    consider(other, start, true, &mut best);
                }
            }
            if let Some(&other) = self.on_rail[rail].back() {
                consider(other, start, false, &mut best);
            }
            if best.is_some() {
                return best.map(|(_, leader)| leader);
            }
        }
        // Past the end of the path: vehicles just out of its last node.
        let &(rail, start) = vehicle.path.back().expect("a rail");
        let end = start + self.track.rails[rail].length;
        if end <= horizon {
            let node = self.track.rails[rail].to;
            for &out in &self.track.nodes[node].out {
                if let Some(&other) = self.on_rail[out].back() {
                    consider(other, end, true, &mut best);
                }
            }
            for &other in &self.clearing[node] {
                consider(other, end, true, &mut best);
            }
        }
        best.map(|(_, leader)| leader)
    }

    /// Makes `leader` the vehicle's leader, in the followers' lists too.
    fn link_leader(&mut self, id: usize, leader: Option<Leader>) {
        let old = self.vehicles[id].leader.map(|leader| leader.id);
        let new = leader.map(|leader| leader.id);
        if old != new {
            if let Some(old) = old {
                self.vehicles[old]
                    .followers
                    .retain(|&follower| follower != id);
            }
            if let Some(new) = new {
                self.vehicles[new].followers.push(id);
            }
        }
        self.vehicles[id].leader = leader;
    }

    /// Speed zones (end odometer, limit) of the vehicle's path from `s` to `to`.
    fn zones(&self, id: usize, s: f64, to: f64) -> Vec<(f64, f64)> {
        let vehicle = &self.vehicles[id];
        let mut zones: Vec<(f64, f64)> = Vec::new();
        for &(rail, start) in &vehicle.path {
            let end = (start + self.track.rails[rail].length).min(to);
            if end > s {
                let limit = self.track.rails[rail].limit.min(vehicle.top);
                match zones.last_mut() {
                    Some(last) if last.1 == limit => last.0 = end,
                    _ => zones.push((end, limit)),
                }
            }
            if end >= to {
                break;
            }
        }
        if zones.is_empty() {
            zones.push((to.max(s), vehicle.top));
        }
        zones
    }

    /// The stop point of `leader` at `now` on the follower's odometer, less its length and the
    /// follower's gap: how far the follower may plan to.
    fn behind(&self, id: usize, leader: Leader, now: f64) -> f64 {
        let other = &self.vehicles[leader.id];
        other.plan.stop_point(now, other.dynamics.decel) + leader.delta
            - other.length
            - self.vehicles[id].gap
    }

    /// The vehicle's plan from `now`, the decision at its braking point and its mode; links its
    /// leader.
    fn plan(&mut self, id: usize, now: f64) -> Planned {
        self.account_blocked(id, now);
        let (s, speed) = self.vehicles[id].plan.state(now);
        if matches!(
            self.vehicles[id].task,
            Task::Loading(_) | Task::Unloading(_)
        ) {
            self.link_leader(id, None);
            return (Plan::rest(now, s), None, Mode::Free);
        }
        let own = self.own_limit(id);
        let leader = self.find_leader(id, s, own.odo, now);
        self.link_leader(id, leader);
        let Some((leader, behind)) = leader
            .map(|leader| (leader, self.behind(id, leader, now)))
            .filter(|&(_, behind)| behind < own.odo - EPS)
        else {
            let (plan, decision) = self.free(id, now, s, speed, own);
            return (plan, decision, Mode::Free);
        };
        let other = &self.vehicles[leader.id];
        let settled = other.plan.settled(now, other.dynamics.decel);
        let (leader_s, leader_speed) = other.plan.state(now);
        if self.vehicles[id].mode == Mode::Mirror
            && !settled
            && (speed - leader_speed).abs() <= SPEED_EPS
            && self.mirrorable(id, leader.id)
        {
            let base = other.plan.shifted(now, s - leader_s);
            return match self.mirror(id, now, base, own, leader.id) {
                Some((plan, decision)) => (plan, decision, Mode::Mirror),
                None => self.brake_for_a_ms(id, now, s, speed, behind),
            };
        }
        let dynamics = self.vehicles[id].dynamics;
        let plan = profile(now, s, speed, &self.zones(id, s, behind.max(s)), dynamics);
        let decision = (!settled).then(|| {
            let at = plan
                .time_stop_point_at(behind, dynamics.decel, now)
                .unwrap_or(plan.end_time());
            (floor(at).max(now as Time), Decision::Catch)
        });
        (plan, decision, Mode::Free)
    }

    /// The vehicle's own plan to `own`, and the decision at its braking point.
    fn free(
        &self,
        id: usize,
        now: f64,
        s: f64,
        speed: f64,
        own: Limit,
    ) -> (Plan, Option<(Time, Decision)>) {
        let vehicle = &self.vehicles[id];
        let plan = profile(
            now,
            s,
            speed,
            &self.zones(id, s, own.odo.max(s)),
            vehicle.dynamics,
        );
        let decision = self.own_decision(id, own).map(|decision| {
            let at = plan
                .time_stop_point_at(own.odo, vehicle.dynamics.decel, now)
                .unwrap_or(plan.end_time());
            (floor(at).max(now as Time), decision)
        });
        (plan, decision)
    }

    /// What the vehicle decides at the braking point for its own limit: asks for the zone, or
    /// carries on past a roaming point; nothing at a port or while it waits for the zone.
    fn own_decision(&self, id: usize, own: Limit) -> Option<Decision> {
        let vehicle = &self.vehicles[id];
        match own.zone {
            Some(zone) if vehicle.waiting != Some(zone) => Some(Decision::Request(zone)),
            Some(_) => None,
            None if vehicle.goal_kind == Goal::Waypoint => Some(Decision::Extend),
            None => None,
        }
    }

    /// Following `leader` with `base` (its motion on the vehicle's odometer from `now`), cut
    /// short where the vehicle's own limit binds; planned again once `leader` brakes for good,
    /// and where a speed limit would bind, which must not happen within this ms (none then).
    fn mirror(
        &self,
        id: usize,
        now: f64,
        base: Plan,
        own: Limit,
        leader: usize,
    ) -> Option<(Plan, Option<(Time, Decision)>)> {
        let decel = self.vehicles[id].dynamics.decel;
        let mut decision: Option<(Time, Decision)> = None;
        let earliest = |at: Time, kind: Decision, decision: &mut Option<(Time, Decision)>| {
            let at = at.max(now as Time);
            if decision.is_none_or(|(earlier, _)| at < earlier) {
                *decision = Some((at, kind));
            }
        };
        let cut = base
            .time_stop_point_at(own.odo, decel, now)
            .filter(|&at| base.end() > own.odo + EPS || at < base.end_time());
        let plan = match cut {
            Some(at) => {
                if let Some(kind) = self.own_decision(id, own) {
                    earliest(floor(at), kind, &mut decision);
                }
                braking_after(&base, at, decel)
            }
            None => base,
        };
        let other = &self.vehicles[leader];
        let settles = other.plan.settling_time(other.dynamics.decel);
        if settles > now && settles < plan.end_time() {
            // The copy holds through it: look again once it has begun.
            earliest(ceil(settles), Decision::Recheck, &mut decision);
        }
        if !self.uniform
            && let Some(at) = self.limit_rise(id, &plan, now)
        {
            if floor(at) <= now as Time {
                return None;
            }
            earliest(floor(at), Decision::Recheck, &mut decision);
        }
        Some((plan, decision))
    }

    /// Where copying the vehicle ahead would break a speed limit within this ms: the vehicle's
    /// own plan to its stop point less that one's length and its gap, looked at again in a ms
    /// (when the vehicles have drifted apart).
    fn brake_for_a_ms(&self, id: usize, now: f64, s: f64, speed: f64, behind: f64) -> Planned {
        let dynamics = self.vehicles[id].dynamics;
        let plan = profile(now, s, speed, &self.zones(id, s, behind.max(s)), dynamics);
        (plan, Some((now as Time + 1, Decision::Recheck)), Mode::Free)
    }

    /// First time the plan runs faster than a rail allows before reaching a faster rail.
    fn limit_rise(&self, id: usize, plan: &Plan, now: f64) -> Option<f64> {
        let vehicle = &self.vehicles[id];
        let mut first: Option<f64> = None;
        for (index, &(rail, _)) in vehicle.path.iter().enumerate() {
            let Some(&(next, start)) = vehicle.path.get(index + 1) else {
                break;
            };
            if start > plan.end() {
                break;
            }
            let slow = self.track.rails[rail].limit.min(vehicle.top);
            let fast = self.track.rails[next].limit.min(vehicle.top);
            if fast > slow
                && let Some(at) = plan.time_above(slow, now)
                && plan.state(at).0 < start - EPS
            {
                first = Some(first.map_or(at, |first| first.min(at)));
            }
        }
        first
    }

    /// Whether the vehicle may copy `leader`: no faster or harder accelerating than itself, and
    /// no chain of copies leading back to it.
    fn mirrorable(&self, id: usize, leader: usize) -> bool {
        let (vehicle, other) = (&self.vehicles[id], &self.vehicles[leader]);
        if other.dynamics.accel > vehicle.dynamics.accel
            || other.dynamics.decel > vehicle.dynamics.decel
            || other.top > vehicle.top
        {
            return false;
        }
        let mut at = leader;
        for _ in 0..self.vehicles.len() {
            if at == id {
                return false;
            }
            match (self.vehicles[at].mode, self.vehicles[at].leader) {
                (Mode::Mirror, Some(next)) => at = next.id,
                _ => return true,
            }
        }
        false
    }

    /// Installs `plan` with `decision` and the other checks.
    pub(super) fn set_plan(
        &mut self,
        id: usize,
        now: f64,
        plan: Plan,
        decision: Option<(Time, Decision)>,
        mode: Mode,
    ) {
        let leave_at = self.leave_time(id, now);
        let (end, end_time) = (plan.end(), plan.end_time());
        self.vehicles[id].plan = plan;
        let node = self.node_time(id, now);
        let vehicle = &mut self.vehicles[id];
        let hoisting = matches!(vehicle.task, Task::Loading(_) | Task::Unloading(_));
        let arrive_at = if vehicle.goal_kind == Goal::Port
            && (end - vehicle.goal).abs() <= EPS
            && matches!(vehicle.task, Task::ToPickup(_) | Task::ToDropoff(_))
        {
            ceil(end_time)
        } else {
            Time::MAX
        };
        vehicle.checks = Checks {
            node,
            node_at: ceil(node),
            decision,
            leave_at,
            arrive_at,
            hoist_at: if hoisting {
                vehicle.checks.hoist_at
            } else {
                Time::MAX
            },
        };
        vehicle.mode = mode;
        // Coming to rest short of the goal is standing blocked.
        vehicle.blocked_from = if !hoisting && end < vehicle.goal - EPS {
            end_time.max(now)
        } else {
            f64::INFINITY
        };
        self.check_deadlock(id, now);
    }

    /// Adds the time the vehicle stood blocked up to `now`.
    fn account_blocked(&mut self, id: usize, now: f64) {
        let vehicle = &mut self.vehicles[id];
        if vehicle.blocked_from < now {
            self.stats.report.blocked += (now - vehicle.blocked_from).round() as Time;
        }
        vehicle.blocked_from = f64::INFINITY;
    }

    /// When the leader's tail clears the diverging node where their paths part.
    fn leave_time(&self, id: usize, now: f64) -> Time {
        let Some(leader) = self.vehicles[id].leader else {
            return Time::MAX;
        };
        let (vehicle, other) = (&self.vehicles[id], &self.vehicles[leader.id]);
        let parting = match vehicle
            .path
            .iter()
            .position(|&(rail, _)| rail == other.path[0].0)
        {
            // Already off the path: its tail still covers the node where it left.
            None => Some(other.path[0].1),
            Some(at) => other
                .path
                .iter()
                .skip(1)
                .zip(vehicle.path.iter().skip(at + 1))
                .find(|(theirs, ours)| theirs.0 != ours.0)
                .map(|(theirs, _)| theirs.1),
        };
        parting
            .and_then(|node| other.plan.time_at(node + other.length, now))
            .map_or(Time::MAX, ceil)
    }

    /// Fails the run if the vehicle, about to stand blocked, waits on a chain of vehicles that
    /// leads back to it.
    fn check_deadlock(&mut self, id: usize, now: f64) {
        let mut at = id;
        for _ in 0..=self.vehicles.len() {
            let vehicle = &self.vehicles[at];
            let blocked = vehicle.blocked_from.is_finite() && vehicle.checks.next() == Time::MAX;
            if !blocked {
                return;
            }
            let next = match vehicle.waiting {
                Some(zone) => self.zones[zone].holder,
                None => vehicle.leader.map(|leader| leader.id),
            };
            match next {
                Some(next) if next == id => {
                    self.failure = Some(format!(
                        "AMHS deadlock at {now:.0} ms: vehicles from {id} wait on each other"
                    ));
                    return;
                }
                Some(next) => at = next,
                None => return,
            }
        }
    }

    /// A decision at a braking point.
    fn decide(&mut self, id: usize, decision: Decision, now: f64, sched: &mut Scheduler<Event>) {
        match decision {
            Decision::Request(zone) => self.ask_zone(id, zone, now),
            Decision::Extend => {
                self.extend_roam(id, now);
                self.pending.push_back(id);
            }
            Decision::Catch => self.catch(id, now, sched),
            Decision::Recheck => {
                self.vehicles[id].mode = Mode::Free;
                self.pending.push_back(id);
            }
        }
    }

    /// The braking point for the stop point of the vehicle ahead is reached while that one still
    /// gains ground: at equal speed copy it from now; faster, brake to its speed and copy it from
    /// then; slower, plan again to its present stop point, or with no room copy its
    /// accelerations with the speed deficit until its next phase.
    fn catch(&mut self, id: usize, now: f64, sched: &mut Scheduler<Event>) {
        let (s, speed) = self.vehicles[id].plan.state(now);
        let own = self.own_limit(id);
        let leader = self.find_leader(id, s, own.odo, now);
        self.link_leader(id, leader);
        let bound = leader
            .map(|leader| (leader, self.behind(id, leader, now)))
            .filter(|&(_, behind)| behind < own.odo - EPS);
        let decel = self.vehicles[id].dynamics.decel;
        let settled = |leader: Leader| {
            let other = &self.vehicles[leader.id];
            other.plan.settled(now, other.dynamics.decel)
        };
        let Some((leader, behind)) = bound.filter(|&(leader, _)| !settled(leader)) else {
            // Its own plan, or one behind a vehicle braking for good.
            self.vehicles[id].mode = Mode::Free;
            self.pending.push_back(id);
            return;
        };
        if !self.mirrorable(id, leader.id) {
            let planned = self.plan_behind(id, now, s, speed, behind);
            self.install(id, now, planned, sched);
            return;
        }
        let (leader_s, leader_speed) = self.vehicles[leader.id].plan.state(now);
        if (speed - leader_speed).abs() <= SPEED_EPS {
            self.vehicles[id].mode = Mode::Mirror;
            self.pending.push_back(id);
            return;
        }
        let (plan, decision, mode) = if speed > leader_speed {
            let other = &self.vehicles[leader.id];
            match other.plan.time_matching(now, speed, decel) {
                // It comes to rest before the vehicle ahead gets as fast.
                None => self.plan_behind(id, now, s, speed, behind),
                Some(matched) => {
                    // Braking until the speeds match, copying from then.
                    let braking = Plan::braking(now, s, speed, decel);
                    let (at_match, _) = braking.state(matched);
                    let copy = other
                        .plan
                        .shifted(matched, at_match - other.plan.state(matched).0);
                    let mut phases = vec![braking.phases[0]];
                    phases.extend(copy.phases);
                    match self.mirror(id, now, Plan { phases }, own, leader.id) {
                        Some((plan, decision)) => (plan, decision, Mode::Mirror),
                        None => self.brake_for_a_ms(id, now, s, speed, behind),
                    }
                }
            }
        } else {
            let dynamics = self.vehicles[id].dynamics;
            let plan = profile(now, s, speed, &self.zones(id, s, behind.max(s)), dynamics);
            let brake = plan
                .time_stop_point_at(behind, decel, now)
                .unwrap_or(plan.end_time());
            if floor(brake) > now as Time {
                (plan, Some((floor(brake), Decision::Catch)), Mode::Free)
            } else {
                // No room: the accelerations ahead at the speed deficit, until its next phase.
                let other = &self.vehicles[leader.id];
                let base = with_deficit(&other.plan, now, s - leader_s, leader_speed - speed);
                let next_phase = other
                    .plan
                    .phases
                    .iter()
                    .map(|phase| phase.t)
                    .find(|&t| t > now)
                    .unwrap_or(f64::INFINITY);
                match self.mirror(id, now, base, own, leader.id) {
                    Some((plan, decision)) => {
                        let recheck = floor(next_phase).max(now as Time + 1);
                        let decision = match decision {
                            Some((at, kind)) if at <= recheck => Some((at, kind)),
                            _ if next_phase.is_finite() => Some((recheck, Decision::Recheck)),
                            decision => decision,
                        };
                        (plan, decision, Mode::Free)
                    }
                    None => self.brake_for_a_ms(id, now, s, speed, behind),
                }
            }
        };
        self.install(id, now, (plan, decision, mode), sched);
    }

    /// Puts a plan in force, telling the followers if the vehicle moves otherwise now.
    fn install(&mut self, id: usize, now: f64, planned: Planned, sched: &mut Scheduler<Event>) {
        let (plan, decision, mode) = planned;
        let changed = !self.vehicles[id].plan.same_motion(&plan, now);
        self.log_motion(id, now, &plan);
        self.set_plan(id, now, plan, decision, mode);
        self.schedule(id, sched);
        if changed {
            self.notify_followers(id);
        }
    }

    /// Where the vehicle can neither copy the vehicle ahead nor match its speed by braking: its own
    /// plan to its stop point less that one's length and its gap, looked at again once it comes
    /// to rest (in a ms at the earliest).
    fn plan_behind(&self, id: usize, now: f64, s: f64, speed: f64, behind: f64) -> Planned {
        let dynamics = self.vehicles[id].dynamics;
        let plan = profile(now, s, speed, &self.zones(id, s, behind.max(s)), dynamics);
        let at = ceil(plan.end_time()).max(now as Time + 1);
        (plan, Some((at, Decision::Recheck)), Mode::Free)
    }
}

/// `plan` up to `at`, then braking at `decel` to rest.
fn braking_after(plan: &Plan, at: f64, decel: f64) -> Plan {
    let mut phases: Vec<Phase> = plan
        .phases
        .iter()
        .copied()
        .take_while(|phase| phase.t < at)
        .collect();
    let (s, v) = plan.state(at);
    if v <= 0.0 {
        phases.push(Phase {
            t: at,
            s,
            v: 0.0,
            a: 0.0,
        });
    } else {
        phases.push(Phase {
            t: at,
            s,
            v,
            a: -decel,
        });
        phases.push(Phase {
            t: at + v / decel,
            s: s + v * v / (2.0 * decel),
            v: 0.0,
            a: 0.0,
        });
    }
    Plan { phases }
}

/// The accelerations of `plan` from `from` at its speed less `deficit`, shifted by `ds`; at rest
/// from where that speed would turn negative.
fn with_deficit(plan: &Plan, from: f64, ds: f64, deficit: f64) -> Plan {
    let base = plan.shifted(from, ds);
    let mut phases: Vec<Phase> = Vec::with_capacity(base.phases.len());
    let (mut s, mut v) = (base.phases[0].s, (base.phases[0].v - deficit).max(0.0));
    for (index, phase) in base.phases.iter().enumerate() {
        if index + 1 == base.phases.len() || (v <= 0.0 && phase.a <= 0.0) {
            phases.push(Phase {
                t: phase.t,
                s,
                v: 0.0,
                a: 0.0,
            });
            break;
        }
        phases.push(Phase {
            t: phase.t,
            s,
            v,
            a: phase.a,
        });
        let length = base.phases[index + 1].t - phase.t;
        let stops = if phase.a < 0.0 {
            -v / phase.a
        } else {
            f64::INFINITY
        };
        if stops < length {
            phases.push(Phase {
                t: phase.t + stops,
                s: s + v * stops + 0.5 * phase.a * stops * stops,
                v: 0.0,
                a: 0.0,
            });
            break;
        }
        s += v * length + 0.5 * phase.a * length * length;
        v = (v + phase.a * length).max(0.0);
    }
    Plan { phases }
}
