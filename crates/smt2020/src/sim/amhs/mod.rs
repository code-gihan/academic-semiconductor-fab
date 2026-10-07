//! AMHS of a dataset with a layout (SMAT2022): OHTs on the rail network carry FOUPs between
//! ports. Event-driven without time steps: every vehicle moves along a closed-form plan of
//! constant accelerations and has one pending event, at the next instant it must act (passing a
//! node, deciding at a braking point, arriving, ending a hoist). `control` keeps vehicles apart
//! and zones exclusive, `jobs` assigns transports and keeps idle vehicles roaming their bays.

#[cfg(test)]
mod check;
mod control;
mod jobs;
mod motion;
mod stats;
mod track;

use std::collections::VecDeque;
use std::sync::Arc;

use des_core::Time;
use serde::{Deserialize, Serialize};

use super::Error;
use super::replay::{ActivityChange, Kinematics, TrackWriter};
use crate::data::Dataset;
use crate::layout::{BayId, Layout, LinkId, PortId, ZoneId};
use motion::{Dynamics, Plan};

pub(crate) use motion::advance;
use track::Track;

pub use stats::{AmhsReport, BAY_DISTANCE_CLASSES, BayDistance, Moves, VehicleTimes};

const DEFAULT_HOIST: Time = 10_000;
const DEFAULT_ROAM_LIMIT: u32 = 15;

/// AMHS settings of a run on a dataset with a layout.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AmhsConfig {
    /// Vehicles in service: the layout's first ones; all if none.
    #[serde(default)]
    pub vehicles: Option<u32>,
    #[serde(default)]
    pub dispatch: Dispatch,
    /// Lots (batches) a tool may have assigned beyond the jobs it can run at once, on their way
    /// to its ports or waiting there. None as the SMAT2022 simulator: a single-lot tool as many as
    /// its ports hold, a batch tool none.
    #[serde(default)]
    pub look_ahead: Option<u32>,
    /// Time to load or unload a FOUP at a port (ms).
    #[serde(
        default = "default_hoist",
        deserialize_with = "super::deserialize_time"
    )]
    pub hoist: Time,
    /// Idle vehicles roaming an intrabay before the longest roaming one moves on to a neighbor.
    #[serde(default = "default_roam_limit")]
    pub roam_limit: u32,
    /// Vehicle type overrides: top speed (mm/s), acceleration and deceleration (mm/s²).
    #[serde(default)]
    pub max_speed: Option<f64>,
    #[serde(default)]
    pub acceleration: Option<f64>,
    #[serde(default)]
    pub deceleration: Option<f64>,
    /// Speed limit overrides of the straight and the curved rails (mm/s).
    #[serde(default)]
    pub straight_speed: Option<f64>,
    #[serde(default)]
    pub curve_speed: Option<f64>,
}

impl Default for AmhsConfig {
    fn default() -> Self {
        Self {
            vehicles: None,
            dispatch: Dispatch::Nearest,
            look_ahead: None,
            hoist: DEFAULT_HOIST,
            roam_limit: DEFAULT_ROAM_LIMIT,
            max_speed: None,
            acceleration: None,
            deceleration: None,
            straight_speed: None,
            curve_speed: None,
        }
    }
}

fn default_hoist() -> Time {
    DEFAULT_HOIST
}

fn default_roam_limit() -> u32 {
    DEFAULT_ROAM_LIMIT
}

/// Which idle vehicle takes a transport.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Dispatch {
    /// The idle vehicle nearest to the pickup along the rails.
    #[default]
    Nearest,
    /// The SMAT2022 simulator's: the longest roaming vehicle of the pickup's bay, else of its
    /// neighbors breadth first; one too close to stop before the pickup is passed over.
    Bay,
}

named_enum! {
    /// What a vehicle does.
    pub enum Activity {
        /// Roaming its bay, free for a transport.
        Idle = "idle",
        ToPickup = "to_pickup",
        Loading = "loading",
        ToDropoff = "to_dropoff",
        Unloading = "unloading",
    }
}

/// A vehicle now.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct VehicleStatus {
    pub id: usize,
    pub activity: Activity,
    /// Rail and position of its front (mm), its point and heading (rad), speed (m/s).
    pub link: usize,
    pub offset: f64,
    pub x: f64,
    pub y: f64,
    pub heading: f64,
    pub speed: f64,
    /// The lot it carries ([`LotStatus::id`](super::LotStatus::id)).
    pub lot: Option<u64>,
}

