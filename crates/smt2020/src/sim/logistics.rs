//! Lots on a layout (SMAT2022): FOUPs stand at ports and OHTs carry them. A lot whose step ends
//! joins its next step's queue at once, wherever its FOUP stands. Dispatching assigns lots to
//! tools ahead: a tool that is up takes lots while it has a free port (a batch tool, a batch once
//! idle) or as many as the look-ahead allows, the tools with the least work first, and the FOUP
//! heads for its port at once (from a tool port: tool to tool). A lot
//! no tool takes goes to the free track buffer nearest to it in the bays of its tool group, else in
//! the nearest ring of neighbor bays with one, and waits there; an assignment sends it on, also
//! while it is on its way. A tool starts the best-ranked of its assignments whose FOUPs are all at
//! its ports; a batch tool takes the FOUPs of its batches in through its ports and hands them out port by port
//! after the batch. A tool that goes down or falls due for a PM gives its assignments back to the
//! queue. Lots enter at the commit station nearest to the tools of their first step and leave
//! through the complete station nearest to their last; steps of tool groups without tools are left
//! out.

use std::collections::VecDeque;
use std::mem;
use std::sync::Arc;

use des_core::{Scheduler, Time};

use super::amhs::{Amhs, AmhsConfig, Notice};
use super::fab::{Event, Fab, LotId, LotState, ToolId, Waiting};
use super::record::{Entry, EventKind};
use super::replay::{FoupMove, FoupPlace, Recorded, Replay, ReplayWindow, Timeline};
use super::status::{AmhsStatus, FoupCount, FoupStatus};
use super::{Error, LotStatus};
use crate::data::{Dataset, StepSetup, ToolGroupId};
use crate::layout::{BayId, LinkId, PortId, PortKind};

/// A port's FOUP, and the lot it is kept for.
#[derive(Clone, Copy, Default)]
struct PortState {
    lot: Option<LotId>,
    reserved: Option<LotId>,
}

impl PortState {
    fn free(self) -> bool {
        self.lot.is_none() && self.reserved.is_none()
    }
}

/// Where a lot's FOUP is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum At {
    /// No lot in this slot.
    #[default]
    Nowhere,
    /// In a commit station, picked up at its port.
    Commit(PortId),
    /// On a tool port or a track buffer.
    Port(PortId),
    Vehicle,
    /// In a batch tool.
    Inside(ToolId),
}

/// A lot's FOUP: where it is, its running transport with the port it drops the FOUP at, the port
/// it is to end up at (kept for it unless a complete station's), and the tool it is assigned to.
#[derive(Clone, Copy, Default)]
struct Place {
    at: At,
    job: Option<usize>,
    dropoff: Option<PortId>,
    goal: Option<PortId>,
    tool: Option<ToolId>,
}

/// Lots assigned to a tool together (one, or a batch) with their queue entries and the lots
/// counted against the CoT campaign, all of which return if the tool gives the assignment back.
struct Assignment {
    lots: Vec<LotId>,
    entries: Vec<Waiting>,
    campaign: u32,
}

/// A tool's ports and the work coming to it.
struct Station {
    ports: Vec<PortId>,
    batch: bool,
    /// Assignments not begun, in assignment order.
    assignments: VecDeque<Assignment>,
    /// Assigned lots waiting for a free port to come to, and (batch tool) lots of a batch done
    /// waiting for one to leave by.
    docking: VecDeque<LotId>,
    exiting: VecDeque<LotId>,
}

pub(super) struct Logistics {
    pub system: Amhs,
    look_ahead: Option<usize>,
    ports: Vec<PortState>,
    /// Per lot slot.
    places: Vec<Place>,
    /// Per tool.
    stations: Vec<Station>,
    /// Per port: the tool it belongs to.
    port_tool: Vec<Option<ToolId>>,
    /// Per tool group: the bays of its tools, and a port of its first tool.
    group_bays: Vec<Vec<BayId>>,
    group_port: Vec<Option<PortId>>,
    /// Per bay: its rails with track buffers, each with its buffers by position.
    bay_buffers: Vec<Vec<(LinkId, Vec<PortId>)>>,
    /// Per part: the port of its commit station; the ports of the complete stations.
    commit: Vec<PortId>,
    completes: Vec<PortId>,
    /// Per tool group: it has no tools, and its steps are left out.
    pub skipped: Vec<bool>,
    /// Lots waiting for a free track buffer, longest first.
    buffer_waiters: VecDeque<LotId>,
    notices: Vec<Notice>,
    /// The window logged for a replay.
    log: Option<FabLog>,
}

/// What the fab logs of a window for its replay: where the FOUPs go, and the tools' timelines.
struct FabLog {
    from: Time,
    until: Time,
    started: bool,
    foups: Vec<FoupMove>,
    timelines: Vec<Timeline>,
}

impl FabLog {
    /// Changes at `now` belong to the window.
    fn open(&self, now: Time) -> bool {
        self.started && now < self.until
    }
}

impl Logistics {
    pub(super) fn new(data: &Arc<Dataset>, config: &AmhsConfig) -> Result<Self, Error> {
        let system = Amhs::new(Arc::clone(data), config)?;
        let layout = data.layout.as_ref().expect("a layout");
        let mut skipped = vec![false; data.tool_groups.len()];
        for &group in &layout.skipped {
            skipped[group] = true;
        }
        let mut stations = Vec::new();
        let mut group_tools = Vec::with_capacity(data.tool_groups.len());
        for group in &data.tool_groups {
            group_tools.push(stations.len()..stations.len() + group.tools as usize);
            for _ in 0..group.tools {
                stations.push(Station {
                    ports: Vec::new(),
                    batch: group.batching.is_some(),
                    assignments: VecDeque::new(),
                    docking: VecDeque::new(),
                    exiting: VecDeque::new(),
                });
            }
        }
        let mut port_tool = vec![None; layout.ports.len()];
        let (mut commits, mut completes) = (Vec::new(), Vec::new());
        let mut bay_buffers: Vec<Vec<(LinkId, Vec<PortId>)>> = vec![Vec::new(); layout.bays.len()];
        for (port, spec) in layout.ports.iter().enumerate() {
            match spec.kind {
                PortKind::Tool(tool) => {
                    stations[tool].ports.push(port);
                    port_tool[port] = Some(tool);
                }
                PortKind::Commit(_) => commits.push(port),
                PortKind::Complete(_) => completes.push(port),
                PortKind::Buffer(_) => {
                    let rails = &mut bay_buffers[layout.links[spec.link].bay];
                    match rails.iter_mut().find(|(rail, _)| *rail == spec.link) {
                        Some((_, ports)) => ports.push(port),
                        None => rails.push((spec.link, vec![port])),
                    }
                }
                // Stockers stand unused (assumed).
                PortKind::Stocker(_) => {}
            }
        }
        for rails in &mut bay_buffers {
            for (_, ports) in rails.iter_mut() {
                ports.sort_by(|&a, &b| {
                    layout.ports[a]
                        .offset
                        .total_cmp(&layout.ports[b].offset)
                        .then(a.cmp(&b))
                });
            }
        }
        let mut group_bays = vec![Vec::new(); data.tool_groups.len()];
        let mut group_port = vec![None; data.tool_groups.len()];
        for (group, tools) in group_tools.iter().enumerate() {
            if skipped[group] {
                continue;
            }
            for tool in tools.clone() {
                let bay = layout.tools[tool].as_ref().expect("a placed tool").bay;
                if !group_bays[group].contains(&bay) {
                    group_bays[group].push(bay);
                }
            }
            group_port[group] = Some(stations[tools.start].ports[0]);
        }
        // Each part enters at the commit station nearest to a tool of its first step with tools.
        let commit = data
            .parts
            .iter()
            .map(|part| {
                let group = data.routes[part.route]
                    .steps
                    .iter()
                    .map(|step| step.tool_group)
                    .find(|&group| !skipped[group])
                    .ok_or_else(|| Error(format!("part {}: no step has tools", part.name)))?;
                let nearest = |commit: PortId| {
                    group_tools[group]
                        .clone()
                        .flat_map(|tool| stations[tool].ports.iter())
                        .map(|&port| system.distance(commit, port))
                        .fold(f64::INFINITY, f64::min)
                };
                Ok(commits
                    .iter()
                    .map(|&commit| (nearest(commit), commit))
                    .min_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)))
                    .expect("commit stations")
                    .1)
            })
            .collect::<Result<Vec<_>, Error>>()?;
        Ok(Self {
            look_ahead: config.look_ahead.map(|look_ahead| look_ahead as usize),
            ports: vec![PortState::default(); layout.ports.len()],
            places: Vec::new(),
            stations,
            port_tool,
            group_bays,
            group_port,
            bay_buffers,
            commit,
            completes,
            skipped,
            buffer_waiters: VecDeque::new(),
            notices: Vec::new(),
            log: None,
            system,
        })
    }

    /// Logs `window` for a replay.
    pub(super) fn record(&mut self, window: &ReplayWindow) {
        self.system.record(window.from, window.until);
        self.log = Some(FabLog {
            from: window.from,
            until: window.until,
            started: false,
            foups: Vec::new(),
            timelines: Vec::new(),
        });
    }

    /// Where the lot's FOUP is, for the log: none out of the fab.
    fn foup_place(&self, id: LotId) -> Option<(FoupPlace, usize)> {
        let place = self.places[id];
        Some(match place.at {
            At::Port(port) => (FoupPlace::Port, port),
            At::Commit(port) => (FoupPlace::Commit, port),
            At::Inside(tool) => (FoupPlace::Tool, tool),
            At::Vehicle => (FoupPlace::Vehicle, self.system.carrier(place.job?)?),
            At::Nowhere => return None,
        })
    }

    fn free_port(&self, tool: ToolId) -> Option<PortId> {
        self.stations[tool]
            .ports
            .iter()
            .copied()
            .find(|&port| self.ports[port].free())
    }

    /// Drops the reservation of `port` for lot `id`.
    fn unreserve(&mut self, port: PortId, id: LotId) {
        if self.ports[port].reserved == Some(id) {
            self.ports[port].reserved = None;
        }
    }
}

