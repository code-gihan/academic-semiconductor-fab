//! Safety of the AMHS at an instant, for tests: every front lies on the rail it is listed on, a
//! front inside a zone holds it, zone grants and requests agree, roaming vehicles are listed once
//! in their bay, and every vehicle can stop behind each one ahead on its path, or whose tail still
//! covers a diverging node of it or its end, by that one's length and its own gap (moving block;
//! at rest, the distance itself).

use des_core::Time;

use super::motion::EPS;
use super::{Amhs, Task};

/// Slack of the comparisons (mm, mm/ms): positions are the same within it.
const SLACK: f64 = EPS;

impl Amhs {
    pub(crate) fn check(&self, now: Time) -> Result<(), String> {
        let now = now as f64;
        let state = |id: usize| {
            let vehicle = &self.vehicles[id];
            let t = now.max(vehicle.plan.start());
            let (s, speed) = vehicle.plan.state(t);
            (s, speed, vehicle.plan.stop_point(t, vehicle.dynamics.decel))
        };
        for (zone, state) in self.zones.iter().enumerate() {
            if let Some(holder) = state.holder
                && !self.vehicles[holder].held.contains(&zone)
            {
                return Err(format!("zone {zone} held by {holder}, which does not know"));
            }
            if let Some(&(waiting, _)) = state
                .queue
                .iter()
                .find(|&&(waiting, _)| self.vehicles[waiting].waiting != Some(zone))
            {
                return Err(format!("vehicle {waiting} queued for zone {zone} unawares"));
            }
        }
        // Roaming vehicles are idle and listed once, in the bay they roam.
        let mut listed = vec![0; self.vehicles.len()];
        for (bay, state) in self.bays.iter().enumerate() {
            for &id in &state.roaming {
                listed[id] += 1;
                let vehicle = &self.vehicles[id];
                if vehicle.task != Task::Idle || vehicle.bay != bay || listed[id] > 1 {
                    return Err(format!("vehicle {id} listed roaming bay {bay}"));
                }
            }
        }
        for (id, vehicle) in self.vehicles.iter().enumerate() {
            let (s, speed, stop) = state(id);
            if !(-SLACK..=vehicle.top + SLACK).contains(&speed) {
                return Err(format!("vehicle {id} at {speed} mm/ms"));
            }
            let (rail, start) = vehicle.path[0];
            if s - start < -SLACK || s - start > self.track.rails[rail].length + SLACK {
                return Err(format!("vehicle {id} {} mm into rail {rail}", s - start));
            }
            if !self.on_rail[rail].contains(&id) {
                return Err(format!("vehicle {id} not listed on rail {rail}"));
            }
            if let Some(zone) = self.track.rails[rail].zone
                && self.zones[zone].holder != Some(id)
            {
                return Err(format!(
                    "vehicle {id} inside zone {zone}, held by {:?}",
                    self.zones[zone].holder
                ));
            }
            for &zone in &vehicle.held {
                if self.zones[zone].holder != Some(id) {
                    return Err(format!("vehicle {id} holds zone {zone} unawares"));
                }
            }
            // Vehicles ahead: (vehicle, its front rail's start on this one's odometer, on the
            // path); off it, those past a node of it (a diverging one, or its end).
            let mut ahead: Vec<(usize, f64, bool)> = Vec::new();
            for (index, &(rail, start)) in vehicle.path.iter().enumerate() {
                if start > stop + self.reach {
                    break;
                }
                ahead.extend(self.on_rail[rail].iter().map(|&other| (other, start, true)));
                if index > 0 {
                    ahead.extend(
                        self.clearing[self.track.rails[rail].from]
                            .iter()
                            .filter(|&&other| self.vehicles[other].path[0].0 != rail)
                            .map(|&other| (other, start, false)),
                    );
                }
            }
            let &(last, start) = vehicle.path.back().expect("a rail");
            let end = start + self.track.rails[last].length;
            let node = self.track.rails[last].to;
            for &out in &self.track.nodes[node].out {
                ahead.extend(self.on_rail[out].iter().map(|&other| (other, end, false)));
            }
            ahead.extend(self.clearing[node].iter().map(|&other| (other, end, false)));
            for (other, start, on_path) in ahead {
                if other == id {
                    continue;
                }
                {
                    let leader = &self.vehicles[other];
                    let rail = leader.path[0].0;
                    let delta = start - leader.path[0].1;
                    let (other_s, _, other_stop) = state(other);
                    let tail = other_s + delta - leader.length;
                    if (on_path && other_s + delta < s) || (!on_path && tail >= start - EPS) {
                        // Behind it on its rail, or clear of the node.
                        continue;
                    }
                    let limit = other_stop + delta - leader.length - vehicle.gap;
                    if stop > limit + SLACK {
                        let describe = |id: usize| {
                            let vehicle = &self.vehicles[id];
                            format!(
                                "vehicle {id}: {:?} {:?} leader {:?} waiting {:?} held {:?} \
                                 path {:?} goal {} plan {:?} checks {:?}",
                                vehicle.task,
                                vehicle.mode,
                                vehicle.leader,
                                vehicle.waiting,
                                vehicle.held,
                                vehicle.path.iter().take(5).collect::<Vec<_>>(),
                                vehicle.goal,
                                vehicle.plan,
                                vehicle.checks
                            )
                        };
                        return Err(format!(
                            "at {now} ms vehicle {id} can stop at {stop:.3}, {:.3} mm past \
                             vehicle {other} less its length and the gap (on its path: \
                             {on_path}, rail {rail}, delta {delta})\n{}\n{}",
                            stop - limit,
                            describe(id),
                            describe(other)
                        ));
                    }
                }
            }
        }
        Ok(())
    }
}
