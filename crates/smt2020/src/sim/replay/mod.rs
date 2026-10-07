//! The AMHS of a window for replaying it ([`Recording::replay`](super::Recording::replay)): a
//! compact record of the run ([`Replay`]) and its player ([`Player`]), which shows the fab at any
//! instant of the window: every vehicle within [`DRIFT`](motion::DRIFT) of where it was, every
//! acceleration change at its instant, every activity, FOUP move and tool state as it was.
//!
//! The record keeps what the run does not determine by itself (`motion`, `changes`) and codes it
//! with probabilities counted over the window (`coder`). Format (version 1): `SMTREPLY`, the
//! version (u32), the window, its counts and the kinematics (varints, f64s), the probability of
//! each context (u16s), then each vehicle's track and the change lists (each its byte length and
//! its coded bytes); integers in varints and numbers little-endian.

mod changes;
mod coder;
mod motion;

use std::collections::BTreeMap;
use std::sync::Arc;

use des_core::Time;
use serde::{Deserialize, Serialize};

use super::amhs::Activity;
use super::stats::LotKind;
use super::tool::ToolState;
use super::{Error, deserialize_time};
use crate::data::Dataset;
use crate::layout::{LinkId, PortKind};
use changes::{ChangeCoding, ChangeContexts, ChangeLists};
use coder::{Contexts, Counter, Encoder, width};
use motion::{Codec, MotionContexts, Track};

pub(crate) use changes::{ActivityChange, FoupMove, ToolChange};
pub(crate) use motion::{Kinematics, TrackWriter};

const MAGIC: &[u8; 8] = b"SMTREPLY";
const VERSION: u32 = 1;

/// A window from `from` up to `until` (ms).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplayWindow {
    #[serde(deserialize_with = "deserialize_time")]
    pub from: Time,
    #[serde(deserialize_with = "deserialize_time")]
    pub until: Time,
}

fn put_varint(out: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        out.push(value as u8 | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

fn corrupt() -> Error {
    Error("corrupt replay".into())
}

/// Reads bytes in order.
struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, at: 0 }
    }

    fn take(&mut self, count: usize) -> Result<&'a [u8], Error> {
        let end = self
            .at
            .checked_add(count)
            .filter(|&end| end <= self.bytes.len());
        let end = end.ok_or_else(corrupt)?;
        let taken = &self.bytes[self.at..end];
        self.at = end;
        Ok(taken)
    }

    fn u8(&mut self) -> Result<u8, Error> {
        Ok(self.take(1)?[0])
    }

    fn varint(&mut self) -> Result<u64, Error> {
        let mut value = 0u64;
        for shift in (0..64).step_by(7) {
            let byte = self.u8()?;
            value |= u64::from(byte & 0x7f) << shift;
            if byte < 0x80 {
                return Ok(value);
            }
        }
        Err(corrupt())
    }

    fn count(&mut self) -> Result<usize, Error> {
        usize::try_from(self.varint()?).map_err(|_| corrupt())
    }

    fn time(&mut self) -> Result<Time, Error> {
        Time::try_from(self.varint()?).map_err(|_| corrupt())
    }

    fn f64(&mut self) -> Result<f64, Error> {
        let bytes = self.take(8)?;
        Ok(f64::from_le_bytes(bytes.try_into().expect("8 bytes")))
    }

    /// Bytes with their length first: where they lie.
    fn part(&mut self) -> Result<std::ops::Range<usize>, Error> {
        let length = self.count()?;
        let start = self.at;
        self.take(length)?;
        Ok(start..self.at)
    }

    fn done(&self) -> bool {
        self.at == self.bytes.len()
    }
}

named_enum! {
    /// Where a FOUP is.
    pub enum FoupPlace {
        /// At a port: of a tool or a track buffer.
        Port = "port",
        /// On a vehicle.
        Vehicle = "vehicle",
        /// In a batch tool.
        Tool = "tool",
        /// In a commit station, picked up at its port.
        Commit = "commit",
        /// Out of the fab, through a complete station's port.
        Gone = "gone",
    }
}

/// A tool's states as known at `known`: from each time on, as of its jobs and outages then.
pub(crate) struct Timeline {
    pub known: Time,
    pub tool: usize,
    pub changes: Vec<(Time, ToolState)>,
}

