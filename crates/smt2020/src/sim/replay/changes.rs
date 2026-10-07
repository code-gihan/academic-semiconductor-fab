//! What a replay keeps beside the vehicles' motion: their activities, the FOUPs' moves and the
//! tools' states, each a list in time order. A change is coded as its ms after the last one, what
//! it concerns (a vehicle, lot or tool, by its bits) and its new value in the context of the old
//! one (an activity after an activity, a place after a place, a state after a state); a lot's kind
//! once, at its first move.

use std::collections::BTreeMap;

use des_core::Time;

use super::FoupPlace;
use super::coder::{Coder, Contexts, Reading, gamma, tree, width};
use super::motion::{COUNT_LENGTHS, nodes};
use crate::sim::Error;
use crate::sim::amhs::Activity;
use crate::sim::stats::LotKind;
use crate::sim::tool::ToolState;

/// Unary contexts of the bit lengths of the ms between changes.
const GAP_LENGTHS: usize = 32;
/// The most changes a list holds: far beyond any window's, a bound on what damaged bytes may
/// claim.
const MAX_CHANGES: u64 = 1 << 26;

/// A vehicle does `activity` from `time` on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ActivityChange {
    pub time: Time,
    pub vehicle: usize,
    pub activity: Activity,
}

/// A lot's FOUP is at `place` (the port, vehicle or tool `index`) from `time` on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct FoupMove {
    pub time: Time,
    pub lot: u64,
    pub kind: LotKind,
    pub place: FoupPlace,
    pub index: usize,
}

/// A tool is in `state` from `time` on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ToolChange {
    pub time: Time,
    pub tool: usize,
    pub state: ToolState,
}

/// The contexts of the change lists, and the bits of what changes concern.
pub(super) struct ChangeContexts {
    vehicle_bits: u32,
    port_bits: u32,
    tool_bits: u32,
    lot_bits: u32,
    /// Per list: its length, its gaps.
    counts: usize,
    gaps: usize,
    /// Per old value (the last row for none yet): the new one's tree.
    activity: usize,
    place: usize,
    state: usize,
    kind: usize,
}

impl ChangeContexts {
    pub(super) fn new(
        contexts: &mut Contexts,
        vehicles: usize,
        ports: usize,
        tools: usize,
        lot_bits: u32,
    ) -> Self {
        let rows = |names: usize| (names + 1) * nodes(depth(names));
        Self {
            vehicle_bits: width(vehicles.saturating_sub(1) as u64),
            port_bits: width(ports.saturating_sub(1) as u64),
            tool_bits: width(tools.saturating_sub(1) as u64),
            lot_bits,
            counts: contexts.take(3 * COUNT_LENGTHS),
            gaps: contexts.take(3 * GAP_LENGTHS),
            activity: contexts.take(rows(Activity::ALL.len())),
            place: contexts.take(rows(FoupPlace::ALL.len())),
            state: contexts.take(rows(ToolState::ALL.len())),
            kind: contexts.take(nodes(depth(LotKind::ALL.len()))),
        }
    }

    /// Codes list `list`'s length.
    fn count(&self, coder: &mut impl Coder, list: usize, count: &mut u64) {
        gamma(
            coder,
            self.counts + list * COUNT_LENGTHS,
            COUNT_LENGTHS,
            count,
        );
    }

    /// Codes `time`, of a change after one at `last` in list `list`; moves `last` on. False if it
    /// is no time.
    fn time(&self, coder: &mut impl Coder, list: usize, last: &mut Time, time: &mut Time) -> bool {
        let mut gap = time.wrapping_sub(*last) as u64;
        gamma(coder, self.gaps + list * GAP_LENGTHS, GAP_LENGTHS, &mut gap);
        match i64::try_from(gap)
            .ok()
            .and_then(|gap| last.checked_add(gap))
        {
            Some(next) => {
                *time = next;
                *last = next;
                true
            }
            None => false,
        }
    }
}

/// Levels of a tree over `names` values.
fn depth(names: usize) -> u32 {
    width(names.saturating_sub(1) as u64)
}

/// Codes `value`, one of `names`, with the tree of row `row` among those from `first`; its index,
/// none if it is none of them.
fn named<T: Copy + PartialEq>(
    coder: &mut impl Coder,
    first: usize,
    row: usize,
    names: &[T],
    value: &mut T,
) -> Option<usize> {
    let depth = depth(names.len());
    let mut index = names.iter().position(|name| name == value).unwrap_or(0) as u32;
    tree(coder, first + row * nodes(depth), depth, &mut index);
    *value = *names.get(index as usize)?;
    Some(index as usize)
}

/// Codes `bits` bits of `value`, an index below `limit`; none if it is not one.
fn index_of(coder: &mut impl Coder, bits: u32, limit: usize, value: &mut usize) -> Option<()> {
    let mut raw = *value as u64;
    coder.bits(bits, &mut raw);
    *value = usize::try_from(raw).ok().filter(|&index| index < limit)?;
    Some(())
}

/// A window's changes.
#[derive(Default)]
pub(super) struct ChangeLists {
    pub activities: Vec<ActivityChange>,
    pub foups: Vec<FoupMove>,
    pub tools: Vec<ToolChange>,
}

/// The coding of a window's changes: the given lists are coded, or read into `read`.
pub(super) struct ChangeCoding<'c> {
    contexts: &'c ChangeContexts,
    vehicles: usize,
    ports: usize,
    tools: usize,
    from: Time,
}