impl Fab {
    fn logistics_ref(&self) -> &Logistics {
        self.logistics.as_ref().expect("a layout")
    }

    fn logistics_mut(&mut self) -> &mut Logistics {
        self.logistics.as_mut().expect("a layout")
    }

    fn port_kind(&self, port: PortId) -> PortKind {
        self.data.layout.as_ref().expect("a layout").ports[port].kind
    }

    /// The first failure of the AMHS, which stops the run.
    pub(super) fn amhs_failure(&self) -> Option<&str> {
        self.logistics
            .as_ref()
            .and_then(|logistics| logistics.system.failure())
    }

    /// Before the event at `now`: the replay's window begins if due, with every vehicle, FOUP and
    /// tool as it stands at the window's start.
    pub(super) fn begin_replay(&mut self, now: Time) {
        let Some(logistics) = &mut self.logistics else {
            return;
        };
        if !logistics.system.log_due(now) {
            return;
        }
        logistics.system.begin_log();
        let places: Vec<_> = (0..self.lots.len())
            .filter(|&id| self.lots[id].alive)
            .filter_map(|id| logistics.foup_place(id).map(|place| (id, place)))
            .collect();
        let log = logistics.log.as_mut().expect("a replay log");
        for (id, (place, index)) in places {
            let lot = &self.lots[id];
            log.foups.push(FoupMove {
                time: log.from,
                lot: lot.serial,
                kind: lot.kind,
                place,
                index,
            });
        }
        for (tool, state) in self.tools.iter().enumerate() {
            log.timelines.push(Timeline {
                known: log.from,
                tool,
                changes: state.timeline(log.from),
            });
        }
        log.started = true;
    }

    /// The lot's FOUP moved at `now` (out of the fab: `gone` at the complete station's port).
    fn log_foup(&mut self, id: LotId, now: Time, gone: Option<PortId>) {
        let Some(logistics) = &mut self.logistics else {
            return;
        };
        let place = match gone {
            Some(port) => Some((FoupPlace::Gone, port)),
            None => logistics.foup_place(id),
        };
        if let Some(log) = logistics.log.as_mut().filter(|log| log.open(now))
            && let Some((place, index)) = place
        {
            let lot = &self.lots[id];
            log.foups.push(FoupMove {
                time: now,
                lot: lot.serial,
                kind: lot.kind,
                place,
                index,
            });
        }
    }

    /// The tool's states from `now` on changed (a job, an outage or a PM began or ended).
    pub(super) fn log_tool_states(&mut self, tool: ToolId, now: Time) {
        if let Some(log) = self
            .logistics
            .as_mut()
            .and_then(|logistics| logistics.log.as_mut())
            .filter(|log| log.open(now))
        {
            log.timelines.push(Timeline {
                known: now,
                tool,
                changes: self.tools[tool].timeline(now),
            });
        }
    }

    /// The replay recorded so far, up to its window's end or `now`; none before its window began.
    pub(super) fn replay(&self, now: Time) -> Option<Replay> {
        let logistics = self.logistics.as_ref()?;
        let log = logistics.log.as_ref().filter(|log| log.started)?;
        let until = log.until.min(now);
        let (tracks, kinematics, activities) = logistics.system.replay(until)?;
        let layout = self.data.layout.as_ref().expect("a layout");
        let recorded = Recorded {
            from: log.from,
            until,
            kinematics,
            tracks,
            activities,
            foups: &log.foups,
            timelines: &log.timelines,
            rails: layout.links.len(),
            ports: layout.ports.len(),
            tools: self.tools.len(),
        };
        Some(recorded.replay())
    }

    /// The AMHS at `now`; none without a layout.
    pub(super) fn amhs_status(&self, now: Time) -> Option<AmhsStatus> {
        let logistics = self.logistics.as_ref()?;
        let (mut foups, mut kept) = (Vec::new(), Vec::new());
        for (port, state) in logistics.ports.iter().enumerate() {
            match (state.lot, state.reserved) {
                (Some(id), _) => foups.push(FoupStatus {
                    port,
                    lot: self.lots[id].serial,
                    kind: self.lots[id].kind,
                }),
                (None, Some(_)) => kept.push(port),
                (None, None) => {}
            }
        }
        let (mut committed, mut inside): (Vec<FoupCount>, Vec<FoupCount>) =
            (Vec::new(), Vec::new());
        let count = |counts: &mut Vec<FoupCount>, at: usize| match counts
            .iter_mut()
            .find(|count| count.at == at)
        {
            Some(count) => count.foups += 1,
            None => counts.push(FoupCount { at, foups: 1 }),
        };
        for (id, _) in self.lots.iter().enumerate().filter(|(_, lot)| lot.alive) {
            match logistics.places[id].at {
                At::Commit(port) => count(&mut committed, port),
                At::Inside(tool) => count(&mut inside, tool),
                At::Port(_) | At::Vehicle | At::Nowhere => {}
            }
        }
        committed.sort_unstable_by_key(|count| count.at);
        inside.sort_unstable_by_key(|count| count.at);
        Some(AmhsStatus {
            vehicles: logistics.system.statuses(now, |lot| self.lots[lot].serial),
            foups,
            kept,
            committed,
            inside,
            tools: self.tools.iter().map(|tool| tool.state(now)).collect(),
            backlog: logistics.system.backlog(),
            report: logistics.system.window(now),
        })
    }

    /// Whether the tool takes another assignment: up with no PM due, a free port unless it
    /// batches or a lot of its group's queue stands at one of its ports, and fewer jobs and
    /// assignments than it runs at once plus the look-ahead.
    pub(super) fn assignable(&self, tool: ToolId) -> bool {
        let state = &self.tools[tool];
        let logistics = self.logistics_ref();
        let station = &logistics.stations[tool];
        let capacity = if state.cascading { 2 } else { 1 };
        let room = match (logistics.look_ahead, station.batch) {
            (Some(look_ahead), _) => self.load(tool) < capacity + look_ahead,
            (None, true) => self.load(tool) == 0,
            (None, false) => true,
        };
        state.breakdowns == 0
            && state.pm.is_none()
            && state.pm_pending.is_empty()
            && room
            && (station.batch
                || logistics.free_port(tool).is_some()
                || station.ports.iter().any(|&port| {
                    logistics.ports[port]
                        .lot
                        .is_some_and(|id| self.waits_at(tool, id))
                }))
    }