/// The states the tools took in the window up to `until`: each timeline holds until the next of
/// its tool is known.
fn tool_changes(timelines: &[Timeline], tools: usize, from: Time, until: Time) -> Vec<ToolChange> {
    let mut next_known = vec![Time::MAX; tools];
    let mut ends = vec![until; timelines.len()];
    for (index, timeline) in timelines.iter().enumerate().rev() {
        ends[index] = next_known[timeline.tool].min(until);
        next_known[timeline.tool] = timeline.known;
    }
    let mut last: Vec<Option<ToolState>> = vec![None; tools];
    let mut changes = Vec::new();
    for (timeline, &end) in timelines.iter().zip(&ends) {
        for &(time, state) in &timeline.changes {
            if time < end && last[timeline.tool] != Some(state) {
                let (time, tool) = (time.max(from), timeline.tool);
                changes.push(ToolChange { time, tool, state });
                last[timeline.tool] = Some(state);
            }
        }
    }
    changes.sort_by_key(|change| (change.time, change.tool));
    changes
}

/// What fixes a window's coding: the window, its counts, the bits of a lot and the highest way
/// taken at a node, and the kinematics.
struct Shape {
    from: Time,
    until: Time,
    vehicles: usize,
    rails: usize,
    ports: usize,
    tools: usize,
    lot_bits: u32,
    widest: u32,
    kinematics: Kinematics,
}

impl Shape {
    /// The contexts of the tracks and of the changes, and their number.
    fn contexts(&self) -> (MotionContexts, ChangeContexts, usize) {
        let mut contexts = Contexts::default();
        let motion = MotionContexts::new(&mut contexts, &self.kinematics, self.rails, self.widest);
        let changes = ChangeContexts::new(
            &mut contexts,
            self.vehicles,
            self.ports,
            self.tools,
            self.lot_bits,
        );
        (motion, changes, contexts.count())
    }

    fn changes<'c>(&self, contexts: &'c ChangeContexts) -> ChangeCoding<'c> {
        ChangeCoding::new(contexts, self.vehicles, self.ports, self.tools, self.from)
    }

    fn write(&self, out: &mut Vec<u8>) {
        let counts = [self.vehicles, self.rails, self.ports, self.tools];
        for value in [self.from as u64, self.until as u64]
            .into_iter()
            .chain(counts.map(|count| count as u64))
        {
            put_varint(out, value);
        }
        put_varint(out, u64::from(self.lot_bits));
        put_varint(out, u64::from(self.widest));
        for list in [&self.kinematics.accelerations, &self.kinematics.speeds] {
            put_varint(out, list.len() as u64);
            for value in list {
                out.extend_from_slice(&value.to_le_bytes());
            }
        }
    }

    fn read(reader: &mut Reader) -> Result<Self, Error> {
        let (from, until) = (reader.time()?, reader.time()?);
        let (vehicles, rails, ports, tools) = (
            reader.count()?,
            reader.count()?,
            reader.count()?,
            reader.count()?,
        );
        let lot_bits = u32::try_from(reader.varint()?)
            .ok()
            .filter(|&bits| bits <= 64);
        let widest = u32::try_from(reader.varint()?)
            .ok()
            .filter(|&way| way <= u32::from(u16::MAX));
        let (Some(lot_bits), Some(widest)) = (lot_bits, widest) else {
            return Err(corrupt());
        };
        let mut lists = [Vec::new(), Vec::new()];
        for list in &mut lists {
            for _ in 0..reader.count()? {
                list.push(reader.f64()?);
            }
        }
        let [accelerations, speeds] = lists;
        if from > until
            || ![&accelerations, &speeds]
                .iter()
                .all(|list| list.iter().all(|value| value.is_finite()))
        {
            return Err(corrupt());
        }
        Ok(Self {
            from,
            until,
            vehicles,
            rails,
            ports,
            tools,
            lot_bits,
            widest,
            kinematics: Kinematics {
                accelerations,
                speeds,
            },
        })
    }
}

/// A window's record as the run left it: the vehicles' tracks and kinematics, their activities,
/// the FOUP moves, the tools' timelines and the layout's counts.
pub(crate) struct Recorded<'a> {
    pub from: Time,
    pub until: Time,
    pub kinematics: &'a Kinematics,
    pub tracks: Vec<TrackWriter>,
    pub activities: &'a [ActivityChange],
    pub foups: &'a [FoupMove],
    pub timelines: &'a [Timeline],
    pub rails: usize,
    pub ports: usize,
    pub tools: usize,
}

