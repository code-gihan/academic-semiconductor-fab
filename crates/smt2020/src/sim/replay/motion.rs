//! A vehicle's motion as a replay keeps it. A vehicle drives at constant accelerations from a
//! short list (its acceleration, its deceleration, none), so its motion is the sequence of its
//! acceleration changes, each with its instant; position and speed follow by integration
//! ([`advance`]). An instant is kept by what makes it: a whole ms (the event that replanned the
//! vehicle), a speed reached (a speed limit, or rest: the instant follows from the speed, up to a
//! few f64 steps), or else its f64 steps after the last change. A breakpoint that keeps the
//! acceleration is left out where the player, driving on, stays within [`DRIFT`] of the vehicle
//! until the next; position and speed are given where the player would drift further. The rails
//! follow from the network and the ways taken at diverging nodes.

use std::ops::Range;

use super::coder::{
    Coder, Contexts, ReadState, Reading, gamma, raw, tree, unzigzag, width, zigzag,
};
use super::{Reader, corrupt, put_varint};
use crate::layout::{Layout, LinkId};
use crate::sim::Error;
use crate::sim::amhs::advance;

/// How far the player may be off a vehicle's position (mm) and speed (mm/ms) at any instant.
pub(super) const DRIFT: f64 = 1e-4;
pub(super) const SPEED_DRIFT: f64 = 1e-7;
/// What makes a change's instant: whole ms after the last change's instant rounded up; the
/// speed reached, give or take f64 steps there; f64 steps after the last change; itself.
const WHOLE: u8 = 0;
const SPEED: u8 = 1;
const STEPS: u8 = 2;
const EXACT: u8 = 3;
/// Unary contexts of the bit lengths of whole ms, speed residuals, steps, drifts and counts.
const WHOLE_LENGTHS: usize = 24;
const RESIDUAL_LENGTHS: usize = 16;
const STEP_LENGTHS: usize = 64;
const DRIFT_LENGTHS: usize = 64;
pub(super) const COUNT_LENGTHS: usize = 40;
/// Records between the player's checkpoints of a track.
const CHECKPOINT: u64 = 128;
/// The most ways and changes a track holds: far beyond any window's, a bound on what damaged
/// bytes may claim.
const MAX_WAYS: u64 = 1 << 24;
const MAX_CHANGES: u64 = 1 << 26;
/// A front this far past its rail's end is on the next rail (mm).
const PAST_END: f64 = 1e-6;

/// The gap between neighbouring f64 values at `x` (≥ 1): later instants are whole multiples of
/// it.
fn spacing(x: f64) -> Option<f64> {
    (x >= 1.0 && x.is_finite()).then(|| f64::from_bits(((x.to_bits() >> 52) - 52) << 52))
}

/// f64 values in order as integers, neighbours 1 apart.
fn ordered(x: f64) -> i64 {
    let bits = x.to_bits() as i64;
    if bits < 0 { bits ^ i64::MAX } else { bits }
}

/// The f64 `steps` steps from `x`.
fn stepped(x: f64, steps: i64) -> f64 {
    let bits = ordered(x).wrapping_add(steps);
    f64::from_bits((if bits < 0 { bits ^ i64::MAX } else { bits }) as u64)
}

/// Contexts of a binary tree of `depth` levels.
pub(super) fn nodes(depth: u32) -> usize {
    (1 << depth) - 1
}

/// The accelerations the vehicles drive at (mm/ms²) and the speeds they come to (mm/ms: rest and
/// the speed limits), window-wide.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Kinematics {
    pub accelerations: Vec<f64>,
    pub speeds: Vec<f64>,
}

impl Kinematics {
    /// The symbol of acceleration `a`: its index, or the escape (one past the list) for one given
    /// as it is.
    fn symbol(&self, a: f64) -> u32 {
        let index = self
            .accelerations
            .iter()
            .position(|each| each.to_bits() == a.to_bits());
        index.unwrap_or(self.accelerations.len()) as u32
    }

    fn escape(&self) -> u32 {
        self.accelerations.len() as u32
    }
}

/// The contexts of the vehicles' tracks.
pub(super) struct MotionContexts {
    /// Acceleration symbols (the escape last), levels of their tree, of the speeds' and of the
    /// ways' trees, bits of a rail.
    symbols: u32,
    depth: u32,
    speed_depth: u32,
    way_depth: u32,
    rail_bits: u32,
    /// Per previous symbol (the last row for a track's start): the next symbol's tree.
    acceleration: usize,
    /// Per class (previous and next symbol): the instant's kind, its whole ms, the speed reached,
    /// whether position and speed are given.
    cause: usize,
    whole: usize,
    speed: usize,
    reset: usize,
    residual: usize,
    steps: usize,
    drift: usize,
    rest: usize,
    counts: usize,
    way: usize,
}

