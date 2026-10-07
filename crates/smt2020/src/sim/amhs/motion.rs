//! Vehicle motion in closed form: plans of constant-acceleration phases, the time-optimal profile
//! to a stop under speed limits, and the instants a plan reaches a position, a stop point or a
//! speed. Times in ms, positions in mm on the vehicle's odometer, speeds in mm/ms (= m/s),
//! accelerations in mm/ms².

/// Phase of constant acceleration from `t`: position `s`, speed `v`, acceleration `a`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Phase {
    pub t: f64,
    pub s: f64,
    pub v: f64,
    pub a: f64,
}

impl Phase {
    fn at(&self, t: f64) -> (f64, f64) {
        advance(self.s, self.v, self.a, t - self.t)
    }
}

/// Position and speed `tau` ms on from position `s` at speed `v` with acceleration `a`: a phase's
/// motion, the same for the plans and their replay.
pub(crate) fn advance(s: f64, v: f64, a: f64, tau: f64) -> (f64, f64) {
    (s + v * tau + 0.5 * a * tau * tau, (v + a * tau).max(0.0))
}

/// Motion from the first phase's time on: phases in time order, the last one at rest for good.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Plan {
    pub phases: Vec<Phase>,
}

/// Positions within this of each other are the same (mm): well above the rounding of positions
/// and of times late in a long run (2 years ≈ 6.3·10¹⁰ ms, rounded to 8·10⁻⁶ ms).
pub(super) const EPS: f64 = 1e-3;

/// Speeds within this of each other are the same (mm/ms).
pub(super) const SPEED_EPS: f64 = 1e-6;

/// Instants within this of each other are the same (ms): a vehicle moves less than [`EPS`]
/// meanwhile.
const TIME_EPS: f64 = 1e-4;

impl Plan {
    /// At rest at `s` from `t`.
    pub(super) fn rest(t: f64, s: f64) -> Self {
        Self {
            phases: vec![Phase {
                t,
                s,
                v: 0.0,
                a: 0.0,
            }],
        }
    }

    pub(super) fn start(&self) -> f64 {
        self.phases[0].t
    }

    /// When the vehicle comes to rest for good.
    pub(super) fn end_time(&self) -> f64 {
        self.phases.last().expect("phases").t
    }

    /// Where the vehicle comes to rest.
    pub(super) fn end(&self) -> f64 {
        self.phases.last().expect("phases").s
    }

    fn phase_at(&self, t: f64) -> usize {
        self.phases
            .iter()
            .rposition(|phase| phase.t <= t)
            .unwrap_or(0)
    }

    /// Position and speed at `t`, not before the plan's start.
    pub(super) fn state(&self, t: f64) -> (f64, f64) {
        let index = self.phase_at(t);
        let (s, v) = self.phases[index].at(t);
        if index + 1 == self.phases.len() {
            // At rest for good.
            (self.end(), 0.0)
        } else {
            (s, v)
        }
    }

    /// Acceleration at `t`, not before the plan's start; 0 at rest for good.
    pub(super) fn acceleration(&self, t: f64) -> f64 {
        let index = self.phase_at(t);
        if index + 1 == self.phases.len() {
            0.0
        } else {
            self.phases[index].a
        }
    }

    /// End of phase `index`.
    fn phase_end(&self, index: usize) -> f64 {
        self.phases
            .get(index + 1)
            .map_or(f64::INFINITY, |next| next.t)
    }

    /// First time from `from` at which the vehicle reaches position `target`.
    pub(super) fn time_at(&self, target: f64, from: f64) -> Option<f64> {
        let first = self.phase_at(from);
        for index in first..self.phases.len() {
            let phase = &self.phases[index];
            let start = phase.t.max(from);
            let (s, v) = phase.at(start);
            if s >= target - EPS {
                return Some(start);
            }
            let end = self.phase_end(index);
            if index + 1 == self.phases.len() {
                return None;
            }
            if self.phases[index + 1].s < target - EPS {
                continue;
            }
            let left = target - s;
            let root = (v * v + 2.0 * phase.a * left).max(0.0).sqrt();
            let tau = if v + root > 0.0 {
                2.0 * left / (v + root)
            } else {
                0.0
            };
            return Some((start + tau).min(end));
        }
        None
    }