impl Recorded<'_> {
    /// The replay: every part coded once to count each context's bits, then with the
    /// probabilities counted.
    pub(crate) fn replay(&self) -> Replay {
        let lists = ChangeLists {
            activities: self.activities.to_vec(),
            foups: self.foups.to_vec(),
            tools: tool_changes(self.timelines, self.tools, self.from, self.until),
        };
        let shape = Shape {
            from: self.from,
            until: self.until,
            vehicles: self.tracks.len(),
            rails: self.rails,
            ports: self.ports,
            tools: self.tools,
            lot_bits: width(
                lists
                    .foups
                    .iter()
                    .map(|change| change.lot)
                    .max()
                    .unwrap_or(0),
            ),
            widest: self
                .tracks
                .iter()
                .map(TrackWriter::widest)
                .max()
                .unwrap_or(0),
            kinematics: self.kinematics.clone(),
        };
        let (motion, changes, count) = shape.contexts();
        let coding = shape.changes(&changes);
        let mut counter = Counter::new(count);
        for track in &self.tracks {
            track.code(&mut counter, &motion);
        }
        coding.code(&mut counter, &lists);
        let probabilities = counter.probabilities();
        let mut bytes = MAGIC.to_vec();
        bytes.extend_from_slice(&VERSION.to_le_bytes());
        shape.write(&mut bytes);
        put_varint(&mut bytes, probabilities.len() as u64);
        for probability in &probabilities {
            bytes.extend_from_slice(&probability.to_le_bytes());
        }
        let mut put_part = |part: Vec<u8>| {
            put_varint(&mut bytes, part.len() as u64);
            bytes.extend_from_slice(&part);
        };
        for track in &self.tracks {
            let mut encoder = Encoder::new(&probabilities);
            track.code(&mut encoder, &motion);
            put_part(encoder.finish());
        }
        let mut encoder = Encoder::new(&probabilities);
        coding.code(&mut encoder, &lists);
        put_part(encoder.finish());
        Replay {
            bytes,
            from: self.from,
            until: self.until,
        }
    }
}

/// A recorded AMHS window in its compact form ([`Simulation::replay`](super::Simulation::replay));
/// a [`Player`] plays it on its dataset.
#[derive(Clone, Debug, PartialEq)]
pub struct Replay {
    bytes: Vec<u8>,
    from: Time,
    until: Time,
}

impl Replay {
    /// A replay from its [`bytes`](Self::bytes).
    pub fn from_bytes(bytes: Vec<u8>) -> Result<Self, Error> {
        let mut reader = Reader::new(&bytes);
        if reader.take(MAGIC.len()).ok() != Some(MAGIC.as_slice()) {
            return Err(Error("not a replay".into()));
        }
        let version = u32::from_le_bytes(reader.take(4)?.try_into().expect("4 bytes"));
        if version != VERSION {
            return Err(Error(format!(
                "replay format {version}; this build reads {VERSION}"
            )));
        }
        let (from, until) = (reader.time()?, reader.time()?);
        Ok(Self { bytes, from, until })
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }

    /// The window: from `from` up to `until` (ms).
    pub fn window(&self) -> ReplayWindow {
        ReplayWindow {
            from: self.from,
            until: self.until,
        }
    }
}

/// The fab at an instant of a replay.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Frame {
    pub time: f64,
    pub vehicles: VehicleFrames,
    pub foups: FoupFrames,
    /// Every tool's state, by id.
    pub tools: Vec<ToolState>,
    /// Deliveries picked up and set down within the window so far, those from a tool's port to
    /// a tool's, and their time on vehicles (ms).
    pub delivered: u64,
    pub tool_to_tool: u64,
    pub carried: f64,
}

/// Every vehicle, by id: its front's point (mm) and heading (rad), its speed (m/s) and what it
/// does.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct VehicleFrames {
    pub x: Vec<f64>,
    pub y: Vec<f64>,
    pub heading: Vec<f64>,
    pub speed: Vec<f64>,
    pub activity: Vec<Activity>,
}

/// Every FOUP in the fab, by lot ([`LotStatus::id`](super::LotStatus::id)): the lot's kind and
/// where the FOUP is (the port, vehicle or tool, or the commit station's port).
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct FoupFrames {
    pub lot: Vec<u64>,
    pub kind: Vec<LotKind>,
    pub place: Vec<FoupPlace>,
    pub index: Vec<u32>,
}