impl MotionContexts {
    /// `widest` is the highest way taken at a diverging node.
    pub(super) fn new(
        contexts: &mut Contexts,
        kinematics: &Kinematics,
        rails: usize,
        widest: u32,
    ) -> Self {
        let symbols = kinematics.accelerations.len() as u32 + 1;
        let depth = width(u64::from(symbols - 1));
        let speed_depth = width(kinematics.speeds.len().saturating_sub(1) as u64);
        let way_depth = width(u64::from(widest));
        let classes = (symbols * symbols) as usize;
        Self {
            symbols,
            depth,
            speed_depth,
            way_depth,
            rail_bits: width(rails.saturating_sub(1) as u64),
            acceleration: contexts.take((symbols as usize + 1) * nodes(depth)),
            cause: contexts.take(classes * 3),
            whole: contexts.take(classes * WHOLE_LENGTHS),
            speed: contexts.take(classes * nodes(speed_depth)),
            reset: contexts.take(classes),
            residual: contexts.take(RESIDUAL_LENGTHS),
            steps: contexts.take(STEP_LENGTHS),
            drift: contexts.take(2 * DRIFT_LENGTHS),
            rest: contexts.take(1),
            counts: contexts.take(2 * COUNT_LENGTHS),
            way: contexts.take(nodes(way_depth)),
        }
    }

    fn count(&self, coder: &mut impl Coder, which: usize, count: &mut u64) {
        gamma(
            coder,
            self.counts + which * COUNT_LENGTHS,
            COUNT_LENGTHS,
            count,
        );
    }

    fn way(&self, coder: &mut impl Coder, way: &mut u32) {
        tree(coder, self.way, self.way_depth, way);
    }
}

/// The motion from `t` on: at `s` with speed `v` and acceleration `a` (its `symbol`).
#[derive(Clone, Copy, Debug)]
struct Segment {
    t: f64,
    s: f64,
    v: f64,
    a: f64,
    symbol: u32,
}

impl Segment {
    /// Position and speed at `time`.
    fn at(&self, time: f64) -> (f64, f64) {
        advance(self.s, self.v, self.a, time - self.t)
    }

    /// The instant of `record`'s change, not before this segment's start; none if it has none.
    fn instant(&self, record: &Record, kinematics: &Kinematics) -> Option<f64> {
        let t = match record.cause {
            WHOLE => self.t.ceil() + record.value as f64,
            SPEED => {
                let target = *kinematics.speeds.get(record.speed as usize)?;
                let reach = self.t + (target - self.v) / self.a;
                reach + unzigzag(record.value) as f64 * spacing(reach)?
            }
            STEPS => self.t + record.value as f64 * spacing(self.t)?,
            _ => f64::from_bits(record.value),
        };
        (t.is_finite() && t >= self.t).then_some(t)
    }

    /// The segment `record` begins; none if it is no change of this one.
    fn follow(&self, record: &Record, kinematics: &Kinematics) -> Option<Segment> {
        let t = self.instant(record, kinematics)?;
        let (mut s, mut v) = self.at(t);
        if record.cause == SPEED {
            v = kinematics.speeds[record.speed as usize];
        }
        if record.reset {
            s = stepped(s, unzigzag(record.ds));
            v = stepped(v, unzigzag(record.dv));
        }
        let a = match kinematics.accelerations.get(record.symbol as usize) {
            Some(&a) => a,
            None if record.symbol == kinematics.escape() => record.given,
            None => return None,
        };
        let symbol = record.symbol;
        [s, v, a]
            .iter()
            .all(|value| value.is_finite())
            .then_some(Segment { t, s, v, a, symbol })
    }