/// What a vehicle event did to a lot, for the fab.
pub(crate) enum Notice {
    /// The FOUP of `lot` left `port` on a vehicle.
    PickedUp {
        job: usize,
        lot: usize,
        port: PortId,
    },
    /// The FOUP of `lot` is down at `port`.
    Delivered {
        job: usize,
        lot: usize,
        port: PortId,
    },
}

/// What a vehicle stops for at the end of its path.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Goal {
    /// A port: it stops there.
    Port,
    /// A roaming point: it carries on to the next before stopping.
    Waypoint,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Task {
    Idle,
    ToPickup(usize),
    Loading(usize),
    ToDropoff(usize),
    Unloading(usize),
}

impl Task {
    fn activity(self) -> Activity {
        match self {
            Task::Idle => Activity::Idle,
            Task::ToPickup(_) => Activity::ToPickup,
            Task::Loading(_) => Activity::Loading,
            Task::ToDropoff(_) => Activity::ToDropoff,
            Task::Unloading(_) => Activity::Unloading,
        }
    }
}

/// How a vehicle follows the one ahead.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Mode {
    /// On its own plan, braking for the stop point of the vehicle ahead if it binds.
    Free,
    /// Copying the motion of the vehicle ahead at a fixed distance (`speed` lower by a fixed
    /// deficit).
    Mirror,
}

/// The vehicle ahead: its front at its odometer plus `delta` on the follower's odometer.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Leader {
    id: usize,
    delta: f64,
}

/// A decision due at a braking point of the plan.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Decision {
    /// Ask for the zone of the stop node ahead.
    Request(ZoneId),
    /// Carry on to the next roaming point.
    Extend,
    /// Close up to the vehicle ahead.
    Catch,
    /// Plan again (mirroring ends, the vehicle ahead settles or leaves, a speed limit binds).
    Recheck,
}

/// The instants a plan needs its vehicle (ms; `Time::MAX` for none).
#[derive(Clone, Copy, Debug)]
struct Checks {
    /// The front passes the end of its rail: exact and the event instant.
    node: f64,
    node_at: Time,
    /// A decision at a braking point (event at or before it).
    decision: Option<(Time, Decision)>,
    /// The vehicle ahead clears the follower's way.
    leave_at: Time,
    /// Rest at the goal port.
    arrive_at: Time,
    /// A hoist ends.
    hoist_at: Time,
}

impl Checks {
    const NONE: Self = Self {
        node: f64::INFINITY,
        node_at: Time::MAX,
        decision: None,
        leave_at: Time::MAX,
        arrive_at: Time::MAX,
        hoist_at: Time::MAX,
    };

    fn next(&self) -> Time {
        self.node_at
            .min(self.decision.map_or(Time::MAX, |(at, _)| at))
            .min(self.leave_at)
            .min(self.arrive_at)
            .min(self.hoist_at)
    }
}

struct Vehicle {
    length: f64,
    gap: f64,
    /// Top speed (mm/ms).
    top: f64,
    dynamics: Dynamics,
    plan: Plan,
    /// Rails from the one the front is on, with the odometer at each one's start.
    path: VecDeque<(LinkId, f64)>,
    /// Odometer of the end of the path and what is there.
    goal: f64,
    goal_kind: Goal,
    mode: Mode,
    leader: Option<Leader>,
    followers: Vec<usize>,
    /// Zones granted, and the zone asked for and waited on.
    held: Vec<ZoneId>,
    waiting: Option<ZoneId>,
    checks: Checks,
    /// The instant of its pending event, which carries `epoch` (`Time::MAX`: none).
    scheduled: Time,
    epoch: u32,
    /// Events handled at the current instant (a guard against control loops).
    burst: (Time, u32),
    task: Task,
    /// Bay it roams while idle.
    bay: BayId,
    /// Roaming point it heads for (0 or 1).
    point: usize,
    /// Accounting: activity and odometer since, blocked time accounted up to.
    since: f64,
    odometer: f64,
    blocked_from: f64,
}

/// A zone's holder and the vehicles waiting for it, with their request times.
#[derive(Default)]
struct Zone {
    holder: Option<usize>,
    queue: VecDeque<(usize, f64)>,
}

/// A transport of a lot from one port to another.
struct Job {
    lot: usize,
    from: PortId,
    to: PortId,
    vehicle: Option<usize>,
    created: f64,
    assigned: f64,
    /// Arrival at the pickup, pickup done, arrival at the drop-off.
    reached: f64,
    loaded: f64,
    arrived: f64,
    /// Rails of the loaded drive.
    trail: Vec<LinkId>,
}