/// Where a FOUP is as played: its kind and place, and its pickup within the window (when, and
/// whether from a tool's port).
#[derive(Clone, Copy)]
struct FoupState {
    kind: LotKind,
    place: FoupPlace,
    index: usize,
    boarded: Option<(Time, bool)>,
}

/// Plays a [`Replay`] on its dataset's layout: the fab at any instant of the window. Vehicles move
/// on from the instant last asked for (from a checkpoint when asked for an earlier one), FOUPs and
/// tools step through their changes (from the window's start when asked for an earlier instant).
pub struct Player {
    data: Arc<Dataset>,
    bytes: Vec<u8>,
    from: Time,
    until: Time,
    codec: Codec,
    /// Per node: the rails out of it, in rail order.
    out: Vec<Vec<LinkId>>,
    tracks: Vec<Track>,
    /// Per vehicle: its activities from each time on.
    activities: Vec<Vec<(Time, Activity)>>,
    foup_moves: Vec<FoupMove>,
    tool_changes: Vec<ToolChange>,
    places: BTreeMap<u64, FoupState>,
    states: Vec<ToolState>,
    next_foup: usize,
    next_tool: usize,
    at: f64,
    delivered: u64,
    tool_to_tool: u64,
    carried: f64,
}

impl Player {
    /// The player of `replay` on `data`, the dataset it was recorded on; the replay read through
    /// and checked.
    pub fn new(data: Arc<Dataset>, replay: Replay) -> Result<Self, Error> {
        let layout = data
            .layout
            .as_ref()
            .ok_or_else(|| Error("the dataset has no AMHS layout to replay on".into()))?;
        let tools: usize = data
            .tool_groups
            .iter()
            .map(|group| group.tools as usize)
            .sum();
        let bytes = replay.bytes;
        let mut reader = Reader::new(&bytes);
        reader.take(MAGIC.len() + 4)?;
        let shape = Shape::read(&mut reader)?;
        if (shape.rails, shape.ports, shape.tools)
            != (layout.links.len(), layout.ports.len(), tools)
            || shape.vehicles > layout.vehicles.len()
        {
            return Err(Error("the replay was recorded on another dataset".into()));
        }
        let (motion, changes, count) = shape.contexts();
        if reader.count()? != count {
            return Err(corrupt());
        }
        let mut probabilities = Vec::with_capacity(count);
        for _ in 0..count {
            let probability = u16::from_le_bytes(reader.take(2)?.try_into().expect("2 bytes"));
            if !(1..1 << 12).contains(&probability) {
                return Err(corrupt());
            }
            probabilities.push(probability);
        }
        let codec = Codec {
            kinematics: shape.kinematics.clone(),
            contexts: motion,
            probabilities,
        };
        let mut out = vec![Vec::new(); layout.nodes.len()];
        for (rail, link) in layout.links.iter().enumerate() {
            out[link.from].push(rail);
        }
        let (from, until) = (shape.from as f64, shape.until as f64);
        let mut tracks = Vec::with_capacity(shape.vehicles);
        for _ in 0..shape.vehicles {
            let part = reader.part()?;
            tracks.push(Track::read(
                &bytes, part, &codec, from, until, layout, &out,
            )?);
        }
        let part = reader.part()?;
        let lists = shape
            .changes(&changes)
            .read(&bytes[part], &codec.probabilities)?;
        if !reader.done() {
            return Err(corrupt());
        }
        let mut activities = vec![Vec::new(); shape.vehicles];
        for change in &lists.activities {
            activities[change.vehicle].push((change.time, change.activity));
        }
        let within = |time: Time| (shape.from..=shape.until).contains(&time);
        let started = activities
            .iter()
            .all(|each| each.first().is_some_and(|&(time, _)| time == shape.from));
        let timely = lists
            .activities
            .iter()
            .map(|change| change.time)
            .chain(lists.foups.iter().map(|change| change.time))
            .chain(lists.tools.iter().map(|change| change.time))
            .all(within);
        if !(started && timely) {
            return Err(corrupt());
        }
        Ok(Self {
            data: Arc::clone(&data),
            bytes,
            from: shape.from,
            until: shape.until,
            codec,
            out,
            tracks,
            activities,
            foup_moves: lists.foups,
            tool_changes: lists.tools,
            places: BTreeMap::new(),
            states: vec![ToolState::Idle; tools],
            next_foup: 0,
            next_tool: 0,
            at: f64::NEG_INFINITY,
            delivered: 0,
            tool_to_tool: 0,
            carried: 0.0,
        })
    }