    /// How the instant `t` of a change after this segment's start is kept, the vehicle's speed
    /// then being `speed`: the shortest exact code.
    fn code_instant(&self, kinematics: &Kinematics, t: f64, speed: f64) -> (u8, u32, u64) {
        let mut best = (EXACT, 0, t.to_bits());
        let mut cost = u64::BITS;
        let mut consider = |code: (u8, u32, u64), value: u64| {
            if width(value) < cost {
                cost = width(value);
                best = code;
            }
        };
        let ceiling = self.t.ceil();
        if t.fract() == 0.0 && t >= ceiling {
            let value = (t - ceiling) as u64;
            consider((WHOLE, 0, value), value);
        }
        for (index, &target) in kinematics.speeds.iter().enumerate() {
            if self.a == 0.0 || (target - speed).abs() > SPEED_DRIFT {
                continue;
            }
            let reach = self.t + (target - self.v) / self.a;
            if let Some(step) = spacing(reach) {
                let steps = (t - reach) / step;
                if steps.fract() == 0.0 && steps.abs() < 1e18 && reach + steps * step == t {
                    let value = zigzag(steps as i64);
                    consider((SPEED, index as u32, value), value);
                }
            }
        }
        if let Some(step) = spacing(self.t) {
            let steps = (t - self.t) / step;
            if steps.fract() == 0.0 && steps < 1e18 && self.t + steps * step == t {
                consider((STEPS, 0, steps as u64), steps as u64);
            }
        }
        best
    }
}

/// A change of a vehicle's acceleration: the new one (its symbol, or itself after the escape),
/// what makes its instant (with the speed reached), and the position and speed if given (their
/// f64 steps from the player's, zigzagged).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Record {
    symbol: u32,
    given: f64,
    cause: u8,
    speed: u32,
    value: u64,
    reset: bool,
    ds: u64,
    dv: u64,
}

impl Record {
    /// Codes the record, the change after one to the acceleration of symbol `previous`.
    fn code(&mut self, coder: &mut impl Coder, contexts: &MotionContexts, previous: u32) {
        let c = contexts;
        let row = c.acceleration + previous as usize * nodes(c.depth);
        tree(coder, row, c.depth, &mut self.symbol);
        if self.symbol == c.symbols - 1 {
            raw(coder, &mut self.given);
        }
        // The symbol may be past the escape in damaged bytes: the class stays in range.
        let class =
            (previous.min(c.symbols - 1) * c.symbols + self.symbol.min(c.symbols - 1)) as usize;
        let kinds = c.cause + 3 * class;
        let mut whole = self.cause == WHOLE;
        coder.bit(kinds, &mut whole);
        self.cause = if whole {
            WHOLE
        } else {
            let mut speed = self.cause == SPEED;
            coder.bit(kinds + 1, &mut speed);
            if speed {
                SPEED
            } else {
                let mut steps = self.cause == STEPS;
                coder.bit(kinds + 2, &mut steps);
                if steps { STEPS } else { EXACT }
            }
        };
        match self.cause {
            WHOLE => gamma(
                coder,
                c.whole + class * WHOLE_LENGTHS,
                WHOLE_LENGTHS,
                &mut self.value,
            ),
            SPEED => {
                let speeds = c.speed + class * nodes(c.speed_depth);
                tree(coder, speeds, c.speed_depth, &mut self.speed);
                gamma(coder, c.residual, RESIDUAL_LENGTHS, &mut self.value);
            }
            STEPS => gamma(coder, c.steps, STEP_LENGTHS, &mut self.value),
            _ => coder.bits(64, &mut self.value),
        }
        coder.bit(c.reset + class, &mut self.reset);
        if self.reset {
            gamma(coder, c.drift, DRIFT_LENGTHS, &mut self.ds);
            gamma(coder, c.drift + DRIFT_LENGTHS, DRIFT_LENGTHS, &mut self.dv);
        }
    }

    /// The record in a recording's bytes; `escape` is the symbol of an acceleration given.
    fn put(&self, out: &mut Vec<u8>, escape: u32) {
        put_varint(out, u64::from(self.symbol));
        out.push(self.cause | u8::from(self.reset) << 2);
        if self.cause == SPEED {
            put_varint(out, u64::from(self.speed));
        }
        put_varint(out, self.value);
        if self.reset {
            put_varint(out, self.ds);
            put_varint(out, self.dv);
        }
        if self.symbol == escape {
            out.extend_from_slice(&self.given.to_le_bytes());
        }
    }

    fn take(reader: &mut Reader, escape: u32) -> Result<Self, Error> {
        let symbol = u32::try_from(reader.varint()?).map_err(|_| corrupt())?;
        let tag = reader.u8()?;
        let (cause, reset) = (tag & 3, tag & 4 != 0);
        let speed = match cause {
            SPEED => u32::try_from(reader.varint()?).map_err(|_| corrupt())?,
            _ => 0,
        };
        let value = reader.varint()?;
        let (ds, dv) = match reset {
            true => (reader.varint()?, reader.varint()?),
            false => (0, 0),
        };
        let given = if symbol == escape { reader.f64()? } else { 0.0 };
        Ok(Self {
            symbol,
            given,
            cause,
            speed,
            value,
            reset,
            ds,
            dv,
        })
    }
}