/// An intrabay's roaming: its two points and the vehicles roaming it, longest first.
struct Bay {
    points: Option<[(LinkId, f64); 2]>,
    roaming: VecDeque<usize>,
}

pub(crate) struct Amhs {
    data: Arc<Dataset>,
    track: Track,
    hoist: Time,
    dispatch: Dispatch,
    roam_limit: u32,
    vehicles: Vec<Vehicle>,
    /// Per rail: the vehicles whose front is on it, frontmost first.
    on_rail: Vec<VecDeque<usize>>,
    /// Per diverging node: vehicles that passed it whose tail may still cover it.
    clearing: Vec<Vec<usize>>,
    zones: Vec<Zone>,
    jobs: Vec<Option<Job>>,
    free_jobs: Vec<usize>,
    /// Transports without a vehicle, oldest first.
    waiting: VecDeque<usize>,
    bays: Vec<Bay>,
    /// Longest vehicle with its gap.
    reach: f64,
    /// Every rail has the same speed limit.
    uniform: bool,
    /// Vehicles to plan again at the current instant.
    pending: VecDeque<usize>,
    notices: Vec<Notice>,
    /// The window logged for a replay.
    log: Option<MotionLog>,
    stats: stats::Stats,
    failure: Option<String>,
}

impl Amhs {
    /// The AMHS of `data`'s layout under `config`, at time 0 before any vehicle moves.
    pub(crate) fn new(data: Arc<Dataset>, config: &AmhsConfig) -> Result<Self, Error> {
        let layout = data
            .layout
            .as_ref()
            .ok_or_else(|| Error("the dataset has no AMHS layout".into()))?;
        let positive = |value: Option<f64>, what: &str| match value {
            Some(value) if !(value.is_finite() && value > 0.0) => {
                Err(Error(format!("the AMHS {what} must be positive")))
            }
            _ => Ok(value),
        };
        let max_speed = positive(config.max_speed, "max_speed")?;
        let acceleration = positive(config.acceleration, "acceleration")?;
        let deceleration = positive(config.deceleration, "deceleration")?;
        let straight = positive(config.straight_speed, "straight_speed")?;
        let curve = positive(config.curve_speed, "curve_speed")?;
        if config.hoist < 0 {
            return Err(Error("the AMHS hoist time must not be negative".into()));
        }
        if config.roam_limit == 0 {
            return Err(Error("the AMHS roam_limit must be positive".into()));
        }
        let count = match config.vehicles {
            None => layout.vehicles.len(),
            Some(count) if count >= 1 && count as usize <= layout.vehicles.len() => count as usize,
            Some(count) => {
                return Err(Error(format!(
                    "the AMHS needs 1 to {} vehicles, not {count}",
                    layout.vehicles.len()
                )));
            }
        };
        let track = Track::new(layout, |link| {
            let rail = &layout.links[link];
            let limit = match rail.arc {
                None => straight,
                Some(_) => curve,
            };
            limit.unwrap_or(rail.max_speed) / 1_000.0
        });
        let mut vehicles = Vec::with_capacity(count);
        let mut reach: f64 = 0.0;
        for spec in &layout.vehicles[..count] {
            let kind = &layout.vehicle_types[spec.kind];
            let dynamics = Dynamics {
                accel: acceleration.unwrap_or(kind.acceleration) / 1e6,
                decel: deceleration.unwrap_or(kind.deceleration) / 1e6,
            };
            // Followers close up in closed form only if they brake at least as hard as they
            // speed up.
            if dynamics.accel > dynamics.decel {
                return Err(Error(format!(
                    "vehicle type {}: the AMHS needs the deceleration at least the acceleration",
                    kind.name
                )));
            }
            reach = reach.max(kind.length + kind.min_gap);
            let rest = Plan::rest(0.0, spec.offset);
            let mut path = VecDeque::new();
            path.push_back((spec.link, 0.0));
            vehicles.push(Vehicle {
                length: kind.length,
                gap: kind.min_gap,
                top: max_speed.unwrap_or(kind.max_speed) / 1_000.0,
                dynamics,
                plan: rest,
                path,
                goal: spec.offset,
                goal_kind: Goal::Port,
                mode: Mode::Free,
                leader: None,
                followers: Vec::new(),
                held: Vec::new(),
                waiting: None,
                checks: Checks::NONE,
                scheduled: Time::MAX,
                epoch: 0,
                burst: (0, 0),
                task: Task::Idle,
                bay: layout.links[spec.link].bay,
                point: 0,
                since: 0.0,
                odometer: spec.offset,
                blocked_from: f64::INFINITY,
            });
        }
        if let Some(short) = layout.links.iter().find(|link| link.length < reach) {
            return Err(Error(format!(
                "rail {} is shorter than a vehicle with its gap",
                short.name
            )));
        }
        let mut on_rail = vec![VecDeque::new(); layout.links.len()];
        for (id, vehicle) in vehicles.iter().enumerate() {
            on_rail[vehicle.path[0].0].push_back(id);
        }
        for list in &mut on_rail {
            // Frontmost first.
            list.make_contiguous()
                .sort_by(|&a, &b| vehicles[b].plan.end().total_cmp(&vehicles[a].plan.end()));
        }
        let mut zones: Vec<Zone> = layout.zones.iter().map(|_| Zone::default()).collect();
        for (id, vehicle) in vehicles.iter_mut().enumerate() {
            if let Some(zone) = layout.links[vehicle.path[0].0].zone {
                zones[zone].holder = Some(id);
                vehicle.held.push(zone);
            }
        }
        let uniform = track
            .rails
            .windows(2)
            .all(|pair| pair[0].limit == pair[1].limit);
        let bays = layout
            .bays
            .iter()
            .enumerate()
            .map(|(bay, spec)| Bay {
                points: (!spec.interbay)
                    .then(|| roaming_points(layout, bay))
                    .flatten(),
                roaming: VecDeque::new(),
            })
            .collect();
        Ok(Self {
            track,
            hoist: config.hoist,
            dispatch: config.dispatch,
            roam_limit: config.roam_limit,
            on_rail,
            clearing: vec![Vec::new(); layout.nodes.len()],
            zones,
            jobs: Vec::new(),
            free_jobs: Vec::new(),
            waiting: VecDeque::new(),
            bays,
            reach,
            uniform,
            pending: VecDeque::new(),
            notices: Vec::new(),
            log: None,
            stats: stats::Stats::new(count as u32),
            failure: None,
            vehicles,
            data,
        })
    }