    /// The lot is in the queue of the tool's group (not assigned) and its FOUP stands at a port of
    /// the tool.
    fn waits_at(&self, tool: ToolId, id: LotId) -> bool {
        let logistics = self.logistics_ref();
        let place = logistics.places[id];
        self.lots[id].state == LotState::Queued
            && place.tool.is_none()
            && self.group_of(id) == self.tools[tool].group
            && matches!(place.at, At::Port(port) if logistics.port_tool[port] == Some(tool))
    }

    /// The lots a tool without a free port can take: those whose FOUPs stand at its ports; none
    /// (any lot) for a tool with one, a batch tool, or without a layout.
    pub(super) fn dockable(&self, tool: ToolId) -> Option<Vec<LotId>> {
        let logistics = self.logistics.as_ref()?;
        let station = &logistics.stations[tool];
        if station.batch || logistics.free_port(tool).is_some() {
            return None;
        }
        Some(
            station
                .ports
                .iter()
                .filter_map(|&port| logistics.ports[port].lot)
                .filter(|&id| self.waits_at(tool, id))
                .collect(),
        )
    }

    /// The tool at one of whose ports the lot's FOUP stands.
    pub(super) fn standing_at(&self, id: LotId) -> Option<ToolId> {
        let logistics = self.logistics_ref();
        match logistics.places[id].at {
            At::Port(port) => logistics.port_tool[port],
            _ => None,
        }
    }

    /// The tool's jobs and assignments: what it processes before a new assignment.
    pub(super) fn load(&self, tool: ToolId) -> usize {
        self.tools[tool].busy() + self.logistics_ref().stations[tool].assignments.len()
    }

    /// When the tool's jobs end (0 without jobs): of the tools with the same load, dispatching
    /// gives a lot to the one free first.
    pub(super) fn free_at(&self, tool: ToolId) -> Time {
        self.tools[tool]
            .jobs
            .iter()
            .filter(|job| job.active)
            .map(|job| job.end)
            .max()
            .unwrap_or(0)
    }

    /// The tool the lot is assigned to.
    pub(super) fn assigned_tool(&self, id: LotId) -> Option<ToolId> {
        self.logistics
            .as_ref()
            .and_then(|logistics| logistics.places[id].tool)
    }

    /// A lot just released: its FOUP in its part's commit station or, initial WIP, in the free
    /// track buffer nearest from the first tool of its first step with tools from its present
    /// one, if one is free.
    pub(super) fn place_released(&mut self, id: LotId, initial: bool, now: Time) {
        let lot = &self.lots[id];
        let logistics = self.logistics_ref();
        let buffer = if initial {
            self.data.routes[lot.route].steps[lot.step..]
                .iter()
                .map(|step| step.tool_group)
                .find(|&group| !logistics.skipped[group])
                .and_then(|group| {
                    self.choose_buffer(group, logistics.group_port[group].expect("tools"))
                })
        } else {
            None
        };
        let commit = logistics.commit[lot.part];
        let logistics = self.logistics_mut();
        if logistics.places.len() <= id {
            logistics.places.resize(id + 1, Place::default());
        }
        logistics.places[id] = Place {
            at: At::Commit(commit),
            ..Place::default()
        };
        if let Some(port) = buffer {
            logistics.ports[port].lot = Some(id);
            logistics.places[id].at = At::Port(port);
        }
        self.log_foup(id, now, None);
    }

    /// The free track buffer nearest from port `from` in the bays of `group`'s tools, else in the
    /// nearest ring of neighbor bays around them that has one.
    fn choose_buffer(&self, group: ToolGroupId, from: PortId) -> Option<PortId> {
        let logistics = self.logistics_ref();
        let layout = self.data.layout.as_ref().expect("a layout");
        let origin = &layout.ports[from];
        let free = |port: &&PortId| logistics.ports[**port].free();
        let mut seen = vec![false; layout.bays.len()];
        let mut ring = logistics.group_bays[group].clone();
        for &bay in &ring {
            seen[bay] = true;
        }
        while !ring.is_empty() {
            let mut best: Option<(f64, PortId)> = None;
            for &bay in &ring {
                for (rail, ports) in &logistics.bay_buffers[bay] {
                    // Nearest on a rail: the first free one past `from` on it, else the first.
                    let ahead = if *rail == origin.link {
                        ports
                            .iter()
                            .filter(free)
                            .find(|&&port| layout.ports[port].offset >= origin.offset)
                    } else {
                        None
                    };
                    let Some(&port) = ahead.or_else(|| ports.iter().find(free)) else {
                        continue;
                    };
                    let distance = logistics.system.distance(from, port);
                    if best.is_none_or(|(least, nearest)| {
                        distance < least || (distance == least && port < nearest)
                    }) {
                        best = Some((distance, port));
                    }
                }
            }
            if let Some((_, port)) = best {
                return Some(port);
            }
            let mut next = Vec::new();
            for &bay in &ring {
                for &neighbor in &layout.bays[bay].neighbors {
                    if !seen[neighbor] {
                        seen[neighbor] = true;
                        next.push(neighbor);
                    }
                }
            }
            ring = next;
        }
        None
    }

    /// The port the lot's FOUP stands at or is carried to; for a FOUP in a batch tool, a port of
    /// the tool.
    fn position(&self, id: LotId) -> PortId {
        let logistics = self.logistics_ref();
        let place = logistics.places[id];
        match place.at {
            At::Port(port) | At::Commit(port) => port,
            At::Vehicle => place.dropoff.expect("a drop-off"),
            At::Inside(tool) => logistics.stations[tool].ports[0],
            At::Nowhere => unreachable!("lot {id} is not in the fab"),
        }
    }

    /// The lot waits for a tool with nowhere to go yet and stands in the way: at a port of a
    /// tool it is not assigned to, or in a commit station.
    fn stranded(&self, id: LotId) -> bool {
        let logistics = self.logistics_ref();
        let place = logistics.places[id];
        self.lots[id].state == LotState::Queued
            && place.goal.is_none()
            && match place.at {
                At::Port(port) => {
                    logistics.port_tool[port].is_some_and(|tool| place.tool != Some(tool))
                }
                At::Commit(_) => true,
                At::Vehicle | At::Inside(_) | At::Nowhere => false,
            }
    }

    /// A lot with nowhere to go yet leaves a tool port or a commit station for a track buffer,
    /// or waits for one to come free.
    pub(super) fn settle(&mut self, id: LotId, sched: &mut Scheduler<Event>) {
        if !self.stranded(id) {
            return;
        }
        match self.choose_buffer(self.group_of(id), self.position(id)) {
            Some(port) => self.send(id, port, sched),
            None => {
                let waiters = &mut self.logistics_mut().buffer_waiters;
                if !waiters.contains(&id) {
                    waiters.push_back(id);
                }
            }
        }
    }