    /// The window: from `from` up to `until` (ms).
    pub fn window(&self) -> ReplayWindow {
        ReplayWindow {
            from: self.from,
            until: self.until,
        }
    }

    /// The FOUPs and tools as before the window's start.
    fn restart(&mut self) {
        self.places.clear();
        self.states.fill(ToolState::Idle);
        self.next_foup = 0;
        self.next_tool = 0;
        self.at = f64::NEG_INFINITY;
        self.delivered = 0;
        self.tool_to_tool = 0;
        self.carried = 0.0;
    }

    /// The fab at `time`, within the window.
    pub fn frame(&mut self, time: f64) -> Frame {
        let mut frame = Frame::default();
        self.frame_into(time, &mut frame);
        frame
    }

    /// The fab at `time` into `frame`, whose buffers it reuses.
    pub fn frame_into(&mut self, time: f64, frame: &mut Frame) {
        let time = time.clamp(self.from as f64, self.until as f64);
        self.seek(time);
        let layout = self.data.layout.as_ref().expect("a layout");
        frame.time = time;
        let vehicles = &mut frame.vehicles;
        for column in [
            &mut vehicles.x,
            &mut vehicles.y,
            &mut vehicles.heading,
            &mut vehicles.speed,
        ] {
            column.clear();
        }
        vehicles.activity.clear();
        for (track, activities) in self.tracks.iter_mut().zip(&self.activities) {
            let (rail, offset, speed) = track.at(time, &self.bytes, &self.codec, layout, &self.out);
            let (x, y, heading) = layout.point(rail, offset);
            vehicles.x.push(x);
            vehicles.y.push(y);
            vehicles.heading.push(heading);
            vehicles.speed.push(speed);
            // Every vehicle's first activity is at the window's start.
            let latest = activities.partition_point(|&(each, _)| each as f64 <= time);
            vehicles.activity.push(activities[latest.max(1) - 1].1);
        }
        let foups = &mut frame.foups;
        foups.lot.clear();
        foups.kind.clear();
        foups.place.clear();
        foups.index.clear();
        for (&lot, state) in &self.places {
            foups.lot.push(lot);
            foups.kind.push(state.kind);
            foups.place.push(state.place);
            foups.index.push(state.index as u32);
        }
        frame.tools.clone_from(&self.states);
        frame.delivered = self.delivered;
        frame.tool_to_tool = self.tool_to_tool;
        frame.carried = self.carried;
    }

    /// The FOUPs and tools as at `time`.
    fn seek(&mut self, time: f64) {
        if time < self.at {
            self.restart();
        }
        let layout = self.data.layout.as_ref().expect("a layout");
        let tool_port = |port: usize| matches!(layout.ports[port].kind, PortKind::Tool(_));
        while let Some(&change) = self.foup_moves.get(self.next_foup) {
            if change.time as f64 > time {
                break;
            }
            self.next_foup += 1;
            let mut state = FoupState {
                kind: change.kind,
                place: change.place,
                index: change.index,
                boarded: None,
            };
            match self.places.get(&change.lot).copied() {
                Some(before) if before.place == FoupPlace::Vehicle => {
                    if state.place == FoupPlace::Vehicle {
                        state.boarded = before.boarded;
                    } else if let Some((boarded, from_tool)) = before.boarded {
                        self.delivered += 1;
                        self.carried += (change.time - boarded) as f64;
                        if from_tool && state.place == FoupPlace::Port && tool_port(state.index) {
                            self.tool_to_tool += 1;
                        }
                    }
                }
                Some(before) if state.place == FoupPlace::Vehicle => {
                    let from_tool = before.place == FoupPlace::Port && tool_port(before.index);
                    state.boarded = Some((change.time, from_tool));
                }
                _ => {}
            }
            if state.place == FoupPlace::Gone {
                self.places.remove(&change.lot);
            } else {
                self.places.insert(change.lot, state);
            }
        }
        while let Some(&change) = self.tool_changes.get(self.next_tool) {
            if change.time as f64 > time {
                break;
            }
            self.next_tool += 1;
            self.states[change.tool] = change.state;
        }
        self.at = time;
    }
}