    fn layout(&self) -> &Layout {
        self.data.layout.as_ref().expect("AMHS layout")
    }

    /// Port position: rail and offset.
    fn port(&self, port: PortId) -> (LinkId, f64) {
        let port = &self.layout().ports[port];
        (port.link, port.offset)
    }

    /// The first failure (a deadlock or a control loop), which stops the run.
    pub(crate) fn failure(&self) -> Option<&str> {
        self.failure.as_deref()
    }

    /// The notices of the events handled since the last call.
    pub(crate) fn take_notices(&mut self, into: &mut Vec<Notice>) {
        into.append(&mut self.notices);
    }

    /// Every vehicle at `now`, by id; `lot_id` names a carried lot.
    pub(crate) fn statuses(&self, now: Time, lot_id: impl Fn(usize) -> u64) -> Vec<VehicleStatus> {
        let now = now as f64;
        self.vehicles
            .iter()
            .enumerate()
            .map(|(id, vehicle)| {
                let (s, speed) = vehicle.plan.state(now.max(vehicle.plan.start()));
                let (link, start) = vehicle.path[0];
                let offset = (s - start).clamp(0.0, self.track.rails[link].length);
                let (x, y, heading) = self.layout().point(link, offset);
                let lot = match vehicle.task {
                    Task::ToDropoff(job) | Task::Unloading(job) => {
                        Some(lot_id(self.jobs[job].as_ref().expect("job").lot))
                    }
                    _ => None,
                };
                VehicleStatus {
                    id,
                    activity: vehicle.task.activity(),
                    link,
                    offset,
                    x,
                    y,
                    heading,
                    speed,
                    lot,
                }
            })
            .collect()
    }

    /// The window's measures up to `now`.
    pub(crate) fn report(&mut self, now: Time) -> AmhsReport {
        let now = now as f64;
        for id in 0..self.vehicles.len() {
            self.account(id, now);
        }
        self.stats.report.clone()
    }

    /// Starts a new window at `now`; the vehicles must be accounted up to it.
    pub(crate) fn reset(&mut self) {
        self.stats.reset();
    }