    /// The lot's FOUP heads for port `to`, kept for it unless a complete station's: by a new
    /// transport from where it stands, or by its running one sent there instead. Where that one's
    /// drop-off has begun, it goes on from where it lands; in a batch tool, once it is out.
    fn send(&mut self, id: LotId, to: PortId, sched: &mut Scheduler<Event>) {
        let kept = !matches!(self.port_kind(to), PortKind::Complete(_));
        let now = sched.now();
        let logistics = self.logistics_mut();
        let place = logistics.places[id];
        if let Some(old) = place.goal
            && old != to
            && Some(old) != place.dropoff
        {
            logistics.unreserve(old, id);
        }
        if kept {
            debug_assert!(
                logistics.ports[to].reserved.is_none_or(|lot| lot == id),
                "port {to} kept for another lot"
            );
            logistics.ports[to].reserved = Some(id);
        }
        logistics.places[id].goal = Some(to);
        match (place.job, place.at) {
            (Some(_), _) if place.dropoff == Some(to) => {}
            (Some(job), _) => {
                if logistics.system.retarget(job, to, sched) {
                    if let Some(old) = place.dropoff {
                        logistics.unreserve(old, id);
                    }
                    logistics.places[id].dropoff = Some(to);
                }
            }
            (None, At::Port(from) | At::Commit(from)) => {
                debug_assert_ne!(from, to, "lot {id} sent where it stands");
                let job = logistics.system.request(id, from, to, sched);
                let place = &mut logistics.places[id];
                place.job = Some(job);
                place.dropoff = Some(to);
                self.times[id].requested = now;
            }
            (None, At::Inside(_)) => {}
            (None, at) => unreachable!("lot {id} without a transport while {at:?}"),
        }
    }

    /// The lot's FOUP stays where it stands: its goal is dropped and its transport, if any,
    /// called off. False once the pickup has begun.
    fn call_off(&mut self, id: LotId, sched: &mut Scheduler<Event>) -> bool {
        let logistics = self.logistics_mut();
        let place = logistics.places[id];
        if let Some(job) = place.job
            && !logistics.system.cancel(job, sched)
        {
            return false;
        }
        for port in [place.goal, place.dropoff].into_iter().flatten() {
            logistics.unreserve(port, id);
        }
        let place = &mut logistics.places[id];
        place.job = None;
        place.dropoff = None;
        place.goal = None;
        true
    }

    /// Assigns the selected lots (one, or a batch) to `tool`: they leave the queue and head for
    /// its ports.
    pub(super) fn assign(&mut self, tool: ToolId, sched: &mut Scheduler<Event>) {
        let now = sched.now();
        let lots = mem::take(&mut self.selected);
        let group = self.tools[tool].group;
        self.groups[group].integrate_queue(now);
        let mut entries = Vec::with_capacity(lots.len());
        self.groups[group].queue.retain(|waiting| {
            let assigned = lots.contains(&waiting.lot);
            if assigned {
                entries.push(waiting.clone());
            }
            !assigned
        });
        entries.sort_by_key(|entry| lots.iter().position(|&lot| lot == entry.lot));
        let head = &self.lots[lots[0]];
        self.project(tool, self.data.routes[head.route].steps[head.step].setup);
        // CoT counts the engineering lots of a campaign as they are dispatched.
        let mut campaign = 0;
        for &id in &lots {
            if self.lots[id].kind.engineering()
                && self.groups[group].campaign > 0
                && self.strategy.steppers[group]
            {
                self.groups[group].campaign -= 1;
                campaign += 1;
            }
            self.logistics_mut().places[id].tool = Some(tool);
        }
        self.logistics_mut().stations[tool]
            .assignments
            .push_back(Assignment {
                lots: lots.clone(),
                entries,
                campaign,
            });
        for &id in &lots {
            self.dock(tool, id, sched);
        }
        self.selected = lots;
        self.selected.clear();
        self.set_unready(tool);
        self.refresh(tool, sched);
        self.try_start(tool, sched);
    }

    /// The tool's next setup and run after one more job needing `setup`.
    fn project(&mut self, tool: ToolId, setup: Option<StepSetup>) {
        let state = &self.tools[tool];
        let rule = self.data.tool_groups[state.group].rule;
        let (next, run_left, _) = super::fab::after_job(
            &self.data,
            rule,
            state.next_setup,
            state.next_run_left,
            setup,
        );
        let state = &mut self.tools[tool];
        state.next_setup = next;
        state.next_run_left = run_left;
    }

    /// The tool's next setup and run again: from its own over its assignments.
    pub(super) fn reproject(&mut self, tool: ToolId) {
        let state = &mut self.tools[tool];
        state.next_setup = state.setup;
        state.next_run_left = state.run_left;
        for index in 0..self.logistics_ref().stations[tool].assignments.len() {
            let head = self.logistics_ref().stations[tool].assignments[index].lots[0];
            let lot = &self.lots[head];
            self.project(tool, self.data.routes[lot.route].steps[lot.step].setup);
        }
    }

    /// A lot assigned to a tool heads for a free port of it, or waits for one where it is or in a
    /// track buffer. Standing at a port of the tool it stays unless its pickup there has begun (a
    /// batch tool takes it in), as it does still in a batch tool from its last batch there.
    fn dock(&mut self, tool: ToolId, id: LotId, sched: &mut Scheduler<Event>) {
        let place = self.logistics_ref().places[id];
        match place.at {
            At::Inside(inside) if inside == tool && self.call_off(id, sched) => {
                let station = &mut self.logistics_mut().stations[tool];
                station.exiting.retain(|&lot| lot != id);
                return;
            }
            At::Port(port)
                if self.logistics_ref().port_tool[port] == Some(tool)
                    && self.call_off(id, sched) =>
            {
                if self.logistics_ref().stations[tool].batch {
                    self.take_in(tool, id, port, sched);
                }
                return;
            }
            _ => {}
        }
        match self.logistics_ref().free_port(tool) {
            Some(port) => self.send(id, port, sched),
            None => {
                self.logistics_mut().stations[tool].docking.push_back(id);
                self.settle(id, sched);
            }
        }
    }

    /// The FOUP at `port` goes into the batch tool; the port is free again.
    fn take_in(&mut self, tool: ToolId, id: LotId, port: PortId, sched: &mut Scheduler<Event>) {
        let logistics = self.logistics_mut();
        logistics.ports[port].lot = None;
        logistics.places[id].at = At::Inside(tool);
        self.log_foup(id, sched.now(), None);
        self.port_freed(port, sched);
    }

    /// Starts the best-ranked of the tool's assignments whose FOUPs are all at its ports (in it,
    /// for a batch tool) as long as the tool can take a job; the first assigned among equals.
    pub(super) fn try_start(&mut self, tool: ToolId, sched: &mut Scheduler<Event>) {
        while self.tools[tool].available() {
            let now = sched.now();
            let logistics = self.logistics_ref();
            let present = |&id: &LotId| {
                let place = logistics.places[id];
                place.goal.is_none()
                    && match place.at {
                        At::Port(port) => logistics.port_tool[port] == Some(tool),
                        At::Inside(inside) => inside == tool,
                        At::Commit(_) | At::Vehicle | At::Nowhere => false,
                    }
            };
            // A batch ranks by its best lot, the first of its entries.
            let Some(index) = self.best_ranked(
                tool,
                logistics.stations[tool]
                    .assignments
                    .iter()
                    .enumerate()
                    .filter(|(_, assignment)| assignment.lots.iter().all(present))
                    .map(|(index, assignment)| (index, &assignment.entries[0])),
                now,
            ) else {
                return;
            };
            let logistics = self.logistics_mut();
            let assignment = logistics.stations[tool]
                .assignments
                .remove(index)
                .expect("an assignment");
            for &id in &assignment.lots {
                logistics.places[id].tool = None;
            }
            self.begin_job(tool, assignment.lots, false, sched);
        }
    }

    /// After a batch: its FOUPs leave the tool port by port.
    pub(super) fn batch_done(
        &mut self,
        tool: ToolId,
        lots: &[LotId],
        sched: &mut Scheduler<Event>,
    ) {
        self.logistics_mut().stations[tool].exiting.extend(lots);
        for index in 0..self.logistics_ref().stations[tool].ports.len() {
            let port = self.logistics_ref().stations[tool].ports[index];
            self.port_freed(port, sched);
        }
    }