/// A track's start: the acceleration (its symbol, or itself after the escape), position on its
/// first rail (from the rail's start) and speed.
#[derive(Clone, Copy, Debug, Default)]
struct Start {
    symbol: u32,
    given: f64,
    s: f64,
    v: f64,
    rail: u64,
}

impl Start {
    fn code(&mut self, coder: &mut impl Coder, contexts: &MotionContexts) {
        let c = contexts;
        tree(
            coder,
            c.acceleration + c.symbols as usize * nodes(c.depth),
            c.depth,
            &mut self.symbol,
        );
        if self.symbol == c.symbols - 1 {
            raw(coder, &mut self.given);
        }
        let mut rest = self.v.to_bits() == 0;
        coder.bit(c.rest, &mut rest);
        if rest {
            self.v = 0.0;
        } else {
            raw(coder, &mut self.v);
        }
        raw(coder, &mut self.s);
        coder.bits(c.rail_bits, &mut self.rail);
    }
}

/// A vehicle's breakpoint as the AMHS drove it.
#[derive(Clone, Copy, Debug)]
struct Breakpoint {
    t: f64,
    s: f64,
    v: f64,
    a: f64,
}

/// A vehicle's track as it is recorded.
#[derive(Clone)]
pub(crate) struct TrackWriter {
    start: Segment,
    rail: LinkId,
    /// The changes and the ways taken, in a recording's bytes.
    records: Vec<u8>,
    count: u64,
    ways: Vec<u8>,
    way_count: u64,
    widest: u32,
    /// The player's segment, and the last breakpoint: written or left out once the next shows
    /// how the player fares.
    player: Segment,
    pending: Option<Breakpoint>,
}

impl TrackWriter {
    /// A vehicle at the window's start `t`: at `s` on rail `rail` (positions from that rail's
    /// start), with speed `v` and acceleration `a`.
    pub(crate) fn new(
        kinematics: &Kinematics,
        t: f64,
        s: f64,
        v: f64,
        a: f64,
        rail: LinkId,
    ) -> Self {
        let start = Segment {
            t,
            s,
            v,
            a,
            symbol: kinematics.symbol(a),
        };
        Self {
            start,
            rail,
            records: Vec::new(),
            count: 0,
            ways: Vec::new(),
            way_count: 0,
            widest: 0,
            player: start,
            pending: None,
        }
    }

    /// From `t` on the vehicle drives from `s` at `v` with acceleration `a`.
    pub(crate) fn breakpoint(&mut self, kinematics: &Kinematics, t: f64, s: f64, v: f64, a: f64) {
        if let Some(last) = self.pending.replace(Breakpoint { t, s, v, a }) {
            self.settle(kinematics, last, t);
        }
    }

    /// The window ends at `until`.
    pub(crate) fn finish(&mut self, kinematics: &Kinematics, until: f64) {
        if let Some(last) = self.pending.take() {
            self.settle(kinematics, last, until);
        }
    }

    /// At a diverging node the vehicle took the `way`-th rail out of it.
    pub(crate) fn choice(&mut self, way: usize) {
        put_varint(&mut self.ways, way as u64);
        self.way_count += 1;
        self.widest = self.widest.max(way as u32);
    }

    /// The highest way taken.
    pub(super) fn widest(&self) -> u32 {
        self.widest
    }