    /// Adds a vehicle's activity time, distance and blocked time up to `now` to the window.
    fn account(&mut self, id: usize, now: f64) {
        let odometer = pending(&self.vehicles[id], now, &mut self.stats.report);
        let vehicle = &mut self.vehicles[id];
        vehicle.blocked_from = vehicle.blocked_from.max(now);
        vehicle.since = now;
        vehicle.odometer = odometer;
    }

    /// The window's measures up to `now`, the accounting left as it is.
    pub(crate) fn window(&self, now: Time) -> AmhsReport {
        let mut report = self.stats.report.clone();
        for vehicle in &self.vehicles {
            pending(vehicle, now as f64, &mut report);
        }
        report
    }
}

/// Adds a vehicle's activity time, distance and blocked time since its last accounting up to
/// `now` to `report`; returns its odometer at `now`.
fn pending(vehicle: &Vehicle, now: f64, report: &mut AmhsReport) -> f64 {
    let (odometer, _) = vehicle.plan.state(now.max(vehicle.plan.start()));
    let time = (now - vehicle.since).round() as Time;
    let times = &mut report.vehicle_time;
    *match vehicle.task {
        Task::Idle => &mut times.idle,
        Task::ToPickup(_) => &mut times.to_pickup,
        Task::Loading(_) => &mut times.loading,
        Task::ToDropoff(_) => &mut times.to_dropoff,
        Task::Unloading(_) => &mut times.unloading,
    } += time;
    let distance = (odometer - vehicle.odometer) / 1_000.0;
    match vehicle.task {
        Task::ToDropoff(_) | Task::Unloading(_) => report.loaded_distance += distance,
        _ => report.empty_distance += distance,
    }
    if vehicle.blocked_from < now {
        report.blocked += (now - vehicle.blocked_from).round() as Time;
    }
    odometer
}

/// What the AMHS logs of a window for its replay ([`Replay`](super::replay::Replay)): each
/// vehicle's motion and the ways it takes at diverging nodes, and the vehicles' activities.
struct MotionLog {
    from: Time,
    until: Time,
    /// The window began: the vehicles' states at `from` are in.
    started: bool,
    /// Per vehicle: the time its motion is logged up to, and what turns its odometer into the
    /// log's, which counts from the start of the rail it was on at `from` (rebases leave it
    /// running on).
    logged: Vec<f64>,
    base: Vec<f64>,
    kinematics: Kinematics,
    tracks: Vec<TrackWriter>,
    activities: Vec<ActivityChange>,
}

impl MotionLog {
    /// From `t` on vehicle `id` drives from `s` on its odometer at `v` with acceleration `a`.
    fn push(&mut self, id: usize, t: f64, s: f64, v: f64, a: f64) {
        self.tracks[id].breakpoint(&self.kinematics, t, s + self.base[id], v, a);
    }

    /// Changes at `now` belong to the window.
    fn open(&self, now: f64) -> bool {
        self.started && now < self.until as f64
    }
}

impl Amhs {
    /// Logs the window [`from`, `until`) for a replay.
    pub(crate) fn record(&mut self, from: Time, until: Time) {
        let count = self.vehicles.len();
        // The vehicles' accelerations, and the speeds they come to: rest and their limits.
        let mut kinematics = Kinematics {
            accelerations: vec![0.0],
            speeds: vec![0.0],
        };
        let add = |list: &mut Vec<f64>, value: f64| {
            if !list.iter().any(|known| known.to_bits() == value.to_bits()) {
                list.push(value);
            }
        };
        for vehicle in &self.vehicles {
            add(&mut kinematics.accelerations, vehicle.dynamics.accel);
            add(&mut kinematics.accelerations, -vehicle.dynamics.decel);
            for rail in &self.track.rails {
                add(&mut kinematics.speeds, rail.limit.min(vehicle.top));
            }
        }
        self.log = Some(MotionLog {
            from,
            until,
            started: false,
            logged: vec![from as f64; count],
            base: vec![0.0; count],
            kinematics,
            tracks: Vec::with_capacity(count),
            activities: Vec::new(),
        });
    }

    /// Whether the logged window has yet to begin by `now`.
    pub(crate) fn log_due(&self, now: Time) -> bool {
        self.log
            .as_ref()
            .is_some_and(|log| !log.started && now >= log.from)
    }

