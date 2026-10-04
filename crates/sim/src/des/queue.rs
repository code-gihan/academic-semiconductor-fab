//! Future event list: binary min-heap ordered by (time, scheduling order).

use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::collections::binary_heap::PeekMut;

use super::Time;

/// Pending events. Events with equal times leave in the order they were pushed (FIFO), so a run
/// is fully determined by its inputs. The payload never takes part in the ordering.
pub(super) struct EventQueue<E> {
    heap: BinaryHeap<Entry<E>>,
    next_seq: u64,
}

impl<E> EventQueue<E> {
    pub(super) fn new() -> Self {
        Self {
            heap: BinaryHeap::new(),
            next_seq: 0,
        }
    }

    pub(super) fn push(&mut self, time: Time, event: E) {
        self.heap.push(Entry {
            time,
            seq: self.next_seq,
            event,
        });
        self.next_seq += 1;
    }

    /// Removes and returns the earliest event if it is due at or before `limit`.
    pub(super) fn pop_due(&mut self, limit: Time) -> Option<(Time, E)> {
        let top = self.heap.peek_mut()?;
        if top.time > limit {
            return None;
        }
        let Entry { time, event, .. } = PeekMut::pop(top);
        Some((time, event))
    }
}

struct Entry<E> {
    time: Time,
    seq: u64,
    event: E,
}

impl<E> Ord for Entry<E> {
    // `BinaryHeap` is a max-heap; reversing puts the earliest (time, seq) on top.
    fn cmp(&self, other: &Self) -> Ordering {
        (other.time, other.seq).cmp(&(self.time, self.seq))
    }
}

impl<E> PartialOrd for Entry<E> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<E> PartialEq for Entry<E> {
    fn eq(&self, other: &Self) -> bool {
        (self.time, self.seq) == (other.time, other.seq)
    }
}

impl<E> Eq for Entry<E> {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pops_by_time_then_push_order() {
        let mut queue = EventQueue::new();
        let pushed: Vec<(Time, usize)> = (0..1_000).map(|i| ((i * 37 % 11) as Time, i)).collect();
        for &(time, id) in &pushed {
            queue.push(time, id);
        }
        let mut expected = pushed;
        expected.sort_by_key(|&(time, _)| time); // stable: push order within equal times
        let popped: Vec<_> = std::iter::from_fn(|| queue.pop_due(Time::MAX)).collect();
        assert_eq!(popped, expected);
    }

    #[test]
    fn pop_due_stops_at_limit() {
        let mut queue = EventQueue::new();
        queue.push(20, 'b');
        queue.push(10, 'a');
        assert_eq!(queue.pop_due(15), Some((10, 'a')));
        assert_eq!(queue.pop_due(15), None);
        assert_eq!(queue.pop_due(20), Some((20, 'b')));
        assert_eq!(queue.pop_due(Time::MAX), None);
    }
}