    /// Port `port` may have come free: a batch tool hands a FOUP out by it or calls the next of
    /// its batches' FOUPs in; a tool takes a new assignment; a track buffer goes to a lot waiting
    /// for one.
    fn port_freed(&mut self, port: PortId, sched: &mut Scheduler<Event>) {
        let logistics = self.logistics_mut();
        if !logistics.ports[port].free() {
            return;
        }
        let Some(tool) = logistics.port_tool[port] else {
            if let PortKind::Buffer(_) = self.port_kind(port) {
                self.serve_buffer_waiters(sched);
            }
            return;
        };
        let station = &mut logistics.stations[tool];
        if let Some(id) = station.exiting.pop_front() {
            let logistics = self.logistics_mut();
            logistics.ports[port].lot = Some(id);
            logistics.places[id].at = At::Port(port);
            self.log_foup(id, sched.now(), None);
            self.out_of_batch_tool(id, sched);
        } else if let Some(id) = station.docking.pop_front() {
            self.send(id, port, sched);
        } else {
            self.refresh(tool, sched);
            if self.tools[tool].ready {
                self.dispatch(self.tools[tool].group, None, sched);
            }
        }
    }

    /// A FOUP just out of a batch tool goes on: out of the fab, or where it is to go, or to a
    /// track buffer. A lot about to join its next step's queue waits for that.
    fn out_of_batch_tool(&mut self, id: LotId, sched: &mut Scheduler<Event>) {
        match self.lots[id].state {
            LotState::Leaving => self.leave(id, sched),
            LotState::Moving => {}
            LotState::Queued | LotState::Processing => match self.logistics_ref().places[id].goal {
                Some(goal) => self.send(id, goal, sched),
                None => self.settle(id, sched),
            },
        }
    }

    /// Free track buffers go to the lots waiting for one, longest waiting first.
    fn serve_buffer_waiters(&mut self, sched: &mut Scheduler<Event>) {
        while let Some(&id) = self.logistics_ref().buffer_waiters.front() {
            if !(self.lots[id].alive && self.stranded(id)) {
                self.logistics_mut().buffer_waiters.pop_front();
                continue;
            }
            let Some(port) = self.choose_buffer(self.group_of(id), self.position(id)) else {
                return;
            };
            self.logistics_mut().buffer_waiters.pop_front();
            self.send(id, port, sched);
        }
    }

    /// A lot past its last step heads for the nearest complete station (once out of a batch
    /// tool).
    pub(super) fn leave(&mut self, id: LotId, sched: &mut Scheduler<Event>) {
        self.lots[id].state = LotState::Leaving;
        if let At::Inside(_) = self.logistics_ref().places[id].at {
            return;
        }
        let from = self.position(id);
        let logistics = self.logistics_ref();
        let to = logistics
            .completes
            .iter()
            .map(|&port| (logistics.system.distance(from, port), port))
            .min_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)))
            .expect("complete stations")
            .1;
        self.send(id, to, sched);
    }

    /// The tool goes down or falls due for a PM: it is kept for no reservation, and its
    /// assignments not begun go back to the queue, except a batch whose FOUPs are partly in the
    /// tool. Lots no other tool takes now leave for a track buffer: from a tool port or a commit
    /// station, or on their way to this tool.
    pub(super) fn release_assignments(&mut self, tool: ToolId, sched: &mut Scheduler<Event>) {
        let now = sched.now();
        self.unhold(tool);
        self.refresh(tool, sched);
        let logistics = self.logistics.as_mut().expect("a layout");
        let places = &logistics.places;
        let station = &mut logistics.stations[tool];
        let (kept, released): (VecDeque<Assignment>, VecDeque<Assignment>) =
            mem::take(&mut station.assignments)
                .into_iter()
                .partition(|assignment| {
                    assignment
                        .lots
                        .iter()
                        .any(|&id| places[id].at == At::Inside(tool))
                });
        station.assignments = kept;
        if released.is_empty() {
            return;
        }
        station.docking.retain(|id| {
            !released
                .iter()
                .any(|assignment| assignment.lots.contains(id))
        });
        let group = self.tools[tool].group;
        self.groups[group].integrate_queue(now);
        let mut lots = Vec::new();
        for assignment in released {
            self.groups[group].campaign += assignment.campaign;
            for (id, entry) in assignment.lots.into_iter().zip(assignment.entries) {
                self.logistics_mut().places[id].tool = None;
                self.groups[group].queue.push(entry);
                lots.push(id);
            }
        }
        self.reproject(tool);
        self.dispatch(group, None, sched);
        for id in lots {
            if self.logistics_ref().places[id].tool.is_none() {
                self.redirect(id, tool, sched);
            }
        }
    }

    /// A lot `tool` gave back that no other tool took: on its way to the tool it goes to a track
    /// buffer instead (it stays where it stands if its pickup has not begun; with no buffer free
    /// it lands at the tool and waits there); standing at a tool port or a commit station it
    /// leaves for one.
    fn redirect(&mut self, id: LotId, tool: ToolId, sched: &mut Scheduler<Event>) {
        let logistics = self.logistics_ref();
        let place = logistics.places[id];
        if let Some(goal) = place.goal
            && logistics.port_tool[goal] == Some(tool)
            && (place.at == At::Vehicle || !self.call_off(id, sched))
        {
            if let Some(port) = self.choose_buffer(self.group_of(id), goal) {
                self.send(id, port, sched);
            }
            return;
        }
        self.settle(id, sched);
    }

    /// The vehicle event of the AMHS.
    pub(super) fn vehicle_event(&mut self, vehicle: u32, epoch: u32, sched: &mut Scheduler<Event>) {
        self.logistics_mut()
            .system
            .handle(vehicle as usize, epoch, sched);
        self.amhs_notices(sched);
    }

    /// Applies what the AMHS did to FOUPs.
    fn amhs_notices(&mut self, sched: &mut Scheduler<Event>) {
        loop {
            let logistics = self.logistics_mut();
            let mut notices = mem::take(&mut logistics.notices);
            logistics.system.take_notices(&mut notices);
            if notices.is_empty() {
                self.logistics_mut().notices = notices;
                return;
            }
            for notice in notices.drain(..) {
                match notice {
                    Notice::PickedUp { job, lot, port } => self.picked_up(lot, job, port, sched),
                    Notice::Delivered { job, lot, port } => self.delivered(lot, job, port, sched),
                }
            }
            self.logistics_mut().notices = notices;
        }
    }

    fn picked_up(&mut self, id: LotId, job: usize, port: PortId, sched: &mut Scheduler<Event>) {
        self.log_transport(sched.now(), EventKind::Pickup, id, port);
        let logistics = self.logistics_mut();
        debug_assert_eq!(logistics.places[id].job, Some(job));
        logistics.places[id].at = At::Vehicle;
        if logistics.ports[port].lot == Some(id) {
            logistics.ports[port].lot = None;
        }
        self.log_foup(id, sched.now(), None);
        self.port_freed(port, sched);
    }

    fn delivered(&mut self, id: LotId, job: usize, port: PortId, sched: &mut Scheduler<Event>) {
        let now = sched.now();
        self.times[id].moved += now - self.times[id].requested;
        self.log_transport(now, EventKind::Dropoff, id, port);
        let complete = matches!(self.port_kind(port), PortKind::Complete(_));
        let logistics = self.logistics_mut();
        debug_assert_eq!(logistics.places[id].job, Some(job));
        if complete {
            logistics.places[id] = Place::default();
            self.log_foup(id, now, Some(port));
            self.complete(id, sched);
            return;
        }
        debug_assert!(logistics.ports[port].lot.is_none(), "port {port} taken");
        logistics.ports[port].lot = Some(id);
        logistics.unreserve(port, id);
        let place = &mut logistics.places[id];
        place.at = At::Port(port);
        place.job = None;
        place.dropoff = None;
        if place.goal == Some(port) {
            place.goal = None;
        }
        let place = *place;
        self.log_foup(id, now, None);
        let logistics = self.logistics_ref();
        match (place.goal, place.tool) {
            // Landed short of where it is to go: on from here.
            (Some(goal), _) => self.send(id, goal, sched),
            (None, Some(tool)) if logistics.port_tool[port] == Some(tool) => {
                if logistics.stations[tool].batch {
                    self.take_in(tool, id, port, sched);
                }
                self.try_start(tool, sched);
            }
            (None, _) => self.settle(id, sched),
        }
    }

    /// Records a FOUP's pickup or drop-off at `port` in the events record.
    fn log_transport(&mut self, now: Time, kind: EventKind, id: LotId, port: PortId) {
        let tool = self.logistics_ref().port_tool[port];
        self.log(now, |fab| Entry {
            kind,
            lot: Some(fab.lots[id].serial),
            part: Some(fab.lots[id].part),
            tool,
            tool_group: tool.map(|tool| fab.tools[tool].group),
            step: Some(fab.lots[id].step),
        });
    }

    /// The lot's FOUP for its status: the port it stands at or the vehicle carrying it.
    pub(super) fn lot_place(&self, id: LotId, status: &mut LotStatus) {
        let Some(logistics) = &self.logistics else {
            return;
        };
        let place = logistics.places[id];
        status.port = match place.at {
            At::Port(port) | At::Commit(port) => Some(port),
            At::Vehicle | At::Inside(_) | At::Nowhere => None,
        };
        status.vehicle = place.job.and_then(|job| logistics.system.carrier(job));
    }
}