    /// First time from `from` at which the stop point s + v²/(2·`decel`) reaches `target`: where
    /// braking at `decel` must begin to stop there. Stop points never fall back while braking is
    /// at most `decel`.
    pub(super) fn time_stop_point_at(&self, target: f64, decel: f64, from: f64) -> Option<f64> {
        let first = self.phase_at(from);
        for index in first..self.phases.len() {
            let phase = &self.phases[index];
            let start = phase.t.max(from);
            let (s, v) = phase.at(start);
            let c = s + v * v / (2.0 * decel) - target;
            if c >= -EPS {
                return Some(start);
            }
            if index + 1 == self.phases.len() {
                return None;
            }
            let end = self.phase_end(index);
            let (s_end, v_end) = phase.at(end);
            if s_end + v_end * v_end / (2.0 * decel) < target - EPS {
                continue;
            }
            let a = phase.a;
            let quadratic = 0.5 * a + a * a / (2.0 * decel);
            let linear = v * (1.0 + a / decel);
            let tau = if quadratic.abs() < 1e-18 {
                if linear > 0.0 { -c / linear } else { 0.0 }
            } else {
                let root = (linear * linear - 4.0 * quadratic * c).max(0.0).sqrt();
                if linear + root > 0.0 {
                    -2.0 * c / (linear + root)
                } else {
                    0.0
                }
            };
            return Some((start + tau).min(end));
        }
        None
    }

    /// Whether this plan and `other` move the same from `from` on, up to rounding: the same state
    /// then and the same phases after.
    pub(super) fn same_motion(&self, other: &Plan, from: f64) -> bool {
        let (first, other_first) = (self.phase_at(from), other.phase_at(from));
        let (rest, other_rest) = (&self.phases[first..], &other.phases[other_first..]);
        let ((s, v), (other_s, other_v)) = (self.state(from), other.state(from));
        rest.len() == other_rest.len()
            && (s - other_s).abs() <= EPS
            && (v - other_v).abs() <= SPEED_EPS
            && rest[0].a == other_rest[0].a
            && rest[1..]
                .iter()
                .zip(&other_rest[1..])
                .all(|(phase, other)| {
                    phase.a == other.a
                        && (phase.t - other.t).abs() <= TIME_EPS
                        && (phase.s - other.s).abs() <= EPS
                        && (phase.v - other.v).abs() <= SPEED_EPS
                })
    }

    /// Stop point at `t` under braking at `decel`.
    pub(super) fn stop_point(&self, t: f64, decel: f64) -> f64 {
        let (s, v) = self.state(t);
        s + v * v / (2.0 * decel)
    }

    /// Whether the stop point stays where it is from `t` on: the vehicle is at rest or braking at
    /// `decel` to its rest.
    pub(super) fn settled(&self, t: f64, decel: f64) -> bool {
        let first = self.phase_at(t);
        self.phases[first..self.phases.len() - 1]
            .iter()
            .all(|phase| phase.a <= -decel * (1.0 - 1e-9))
            || first + 1 == self.phases.len()
    }

    /// When the last braking to rest begins: the stop point is settled from then on.
    pub(super) fn settling_time(&self, decel: f64) -> f64 {
        let last = self.phases.len() - 1;
        let mut index = last;
        while index > 0 && self.phases[index - 1].a <= -decel * (1.0 - 1e-9) {
            index -= 1;
        }
        self.phases[index].t
    }