    /// Records `breakpoint`, in force until `end`, unless the player can do without it.
    fn settle(&mut self, kinematics: &Kinematics, breakpoint: Breakpoint, end: f64) {
        let player = self.player;
        let near = |s: f64, v: f64| {
            (s - breakpoint.s).abs() <= DRIFT && (v - breakpoint.v).abs() <= SPEED_DRIFT
        };
        // At the same acceleration the player's error is linear in time: within the bound at both
        // ends of the phase, within it throughout.
        let stays_near = |s: f64, v: f64| {
            let tau = end - breakpoint.t;
            let player = advance(s, v, breakpoint.a, tau).0;
            (player - advance(breakpoint.s, breakpoint.v, breakpoint.a, tau).0).abs() <= DRIFT
        };
        let (s, v) = player.at(breakpoint.t);
        if breakpoint.a.to_bits() == player.a.to_bits() && near(s, v) && stays_near(s, v) {
            return;
        }
        let (cause, speed, value) = player.code_instant(kinematics, breakpoint.t, breakpoint.v);
        let mut record = Record {
            symbol: kinematics.symbol(breakpoint.a),
            given: breakpoint.a,
            cause,
            speed,
            value,
            ..Record::default()
        };
        let next = player.follow(&record, kinematics).expect("a later change");
        if !(near(next.s, next.v) && stays_near(next.s, next.v)) {
            record.reset = true;
            record.ds = zigzag(ordered(breakpoint.s).wrapping_sub(ordered(next.s)));
            record.dv = zigzag(ordered(breakpoint.v).wrapping_sub(ordered(next.v)));
        }
        record.put(&mut self.records, kinematics.escape());
        self.count += 1;
        self.player = player.follow(&record, kinematics).expect("a later change");
    }

    /// Codes the track: the ways taken, the start and the changes.
    pub(super) fn code(&self, coder: &mut impl Coder, contexts: &MotionContexts) {
        debug_assert!(self.way_count <= MAX_WAYS && self.count <= MAX_CHANGES);
        let escape = contexts.symbols - 1;
        let mut count = self.way_count;
        contexts.count(coder, 0, &mut count);
        let mut reader = Reader::new(&self.ways);
        for _ in 0..self.way_count {
            let mut way = reader.varint().expect("a way") as u32;
            contexts.way(coder, &mut way);
        }
        let mut start = Start {
            symbol: self.start.symbol,
            given: self.start.a,
            s: self.start.s,
            v: self.start.v,
            rail: self.rail as u64,
        };
        start.code(coder, contexts);
        let mut count = self.count;
        contexts.count(coder, 1, &mut count);
        let mut reader = Reader::new(&self.records);
        let mut previous = self.start.symbol;
        for _ in 0..self.count {
            let mut record = Record::take(&mut reader, escape).expect("a record");
            record.code(coder, contexts, previous);
            previous = record.symbol;
        }
    }
}

/// What a track's reading needs: the window's kinematics, contexts and probabilities.
pub(super) struct Codec {
    pub kinematics: Kinematics,
    pub contexts: MotionContexts,
    pub probabilities: Vec<u16>,
}

/// Where a track's reading stands: the segment in force, the next change read ahead (with its
/// instant), and the rail the front is on as far as looked.
#[derive(Clone, Copy)]
struct Cursor {
    read: ReadState,
    left: u64,
    segment: Segment,
    next: Option<(Record, f64)>,
    rail: LinkId,
    /// The position of the rail's start, and the ways taken so far.
    start: f64,
    way: usize,
    /// The latest instant asked for: an earlier one goes back to a checkpoint.
    seen: f64,
}

impl Cursor {
    /// Reads the change after the segment in force, if one is left.
    fn read_next(&mut self, bytes: &[u8], codec: &Codec) -> Result<(), Error> {
        self.next = None;
        if self.left == 0 {
            return Ok(());
        }
        let mut reading = Reading::resume(bytes, &codec.probabilities, self.read);
        let mut record = Record::default();
        record.code(&mut reading, &codec.contexts, self.segment.symbol);
        let t = self.segment.instant(&record, &codec.kinematics);
        let t = t.filter(|_| !reading.damaged()).ok_or_else(corrupt)?;
        self.read = reading.state();
        self.left -= 1;
        self.next = Some((record, t));
        Ok(())
    }

    /// Makes the next change and reads the one after.
    fn step(&mut self, bytes: &[u8], codec: &Codec) -> Result<(), Error> {
        let (record, _) = self.next.take().expect("a change to make");
        self.segment = self
            .segment
            .follow(&record, &codec.kinematics)
            .ok_or_else(corrupt)?;
        self.read_next(bytes, codec)
    }

    /// Moves the rail on to the one the front is on at `s`: past each rail's end onto the next,
    /// the way taken at diverging nodes (staying at the end where none is left). False on a way
    /// that is not there.
    fn advance_rails(
        &mut self,
        s: f64,
        layout: &Layout,
        out: &[Vec<LinkId>],
        ways: &[u16],
    ) -> bool {
        loop {
            let length = layout.links[self.rail].length;
            if s <= self.start + length + PAST_END {
                return true;
            }
            let outs = &out[layout.links[self.rail].to];
            let next = match outs.len() {
                1 => outs[0],
                _ => match ways.get(self.way) {
                    Some(&way) if usize::from(way) < outs.len() => {
                        self.way += 1;
                        outs[usize::from(way)]
                    }
                    Some(_) => return false,
                    None => return true,
                },
            };
            self.start += length;
            self.rail = next;
        }
    }
}