    /// The window begins (before the first event at or after its start): every vehicle's motion,
    /// rail and activity then.
    pub(crate) fn begin_log(&mut self) {
        let Some(log) = &mut self.log else {
            return;
        };
        let from = log.from as f64;
        for (id, vehicle) in self.vehicles.iter().enumerate() {
            let (s, v) = vehicle.plan.state(from);
            let (rail, start) = vehicle.path[0];
            log.base[id] = -start;
            let a = vehicle.plan.acceleration(from);
            let track = TrackWriter::new(&log.kinematics, from, s - start, v, a, rail);
            log.tracks.push(track);
            log.activities.push(ActivityChange {
                time: log.from,
                vehicle: id,
                activity: vehicle.task.activity(),
            });
        }
        log.started = true;
    }

    /// The vehicle drives `plan` from `now` on: the phases of its old plan it drove since the log
    /// reached it, then the new plan's state, into the log.
    fn log_motion(&mut self, id: usize, now: f64, plan: &Plan) {
        let Some(log) = &mut self.log else {
            return;
        };
        if !log.open(now) {
            return;
        }
        for phase in &self.vehicles[id].plan.phases {
            if phase.t > log.logged[id] && phase.t < now {
                log.push(id, phase.t, phase.s, phase.v, phase.a);
            }
        }
        let (s, v) = plan.state(now);
        log.push(id, now, s, v, plan.acceleration(now));
        log.logged[id] = now;
    }

    /// The vehicle entered the rail its path starts with: the way it took, if it had a choice.
    fn log_rail(&mut self, id: usize, now: Time) {
        if let Some(log) = &mut self.log
            && log.open(now as f64)
        {
            let (rail, _) = self.vehicles[id].path[0];
            let node = &self.track.nodes[self.track.rails[rail].from];
            if node.diverging {
                let way = node.out.iter().position(|&out| out == rail);
                log.tracks[id].choice(way.expect("a way out"));
            }
        }
    }

    fn log_task(&mut self, id: usize, now: f64) {
        if let Some(log) = &mut self.log
            && log.open(now)
        {
            log.activities.push(ActivityChange {
                time: now.round() as Time,
                vehicle: id,
                activity: self.vehicles[id].task.activity(),
            });
        }
    }

    /// The window's tracks up to `until`, each vehicle's plan driven since it last changed added,
    /// with the kinematics they are in and the activities.
    #[allow(clippy::type_complexity)]
    pub(crate) fn replay(
        &self,
        until: Time,
    ) -> Option<(Vec<TrackWriter>, &Kinematics, &[ActivityChange])> {
        let log = self.log.as_ref().filter(|log| log.started)?;
        let mut tracks = log.tracks.clone();
        let until = until as f64;
        for (id, vehicle) in self.vehicles.iter().enumerate() {
            let track = &mut tracks[id];
            for phase in &vehicle.plan.phases {
                if phase.t > log.logged[id] && phase.t < until {
                    let s = phase.s + log.base[id];
                    track.breakpoint(&log.kinematics, phase.t, s, phase.v, phase.a);
                }
            }
            track.finish(&log.kinematics, until);
        }
        Some((tracks, &log.kinematics, &log.activities))
    }
}

/// An intrabay's two roaming points (the SMAT2022 simulator's): 100 mm before the end of its
/// topmost and bottommost rails into a diverging node; its first and last rails without such.
fn roaming_points(layout: &Layout, bay: BayId) -> Option<[(LinkId, f64); 2]> {
    let rails: Vec<LinkId> = (0..layout.links.len())
        .filter(|&link| layout.links[link].bay == bay)
        .collect();
    let (&first, &last) = (rails.first()?, rails.last()?);
    let mut out = vec![0usize; layout.nodes.len()];
    for link in &layout.links {
        out[link.from] += 1;
    }
    let (mut top, mut bottom): (Option<LinkId>, Option<LinkId>) = (None, None);
    for &link in &rails {
        let to = &layout.nodes[layout.links[link].to];
        if out[layout.links[link].to] < 2 {
            continue;
        }
        if top.is_none_or(|top| to.y > layout.nodes[layout.links[top].to].y) {
            top = Some(link);
        }
        if bottom.is_none_or(|bottom| to.y < layout.nodes[layout.links[bottom].to].y) {
            bottom = Some(link);
        }
    }
    let (top, bottom) = match (top, bottom) {
        (Some(top), Some(bottom)) if top != bottom => (top, bottom),
        _ => (first, last),
    };
    let at = |link: LinkId| (link, (layout.links[link].length - 100.0).max(0.0));
    Some([at(top), at(bottom)])
}