    /// The same motion from `from` on, shifted by `ds`.
    pub(super) fn shifted(&self, from: f64, ds: f64) -> Self {
        let first = self.phase_at(from);
        let mut phases = Vec::with_capacity(self.phases.len() - first);
        let (s, v) = self.state(from);
        phases.push(Phase {
            t: from,
            s: s + ds,
            v,
            a: if first + 1 == self.phases.len() {
                0.0
            } else {
                self.phases[first].a
            },
        });
        for phase in &self.phases[first + 1..] {
            phases.push(Phase {
                s: phase.s + ds,
                ..*phase
            });
        }
        Self { phases }
    }

    /// Braking at `decel` from (`t`, `s`, `v`) to rest.
    pub(super) fn braking(t: f64, s: f64, v: f64, decel: f64) -> Self {
        if v <= 0.0 {
            return Self::rest(t, s);
        }
        Self {
            phases: vec![
                Phase { t, s, v, a: -decel },
                Phase {
                    t: t + v / decel,
                    s: s + v * v / (2.0 * decel),
                    v: 0.0,
                    a: 0.0,
                },
            ],
        }
    }

    /// First time from `t0` at which this plan's speed is at least that of a vehicle braking at
    /// `decel` from `v0` at `t0`; none if that vehicle stops first.
    pub(super) fn time_matching(&self, t0: f64, v0: f64, decel: f64) -> Option<f64> {
        let stops = t0 + v0 / decel;
        let first = self.phase_at(t0);
        for index in first..self.phases.len() {
            let phase = &self.phases[index];
            let start = phase.t.max(t0);
            if start > stops {
                return None;
            }
            let (_, v) = phase.at(start);
            let gap = v - (v0 - decel * (start - t0));
            if gap >= -SPEED_EPS {
                return Some(start);
            }
            let slope = phase.a + decel;
            if index + 1 < self.phases.len() && slope > 0.0 {
                let at = start - gap / slope;
                if at <= self.phase_end(index) {
                    return (at <= stops).then_some(at);
                }
            } else if index + 1 == self.phases.len() {
                // At rest: never matched before stopping.
                return None;
            }
        }
        None
    }

    /// First time from `from` at which the speed exceeds `limit`.
    pub(super) fn time_above(&self, limit: f64, from: f64) -> Option<f64> {
        let first = self.phase_at(from);
        for index in first..self.phases.len() - 1 {
            let phase = &self.phases[index];
            let start = phase.t.max(from);
            let (_, v) = phase.at(start);
            if v > limit + SPEED_EPS {
                return Some(start);
            }
            if phase.a > 0.0 {
                let at = start + (limit - v) / phase.a;
                if at < self.phase_end(index) {
                    return Some(at);
                }
            }
        }
        None
    }
}

/// Acceleration and deceleration limits (mm/ms²).
#[derive(Clone, Copy, Debug)]
pub(super) struct Dynamics {
    pub accel: f64,
    pub decel: f64,
}