/// A vehicle's track as the player reads it: its bytes in the replay, the ways it took, and
/// checkpoints to read on from.
pub(super) struct Track {
    bytes: Range<usize>,
    ways: Vec<u16>,
    checkpoints: Vec<Cursor>,
    cursor: Cursor,
}

impl Track {
    /// The track in `replay[bytes]`, checked by playing it through to `until` once.
    pub(super) fn read(
        replay: &[u8],
        bytes: Range<usize>,
        codec: &Codec,
        from: f64,
        until: f64,
        layout: &Layout,
        out: &[Vec<LinkId>],
    ) -> Result<Self, Error> {
        let part = &replay[bytes.clone()];
        let c = &codec.contexts;
        let mut reading = Reading::start(part, &codec.probabilities);
        let mut count = 0;
        c.count(&mut reading, 0, &mut count);
        if count > MAX_WAYS {
            return Err(corrupt());
        }
        let mut ways = Vec::new();
        for _ in 0..count {
            let mut way = 0;
            c.way(&mut reading, &mut way);
            if reading.damaged() {
                return Err(corrupt());
            }
            ways.push(u16::try_from(way).map_err(|_| corrupt())?);
        }
        let mut start = Start::default();
        start.code(&mut reading, c);
        let mut left = 0;
        c.count(&mut reading, 1, &mut left);
        if left > MAX_CHANGES {
            return Err(corrupt());
        }
        let a = match codec.kinematics.accelerations.get(start.symbol as usize) {
            Some(&a) => a,
            None if start.symbol == codec.kinematics.escape() => start.given,
            None => return Err(corrupt()),
        };
        let rail = usize::try_from(start.rail).map_err(|_| corrupt())?;
        let finite = [start.s, start.v, a].iter().all(|value| value.is_finite());
        if reading.damaged() || !finite || rail >= layout.links.len() {
            return Err(corrupt());
        }
        let segment = Segment {
            t: from,
            s: start.s,
            v: start.v,
            a,
            symbol: start.symbol,
        };
        let mut cursor = Cursor {
            read: reading.state(),
            left,
            segment,
            next: None,
            rail,
            start: 0.0,
            way: 0,
            seen: from,
        };
        cursor.read_next(part, codec)?;
        let first = cursor;
        let mut checkpoints = vec![first];
        let mut made = 0;
        while cursor.next.is_some() {
            cursor.step(part, codec)?;
            let s = cursor.segment.s;
            if cursor.segment.t > until || !cursor.advance_rails(s, layout, out, &ways) {
                return Err(corrupt());
            }
            made += 1;
            if made % CHECKPOINT == 0 {
                cursor.seen = cursor.segment.t;
                checkpoints.push(cursor);
            }
        }
        let (s, _) = cursor.segment.at(until);
        if !cursor.advance_rails(s, layout, out, &ways) {
            return Err(corrupt());
        }
        Ok(Self {
            bytes,
            ways,
            checkpoints,
            cursor: first,
        })
    }

    /// The rail and offset (mm) of the front at `time`, and the speed (mm/ms).
    pub(super) fn at(
        &mut self,
        time: f64,
        replay: &[u8],
        codec: &Codec,
        layout: &Layout,
        out: &[Vec<LinkId>],
    ) -> (LinkId, f64, f64) {
        let part = &replay[self.bytes.clone()];
        if time < self.cursor.seen {
            let index = self
                .checkpoints
                .partition_point(|checkpoint| checkpoint.segment.t <= time);
            self.cursor = self.checkpoints[index.saturating_sub(1)];
        }
        while self.cursor.next.is_some_and(|(_, t)| t <= time) {
            // Read through when the track was read.
            self.cursor.step(part, codec).expect("a checked track");
        }
        let segment = self.cursor.segment;
        let (s, v) = segment.at(time.max(segment.t));
        self.cursor.advance_rails(s, layout, out, &self.ways);
        self.cursor.seen = time;
        let rail = self.cursor.rail;
        let offset = (s - self.cursor.start).clamp(0.0, layout.links[rail].length);
        (rail, offset, v)
    }
}
