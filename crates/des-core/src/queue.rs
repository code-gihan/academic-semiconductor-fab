//! Future event list: binary min-heap ordered by (time, scheduling order).

use std::cmp::Ordering;
use std::collections::BinaryHeap;

use crate::Time;

/// Pending events. Events with equal times leave in the order they were pushed (FIFO), so a run
/// is fully determined by its inputs. The payload never takes part in the ordering.
pub(crate) struct EventQueue<E> {
    heap: BinaryHeap<Entry<E>>,
    next_seq: u64,
}

impl<E> EventQueue<E> {
    pub(crate) fn new() -> Self {
        Self {
            heap: BinaryHeap::new(),
            next_seq: 0,
        }
    }

    pub(crate) fn push(&mut self, time: Time, event: E) {
        self.heap.push(Entry {
            time,
            seq: self.next_seq,
            event,
        });
        self.next_seq += 1;
    }

    /// Time of the earliest event.
    pub(crate) fn next_time(&self) -> Option<Time> {
        self.heap.peek().map(|entry| entry.time)
    }

    /// Removes and returns the earliest event.
    pub(crate) fn pop(&mut self) -> Option<(Time, E)> {
        self.heap
            .pop()
            .map(|Entry { time, event, .. }| (time, event))
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
        let popped: Vec<_> = std::iter::from_fn(|| queue.pop()).collect();
        assert_eq!(popped, expected);
    }

    #[test]
    fn next_time_is_the_earliest() {
        let mut queue = EventQueue::new();
        assert_eq!(queue.next_time(), None);
        queue.push(20, 'b');
        queue.push(10, 'a');
        assert_eq!(queue.next_time(), Some(10));
        assert_eq!(queue.pop(), Some((10, 'a')));
        assert_eq!(queue.next_time(), Some(20));
    }
}