/// Time-optimal plan from (`t0`, `s0`, `v0`) to rest at the end of the last zone: `zones` are
/// (end position, speed limit) in order from `s0`. The speed stays within each zone's limit, the
/// acceleration within `dynamics`. If `v0` is too high to stop in time, the plan brakes at once.
pub(super) fn profile(t0: f64, s0: f64, v0: f64, zones: &[(f64, f64)], dynamics: Dynamics) -> Plan {
    let (a, d) = (dynamics.accel, dynamics.decel);
    let stop = zones.last().map_or(s0, |zone| zone.0);
    // Highest speed at each zone boundary from which every later limit and the stop are met.
    let mut feasible = vec![0.0; zones.len() + 1];
    for k in (1..zones.len()).rev() {
        let length = zones[k].0 - zones[k - 1].0;
        feasible[k] = zones[k]
            .1
            .min((feasible[k + 1] * feasible[k + 1] + 2.0 * d * length.max(0.0)).sqrt());
    }
    let first_length = zones.first().map_or(0.0, |zone| zone.0 - s0).max(0.0);
    let first_feasible = zones.first().map_or(0.0, |zone| zone.1).min(
        (feasible[1.min(zones.len())] * feasible[1.min(zones.len())] + 2.0 * d * first_length)
            .sqrt(),
    );
    if v0 > first_feasible + SPEED_EPS {
        // Too fast to meet the limits: brake now (a planning error upstream, kept safe here).
        debug_assert!(false, "v0 {v0} exceeds the feasible {first_feasible}");
        return Plan::braking(t0, s0, v0, d);
    }
    let mut phases = Vec::new();
    let (mut t, mut s, mut u) = (t0, s0, v0);
    for (k, &(end, limit)) in zones.iter().enumerate() {
        let length = (end - s).max(0.0);
        let exit = limit
            .min(feasible[k + 1])
            .min((u * u + 2.0 * a * length).sqrt());
        let peak = limit
            .min(((2.0 * a * d * length + d * u * u + a * exit * exit) / (a + d)).sqrt())
            .max(u.max(exit));
        let rise = (peak * peak - u * u) / (2.0 * a);
        let fall = (peak * peak - exit * exit) / (2.0 * d);
        let cruise = length - rise - fall;
        if peak > u + SPEED_EPS * 1e-3 {
            phases.push(Phase { t, s, v: u, a });
            t += (peak - u) / a;
            s += rise;
        }
        if cruise > EPS && peak > 0.0 {
            phases.push(Phase {
                t,
                s,
                v: peak,
                a: 0.0,
            });
            t += cruise / peak;
            s += cruise;
        }
        if peak > exit + SPEED_EPS * 1e-3 {
            phases.push(Phase {
                t,
                s,
                v: peak,
                a: -d,
            });
            t += (peak - exit) / d;
        }
        s = end;
        u = exit;
    }
    phases.push(Phase {
        t,
        s: stop,
        v: 0.0,
        a: 0.0,
    });
    // Neighbors of the same acceleration merge.
    let mut merged: Vec<Phase> = Vec::with_capacity(phases.len());
    for phase in phases {
        match merged.last() {
            Some(last) if last.a == phase.a && phase.v > 0.0 => {}
            _ => merged.push(phase),
        }
    }
    Plan { phases: merged }
}

/// Duration of the time-optimal run from rest at `s0` to rest at the end of `zones`.
pub(super) fn run_time(s0: f64, zones: &[(f64, f64)], dynamics: Dynamics) -> f64 {
    let plan = profile(0.0, s0, 0.0, zones, dynamics);
    plan.end_time()
}

#[cfg(test)]
mod tests {
    use super::*;