#[cfg(test)]
impl Fab {
    /// The FOUP bookkeeping: ports and places agree, a transport goes to a goal and both ports
    /// are kept for the lot, assigned lots wait among their tool's assignments, a processing lot's
    /// FOUP is at its tool, and only batch tools hold FOUPs.
    fn check_logistics(&self) -> Result<(), String> {
        let logistics = self.logistics_ref();
        for (port, state) in logistics.ports.iter().enumerate() {
            if let Some(id) = state.lot
                && logistics.places[id].at != At::Port(port)
            {
                return Err(format!("port {port} holds lot {id}, which is elsewhere"));
            }
            if let Some(id) = state.reserved {
                let place = logistics.places[id];
                if place.goal != Some(port) && place.dropoff != Some(port) {
                    return Err(format!(
                        "port {port} kept for lot {id}, which goes elsewhere"
                    ));
                }
            }
        }
        let mut processing = vec![None; self.lots.len()];
        for (tool, state) in self.tools.iter().enumerate() {
            for job in state.jobs.iter().filter(|job| job.active) {
                for &id in &job.lots {
                    processing[id] = Some(tool);
                }
            }
        }
        for (id, lot) in self.lots.iter().enumerate() {
            let place = logistics.places[id];
            if !lot.alive {
                if place.at != At::Nowhere {
                    return Err(format!("completed lot {id} left a FOUP at {:?}", place.at));
                }
                continue;
            }
            let misplaced = match place.at {
                At::Nowhere => true,
                At::Port(port) => logistics.ports[port].lot != Some(id),
                At::Vehicle => place.job.is_none(),
                At::Inside(tool) => !logistics.stations[tool].batch,
                At::Commit(_) => false,
            };
            if misplaced {
                return Err(format!("lot {id} at {:?} unaccounted", place.at));
            }
            if place.job.is_some() != place.dropoff.is_some()
                || (place.job.is_some() && place.goal.is_none())
            {
                return Err(format!("lot {id}: transport without a destination"));
            }
            for port in [place.goal, place.dropoff].into_iter().flatten() {
                if !matches!(self.port_kind(port), PortKind::Complete(_))
                    && logistics.ports[port].reserved != Some(id)
                {
                    return Err(format!("lot {id} goes to port {port}, not kept for it"));
                }
            }
            if let Some(tool) = processing[id] {
                let at_tool = match place.at {
                    At::Port(port) => logistics.port_tool[port] == Some(tool),
                    At::Inside(inside) => inside == tool,
                    _ => false,
                };
                if !at_tool || place.job.is_some() || place.tool.is_some() {
                    return Err(format!("lot {id} processing on tool {tool} away from it"));
                }
            }
            if let Some(tool) = place.tool
                && (lot.state != LotState::Queued
                    || !logistics.stations[tool]
                        .assignments
                        .iter()
                        .any(|assignment| assignment.lots.contains(&id)))
            {
                return Err(format!("lot {id} assigned to tool {tool} unlisted"));
            }
        }
        for (tool, station) in logistics.stations.iter().enumerate() {
            let assigned = station
                .assignments
                .iter()
                .flat_map(|assignment| &assignment.lots)
                .chain(&station.docking);
            for &id in assigned {
                if logistics.places[id].tool != Some(tool) {
                    return Err(format!("tool {tool} lists lot {id}, assigned elsewhere"));
                }
            }
            for &id in &station.exiting {
                if logistics.places[id].at != At::Inside(tool) {
                    return Err(format!("lot {id} leaves tool {tool} from outside it"));
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use des_core::{DAY, HOUR, MINUTE};

    use super::*;
    use crate::data::{BatchCriterion, BatchSize, Step, Unit, tiny};
    use crate::sim::amhs::Activity;
    use crate::sim::tool::ToolState;
    use crate::sim::{AmhsReport, Config, Dispatch, Frame, Moves, Player, Recording, Simulation};
    use crate::smat::tests::loop_layout;

    /// tiny() on the loop layout: its step three times over, `lots` lots one every `interval`,
    /// batches of 2 to 3 lots if `batch`.
    fn dataset(lots: u32, interval: Time, batch: bool) -> Arc<Dataset> {
        let mut data = tiny();
        data.layout = Some(loop_layout());
        data.routes[0].steps = (1..=3)
            .map(|n| Step {
                name: n.to_string(),
                ..tiny().routes.remove(0).steps.remove(0)
            })
            .collect();
        data.streams[0].count = lots;
        data.streams[0].interval = interval;
        if batch {
            data.tool_groups[0].batching = Some(BatchCriterion::SameRouteStep);
            for step in &mut data.routes[0].steps {
                step.batch = Some(BatchSize { min: 50, max: 75 });
                step.unit = Unit::Batch;
                step.setup = None;
            }
        }
        Arc::new(data)
    }

    /// Runs `config` on `data` to the end, checking the vehicles and the FOUPs every `step`.
    fn run_checked(data: Arc<Dataset>, config: Config, step: Time) -> Simulation {
        let mut simulation = Simulation::new(data, config).unwrap();
        let mut now = 0;
        while !simulation.progress().finished {
            now += step;
            assert!(now < 30 * DAY, "the run does not end");
            simulation
                .run(Some(now))
                .unwrap_or_else(|error| panic!("{error}"));
            let fab = simulation.engine.model();
            fab.logistics_ref()
                .system
                .check(now)
                .unwrap_or_else(|error| panic!("{error}"));
            fab.check_logistics()
                .unwrap_or_else(|error| panic!("at {now} ms: {error}"));
        }
        simulation
    }

    /// The AMHS measures of every reported window, after checking that every lot completed and
    /// that the vehicles' activities fill each window.
    fn reports(simulation: &Simulation) -> Vec<AmhsReport> {
        let results = simulation.results().unwrap();
        assert_eq!(results.completed, results.released);
        results
            .periods
            .iter()
            .map(|period| {
                let amhs = period.amhs.clone().expect("AMHS measures");
                let times = amhs.vehicle_time;
                let total = times.idle
                    + times.to_pickup
                    + times.loading
                    + times.to_dropoff
                    + times.unloading;
                let window = (period.end - period.start) * Time::from(amhs.vehicles);
                assert!(
                    (total - window).abs() <= Time::from(amhs.vehicles),
                    "{total} ≠ {window}"
                );
                amhs
            })
            .collect()
    }

    #[test]
    fn lots_flow_tool_to_tool_and_through_buffers() {
        let simulation = run_checked(dataset(30, 20 * MINUTE, false), Config::new(DAY), 500);
        let released = simulation.results().unwrap().released;
        let mut moves = Moves::default();
        for report in reports(&simulation) {
            let more = report.moves;
            moves.tool_to_tool += more.tool_to_tool;
            moves.tool_to_buffer += more.tool_to_buffer;
            moves.tool_to_complete += more.tool_to_complete;
            moves.buffer_to_tool += more.buffer_to_tool;
            moves.buffer_to_buffer += more.buffer_to_buffer;
            moves.buffer_to_complete += more.buffer_to_complete;
            moves.commit_to_tool += more.commit_to_tool;
            moves.commit_to_buffer += more.commit_to_buffer;
            moves.commit_to_complete += more.commit_to_complete;
        }
        // Every lot but the initial one leaves the commit station; every lot leaves the fab. A
        // lot whose next step its tool takes stays at its port.
        assert_eq!(moves.commit_to_tool + moves.commit_to_buffer, released - 1);
        assert_eq!(moves.tool_to_complete + moves.buffer_to_complete, released);
        assert!(
            moves.tool_to_tool > 0 && moves.commit_to_buffer > 0 && moves.buffer_to_tool > 0,
            "{moves:?}"
        );
    }

    #[test]
    fn batches_go_in_and_out_port_by_port() {
        reports(&run_checked(
            dataset(30, 10 * MINUTE, true),
            Config::new(DAY),
            500,
        ));
    }

    #[test]
    fn every_setting_runs_safely() {
        let settings = [
            AmhsConfig {
                look_ahead: Some(0),
                ..AmhsConfig::default()
            },
            AmhsConfig {
                look_ahead: Some(1),
                dispatch: Dispatch::Bay,
                ..AmhsConfig::default()
            },
            AmhsConfig {
                vehicles: Some(1),
                hoist: 0,
                ..AmhsConfig::default()
            },
        ];
        for amhs in settings {
            let config = Config {
                amhs: Some(amhs),
                ..Config::new(DAY)
            };
            reports(&run_checked(dataset(20, 30 * MINUTE, false), config, 1_000));
        }
    }

    /// Five vehicles at 5 m/s on the loop with its diverging node n1 a plain node, as most of
    /// SMAT2022's are: a vehicle whose leader turns off there keeps its distance to the vehicles
    /// beyond the turn.
    #[test]
    fn fast_vehicles_keep_apart_past_turns() {
        let mut data = Arc::try_unwrap(dataset(40, 6 * MINUTE, false)).expect("one owner");
        let layout = data.layout.as_mut().expect("a layout");
        for node in &mut layout.nodes {
            if ["n1", "n8", "n9"].contains(&node.name.as_str()) {
                node.zone = None;
            }
        }
        for link in &mut layout.links {
            if ["r1", "s1"].contains(&link.name.as_str()) {
                link.zone = None;
            }
        }
        for (name, rail, offset) in [
            ("v2", "r5", 3_000.0),
            ("v3", "r0", 1_500.0),
            ("v4", "r2", 800.0),
        ] {
            let link = layout.links.iter().position(|link| link.name == rail);
            layout.vehicles.push(crate::layout::Vehicle {
                name: name.into(),
                kind: 0,
                link: link.expect("a rail"),
                offset,
            });
        }
        let fast = AmhsConfig {
            max_speed: Some(5_000.0),
            straight_speed: Some(5_000.0),
            curve_speed: Some(5_000.0),
            ..AmhsConfig::default()
        };
        let config = Config {
            amhs: Some(fast),
            ..Config::new(DAY)
        };
        reports(&run_checked(Arc::new(data), config, 500));
    }

    /// The page's SMAT2022 dataset.
    fn smat2022() -> Arc<Dataset> {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../www/data/smat2022.bin");
        let bytes = std::fs::read(path).unwrap_or_else(|error| panic!("{path}: {error}"));
        Arc::new(Dataset::from_bytes(&bytes).unwrap())
    }

    /// The data's speeds (every rail 1 m/s), the paper's Table 1 (straight 5, curves 1 m/s), and
    /// 5 m/s everywhere, where vehicles brake over metres; and no look-ahead, whose idle vehicles
    /// piled up in a bay with full neighbors until its loop locked (8.2 h).
    #[test]
    #[ignore = "slow: run with --release"]
    fn smat2022_runs_safely() {
        let other = AmhsConfig {
            vehicles: Some(300),
            dispatch: Dispatch::Bay,
            look_ahead: Some(1),
            ..AmhsConfig::default()
        };
        let no_look_ahead = AmhsConfig {
            look_ahead: Some(0),
            ..AmhsConfig::default()
        };
        let paper = AmhsConfig {
            max_speed: Some(5_000.0),
            acceleration: Some(2_000.0),
            deceleration: Some(3_500.0),
            straight_speed: Some(5_000.0),
            curve_speed: Some(1_000.0),
            ..AmhsConfig::default()
        };
        let fast = AmhsConfig {
            max_speed: Some(5_000.0),
            straight_speed: Some(5_000.0),
            curve_speed: Some(5_000.0),
            ..AmhsConfig::default()
        };
        for (amhs, hours) in [
            (AmhsConfig::default(), 6),
            (other, 3),
            (paper, 3),
            (fast, 3),
            (no_look_ahead, 10),
        ] {
            let config = Config {
                amhs: Some(amhs),
                ..Config::new(730 * DAY)
            };
            let mut simulation = Simulation::new(smat2022(), config).unwrap();
            let start = std::time::Instant::now();
            let mut now = 0;
            while now < hours * HOUR {
                now += 1_000;
                simulation
                    .run(Some(now))
                    .unwrap_or_else(|error| panic!("{error}"));
                let fab = simulation.engine.model();
                fab.logistics_ref()
                    .system
                    .check(now)
                    .unwrap_or_else(|error| panic!("{error}"));
                fab.check_logistics()
                    .unwrap_or_else(|error| panic!("at {now} ms: {error}"));
            }
            eprintln!(
                "{:?} checked in {:?}",
                simulation.progress(),
                start.elapsed()
            );
        }
    }

    #[test]
    #[ignore = "slow: run with --release"]
    fn smat2022_replay_size() {
        let (from, until) = (DAY, DAY + HOUR);
        let recording = Recording {
            replay: Some(ReplayWindow { from, until }),
            ..Recording::default()
        };
        let mut simulation =
            Simulation::with_recording(smat2022(), Config::new(730 * DAY), recording).unwrap();
        simulation.run(Some(until)).unwrap();
        let start = std::time::Instant::now();
        let replay = simulation.replay().unwrap();
        let built = start.elapsed();
        let size = replay.bytes().len();
        let start = std::time::Instant::now();
        let mut player = Player::new(smat2022(), replay).unwrap();
        let read = start.elapsed();
        let start = std::time::Instant::now();
        let mut frame = Frame::default();
        for second in 0..3_600 {
            player.frame_into((from + second * 1_000) as f64, &mut frame);
        }
        eprintln!(
            "an hour: {size} bytes, built in {built:?}, read in {read:?}, 3,600 frames in {:?}",
            start.elapsed()
        );
    }

    #[test]
    #[ignore = "slow: run with --release"]
    fn smat2022_speed() {
        let mut simulation = Simulation::new(smat2022(), Config::new(730 * DAY)).unwrap();
        for day in 1..=2 {
            let start = std::time::Instant::now();
            let before = simulation.engine.events_processed();
            simulation.run(Some(day * DAY)).unwrap();
            let fab = simulation.engine.model_mut();
            let report = fab.logistics_mut().system.report(day * DAY);
            eprintln!(
                "day {day}: {:?}, {} events in {:?}, {:?}",
                simulation.progress(),
                simulation.engine.events_processed() - before,
                start.elapsed(),
                report.moves
            );
        }
    }

    /// The fab as a replay's frame has it: each vehicle's front and rear points and activity, the
    /// FOUPs at ports and on vehicles (lot, place, index), the FOUPs in commit stations and in
    /// tools by port and tool, and each tool's state.
    #[allow(clippy::type_complexity)]
    fn replayed(
        frame: &Frame,
    ) -> (
        Vec<(f64, f64, f64, f64, Activity)>,
        Vec<(u64, FoupPlace, u32)>,
        Vec<(FoupPlace, u32, u32)>,
        Vec<ToolState>,
    ) {
        let vehicles = &frame.vehicles;
        let points = (0..vehicles.x.len())
            .map(|id| {
                let (x, y) = (vehicles.x[id], vehicles.y[id]);
                let (tail_x, tail_y) = (vehicles.tail_x[id], vehicles.tail_y[id]);
                (x, y, tail_x, tail_y, vehicles.activity[id])
            })
            .collect();
        let foups = &frame.foups;
        let mut held = Vec::new();
        let mut counts: std::collections::BTreeMap<(FoupPlace, u32), u32> = Default::default();
        for index in 0..foups.lot.len() {
            let (lot, place, at) = (foups.lot[index], foups.place[index], foups.index[index]);
            match place {
                FoupPlace::Port | FoupPlace::Vehicle => held.push((lot, place, at)),
                FoupPlace::Tool | FoupPlace::Commit => *counts.entry((place, at)).or_default() += 1,
                FoupPlace::Gone => {}
            }
        }
        held.sort();
        let counts = counts
            .into_iter()
            .map(|((place, at), count)| (place, at, count))
            .collect();
        (points, held, counts, frame.tools.clone())
    }

    /// The same as the AMHS status has it.
    #[allow(clippy::type_complexity)]
    fn live(
        amhs: &AmhsStatus,
    ) -> (
        Vec<(f64, f64, f64, f64, Activity)>,
        Vec<(u64, FoupPlace, u32)>,
        Vec<(FoupPlace, u32, u32)>,
        Vec<ToolState>,
    ) {
        let points = amhs
            .vehicles
            .iter()
            .map(|vehicle| {
                let (x, y) = (vehicle.x, vehicle.y);
                (x, y, vehicle.tail_x, vehicle.tail_y, vehicle.activity)
            })
            .collect();
        let mut held: Vec<(u64, FoupPlace, u32)> = amhs
            .foups
            .iter()
            .map(|foup| (foup.lot, FoupPlace::Port, foup.port as u32))
            .chain(amhs.vehicles.iter().filter_map(|vehicle| {
                vehicle
                    .lot
                    .map(|lot| (lot, FoupPlace::Vehicle, vehicle.id as u32))
            }))
            .collect();
        held.sort();
        let counts = amhs
            .committed
            .iter()
            .map(|count| (FoupPlace::Commit, count.at as u32, count.foups))
            .chain(
                amhs.inside
                    .iter()
                    .map(|count| (FoupPlace::Tool, count.at as u32, count.foups)),
            )
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        (points, held, counts, amhs.tools.clone())
    }

    #[test]
    fn a_replay_shows_the_run_as_it_was() {
        replays_as_it_was(Config::new(DAY), 30 * MINUTE, 3 * HOUR);
    }

    /// Ten days in, times are 8 times coarser f64s and the odometers have been rebased.
    #[test]
    fn a_late_replay_shows_the_run_as_it_was() {
        let (from, until) = (10 * DAY, 10 * DAY + HOUR);
        replays_as_it_was(Config::new(until), from, until);
    }

    /// A replay of `[from, until)` against the run at every 7,919th ms, played forward and sought
    /// backward; recording leaves the run unchanged.
    fn replays_as_it_was(config: Config, from: Time, until: Time) {
        let data = dataset(30, 20 * MINUTE, false);
        let recording = Recording {
            replay: Some(ReplayWindow { from, until }),
            ..Recording::default()
        };
        let mut recorded =
            Simulation::with_recording(Arc::clone(&data), config.clone(), recording).unwrap();
        recorded.run(Some(until)).unwrap();
        let replay = recorded.replay().unwrap();
        assert_eq!(replay.window(), ReplayWindow { from, until });
        assert_eq!(Replay::from_bytes(replay.bytes().to_vec()).unwrap(), replay);
        let mut player = Player::new(Arc::clone(&data), replay.clone()).unwrap();
        let mut plain = Simulation::new(Arc::clone(&data), config.clone()).unwrap();
        let mut seen = Vec::new();
        let mut at = from;
        while at < until {
            plain.run(Some(at)).unwrap();
            seen.push((at, live(&plain.amhs().unwrap())));
            at += 7_919;
        }
        // Forward as played, then backward as sought. Bodies keep their length along the rails:
        // a chord across the loop's right-angled corners is at least its 1/√2.
        let length = data.layout.as_ref().expect("a layout").vehicle_types[0].length;
        let order = seen.iter().chain(seen.iter().rev());
        for (at, (points, held, counts, tools)) in order {
            let frame = player.frame(*at as f64);
            let (replayed_points, replayed_held, replayed_counts, replayed_tools) =
                replayed(&frame);
            for (vehicle, (live, replayed)) in points.iter().zip(&replayed_points).enumerate() {
                // Within the AMHS's node tolerance (a vehicle passes a node 10⁻³ mm early or
                // late) and the player's drift (10⁻⁴ mm).
                let near = |a: f64, b: f64| (a - b).abs() < 1.1e-3;
                assert!(
                    near(live.0, replayed.0)
                        && near(live.1, replayed.1)
                        && near(live.2, replayed.2)
                        && near(live.3, replayed.3),
                    "vehicle {vehicle} at {at}: {live:?} ≠ {replayed:?}"
                );
                assert_eq!(live.4, replayed.4, "vehicle {vehicle} at {at}");
                let chord = (live.0 - live.2).hypot(live.1 - live.3);
                assert!(
                    (length / 2f64.sqrt() - 1e-6..=length + 1e-6).contains(&chord),
                    "vehicle {vehicle} at {at}: {chord} mm from front to rear"
                );
            }
            assert_eq!(points.len(), replayed_points.len());
            assert_eq!(held, &replayed_held, "FOUPs at {at}");
            assert_eq!(counts, &replayed_counts, "FOUP counts at {at}");
            assert_eq!(tools, &replayed_tools, "tools at {at}");
        }
        // Recording leaves the run unchanged.
        recorded.run(None).unwrap();
        plain.run(None).unwrap();
        assert_eq!(
            recorded.results().unwrap().digest(),
            plain.results().unwrap().digest()
        );
    }

    /// A replay taken before its window ends covers the window so far; its bytes cut short are
    /// refused, and with a byte changed refused or played without failing.
    #[test]
    fn a_replay_so_far_plays_and_damaged_ones_are_refused() {
        let data = dataset(30, 20 * MINUTE, false);
        let (from, until) = (30 * MINUTE, 3 * HOUR);
        let recording = Recording {
            replay: Some(ReplayWindow { from, until }),
            ..Recording::default()
        };
        let mut simulation =
            Simulation::with_recording(Arc::clone(&data), Config::new(DAY), recording).unwrap();
        simulation.run(Some(from - 1)).unwrap();
        assert!(simulation.replay().is_none());
        simulation.run(Some(HOUR)).unwrap();
        let so_far = simulation.replay().unwrap();
        assert_eq!(so_far.window(), ReplayWindow { from, until: HOUR });
        let bytes = so_far.into_bytes();
        let play = |bytes: Vec<u8>| -> Result<(), Error> {
            let mut player = Player::new(Arc::clone(&data), Replay::from_bytes(bytes)?)?;
            for at in [from, from + 1_234, HOUR, from] {
                player.frame(at as f64);
            }
            Ok(())
        };
        play(bytes.clone()).unwrap();
        for cut in 0..bytes.len() {
            assert!(play(bytes[..cut].to_vec()).is_err(), "cut at {cut}");
        }
        for index in 0..bytes.len() {
            for flip in [0x01, 0x80] {
                let mut damaged = bytes.clone();
                damaged[index] ^= flip;
                let _ = play(damaged);
            }
        }
    }

    #[test]
    fn pausing_leaves_the_run_unchanged() {
        let data = dataset(20, 30 * MINUTE, false);
        let mut straight = Simulation::new(Arc::clone(&data), Config::new(DAY)).unwrap();
        straight.run(Some(6 * HOUR)).unwrap();
        let mut stepped = Simulation::new(data, Config::new(DAY)).unwrap();
        for at in [1, 7_000, HOUR, HOUR + 1, 6 * HOUR] {
            stepped.run(Some(at)).unwrap();
        }
        assert_eq!(stepped.progress(), straight.progress());
        assert_eq!(stepped.lots(), straight.lots());
        assert_eq!(stepped.amhs(), straight.amhs());
        assert_eq!(straight.amhs().unwrap().vehicles.len(), 2);
    }
}