impl<'c> ChangeCoding<'c> {
    pub(super) fn new(
        contexts: &'c ChangeContexts,
        vehicles: usize,
        ports: usize,
        tools: usize,
        from: Time,
    ) -> Self {
        Self {
            contexts,
            vehicles,
            ports,
            tools,
            from,
        }
    }

    /// Codes `lists` (their changes in time order, of the window's vehicles, ports and tools).
    pub(super) fn code(&self, coder: &mut impl Coder, lists: &ChangeLists) {
        let coded = self
            .activities(coder, &lists.activities, None)
            .and_then(|()| self.foups(coder, &lists.foups, None))
            .and_then(|()| self.tools(coder, &lists.tools, None));
        debug_assert!(coded.is_some(), "changes out of order or out of range");
    }

    /// The lists in `bytes`.
    pub(super) fn read(&self, bytes: &[u8], probabilities: &[u16]) -> Result<ChangeLists, Error> {
        let mut reading = Reading::start(bytes, probabilities);
        let mut lists = ChangeLists::default();
        let read = self
            .activities(&mut reading, &[], Some(&mut lists.activities))
            .and_then(|()| self.foups(&mut reading, &[], Some(&mut lists.foups)))
            .and_then(|()| self.tools(&mut reading, &[], Some(&mut lists.tools)));
        match read {
            Some(()) if !reading.damaged() => Ok(lists),
            _ => Err(super::corrupt()),
        }
    }

    fn activities(
        &self,
        coder: &mut impl Coder,
        given: &[ActivityChange],
        mut read: Option<&mut Vec<ActivityChange>>,
    ) -> Option<()> {
        let c = self.contexts;
        let mut count = given.len() as u64;
        c.count(coder, 0, &mut count);
        (count <= MAX_CHANGES).then_some(())?;
        let mut rows = vec![Activity::ALL.len(); self.vehicles];
        let mut last = self.from;
        for index in 0..count as usize {
            let mut change = given.get(index).copied().unwrap_or(ActivityChange {
                time: 0,
                vehicle: 0,
                activity: Activity::ALL[0],
            });
            c.time(coder, 0, &mut last, &mut change.time)
                .then_some(())?;
            index_of(coder, c.vehicle_bits, self.vehicles, &mut change.vehicle)?;
            let row = &mut rows[change.vehicle];
            *row = named(coder, c.activity, *row, Activity::ALL, &mut change.activity)?;
            if let Some(read) = &mut read {
                read.push(change);
            }
        }
        Some(())
    }

    fn foups(
        &self,
        coder: &mut impl Coder,
        given: &[FoupMove],
        mut read: Option<&mut Vec<FoupMove>>,
    ) -> Option<()> {
        let c = self.contexts;
        let mut count = given.len() as u64;
        c.count(coder, 1, &mut count);
        (count <= MAX_CHANGES).then_some(())?;
        // Per lot in the fab: its kind and its place's row.
        let mut lots: BTreeMap<u64, (LotKind, usize)> = BTreeMap::new();
        let mut last = self.from;
        for index in 0..count as usize {
            let mut change = given.get(index).copied().unwrap_or(FoupMove {
                time: 0,
                lot: 0,
                kind: LotKind::ALL[0],
                place: FoupPlace::ALL[0],
                index: 0,
            });
            c.time(coder, 1, &mut last, &mut change.time)
                .then_some(())?;
            coder.bits(c.lot_bits, &mut change.lot);
            let row = match lots.get(&change.lot) {
                Some(&(kind, row)) => {
                    change.kind = kind;
                    row
                }
                None => {
                    named(coder, c.kind, 0, LotKind::ALL, &mut change.kind)?;
                    FoupPlace::ALL.len()
                }
            };
            let place = named(coder, c.place, row, FoupPlace::ALL, &mut change.place)?;
            let (bits, limit) = match change.place {
                FoupPlace::Port | FoupPlace::Commit | FoupPlace::Gone => (c.port_bits, self.ports),
                FoupPlace::Vehicle => (c.vehicle_bits, self.vehicles),
                FoupPlace::Tool => (c.tool_bits, self.tools),
            };
            index_of(coder, bits, limit, &mut change.index)?;
            if change.place == FoupPlace::Gone {
                lots.remove(&change.lot);
            } else {
                lots.insert(change.lot, (change.kind, place));
            }
            if let Some(read) = &mut read {
                read.push(change);
            }
        }
        Some(())
    }

    fn tools(
        &self,
        coder: &mut impl Coder,
        given: &[ToolChange],
        mut read: Option<&mut Vec<ToolChange>>,
    ) -> Option<()> {
        let c = self.contexts;
        let mut count = given.len() as u64;
        c.count(coder, 2, &mut count);
        (count <= MAX_CHANGES).then_some(())?;
        let mut rows = vec![ToolState::ALL.len(); self.tools];
        let mut last = self.from;
        for index in 0..count as usize {
            let mut change = given.get(index).copied().unwrap_or(ToolChange {
                time: 0,
                tool: 0,
                state: ToolState::ALL[0],
            });
            c.time(coder, 2, &mut last, &mut change.time)
                .then_some(())?;
            index_of(coder, c.tool_bits, self.tools, &mut change.tool)?;
            let row = &mut rows[change.tool];
            *row = named(coder, c.state, *row, ToolState::ALL, &mut change.state)?;
            if let Some(read) = &mut read {
                read.push(change);
            }
        }
        Some(())
    }
}