    const DYN: Dynamics = Dynamics {
        accel: 0.002,
        decel: 0.003,
    };

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-6
    }

    #[test]
    fn a_long_run_accelerates_cruises_and_brakes() {
        // 10 m at 1 m/s: accelerating 500 ms over 250 mm, braking 333.3 ms over 166.7 mm.
        let plan = profile(0.0, 0.0, 0.0, &[(10_000.0, 1.0)], DYN);
        assert_eq!(plan.phases.len(), 4);
        let accel = 1.0 / 0.002;
        let brake = 1.0 / 0.003;
        let cruise = (10_000.0 - 250.0 - 1.0 / 0.006) / 1.0;
        assert!(close(plan.end_time(), accel + cruise + brake));
        assert!(close(plan.end(), 10_000.0));
        assert!(close(plan.state(accel).1, 1.0));
        assert_eq!(plan.state(1e9), (10_000.0, 0.0));
        // Braking starts where the stop point reaches the end.
        let brake_at = plan.time_stop_point_at(10_000.0, 0.003, 0.0).unwrap();
        assert!(close(brake_at, accel + cruise));
        assert!(close(plan.settling_time(0.003), accel + cruise));
        assert!(plan.settled(brake_at + 1.0, 0.003) && !plan.settled(0.0, 0.003));
    }

    #[test]
    fn a_short_run_peaks_below_the_limit() {
        // 100 mm: v_peak² = 2·a·d·L/(a + d) = 0.24·… → peak 0.3464 m/s.
        let plan = profile(0.0, 0.0, 0.0, &[(100.0, 1.0)], DYN);
        let peak = (2.0 * 0.002 * 0.003 * 100.0 / 0.005_f64).sqrt();
        assert_eq!(plan.phases.len(), 3);
        assert!(close(plan.phases[1].v, peak));
        assert!(close(plan.end(), 100.0));
    }

    #[test]
    fn slower_zones_are_met_on_entry() {
        // 1.5 m/s, then 0.5 m/s from 3 m to 4 m, then 1.5 m/s to the stop at 8 m.
        let zones = [(3_000.0, 1.5), (4_000.0, 0.5), (8_000.0, 1.5)];
        let plan = profile(0.0, 0.0, 0.0, &zones, DYN);
        let entry = plan.time_at(3_000.0, 0.0).unwrap();
        let leave = plan.time_at(4_000.0, 0.0).unwrap();
        assert!(close(plan.state(entry).1, 0.5));
        assert!(close(plan.state(leave).1, 0.5));
        // Never above a limit: check on a grid.
        for step in 0..2_000 {
            let t = plan.end_time() * f64::from(step) / 2_000.0;
            let (s, v) = plan.state(t);
            let limit = if (3_000.0 - EPS..4_000.0 + EPS).contains(&s) {
                0.5
            } else {
                1.5
            };
            assert!(v <= limit + 1e-9, "{v} > {limit} at {s}");
        }
        assert!(close(plan.end(), 8_000.0));
    }

    #[test]
    fn a_moving_start_keeps_its_speed_into_the_plan() {
        let plan = profile(100.0, 50.0, 0.8, &[(5_000.0, 1.0)], DYN);
        assert_eq!(plan.state(100.0), (50.0, 0.8));
        assert!(close(plan.phases[1].v, 1.0));
        // A stop just within reach brakes at once.
        let reach = 0.8 * 0.8 / 0.006;
        let plan = profile(0.0, 0.0, 0.8, &[(reach, 1.0)], DYN);
        assert_eq!(plan.phases[0].a, -0.003);
        assert!(close(plan.end(), reach));
    }

    #[test]
    fn crossings_and_speeds_are_exact() {
        let plan = profile(0.0, 0.0, 0.0, &[(10_000.0, 1.0)], DYN);
        // 100 mm into the acceleration: t = sqrt(2·100/a).
        assert!(close(
            plan.time_at(100.0, 0.0).unwrap(),
            (200.0_f64 / 0.002).sqrt()
        ));
        assert!(close(plan.time_at(5_000.0, 0.0).unwrap(), 500.0 + 4_750.0));
        assert_eq!(plan.time_at(10_001.0, 0.0), None);
        // A vehicle at 1.5 m/s braking from 0 meets the plan's speed when 1.5 − 0.003·t = 0.002·t.
        assert!(close(plan.time_matching(0.0, 1.5, 0.003).unwrap(), 300.0));
        // Braking from 0.2 m/s stops before the plan gets that slow again… it is faster at once.
        assert_eq!(plan.time_matching(0.0, 0.0, 0.003), Some(0.0));
        let stopping = Plan::braking(0.0, 0.0, 0.3, 0.003);
        assert_eq!(stopping.time_matching(0.0, 0.6, 0.003), None);
        // A copy 909 mm behind, from 1 s on.
        let copy = plan.shifted(1_000.0, -909.0);
        assert_eq!(copy.state(2_000.0).0, plan.state(2_000.0).0 - 909.0);
        assert!(close(copy.end(), 10_000.0 - 909.0));
        // Braking from 3 s on (at 1 m/s) stops 166.7 mm later.
        let (s, v) = plan.state(3_000.0);
        let stopped = Plan::braking(3_000.0, s, v, 0.003);
        assert!(close(stopped.end(), s + 1.0 / 0.006));
    }
}
