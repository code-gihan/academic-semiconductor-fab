//! The rail network at run time: rails with their speed limits, the zone roles of nodes, and
//! routing along shortest distances. Every node has a single way on up to the next diverging
//! node; tables from each diverging node to each node hold the distance and the rail to take.

use std::cmp::Ordering;
use std::collections::BinaryHeap;

use crate::layout::{BayId, Layout, LinkId, NodeId, ZoneId, ZoneRole};

pub(super) struct Rail {
    pub from: NodeId,
    pub to: NodeId,
    pub length: f64,
    /// Speed limit (mm/ms).
    pub limit: f64,
    pub bay: BayId,
    /// The zone whose region the rail lies in.
    pub zone: Option<ZoneId>,
}

pub(super) struct Node {
    pub out: Vec<LinkId>,
    pub zone: Option<(ZoneId, ZoneRole)>,
    /// Several rails leave the node.
    pub diverging: bool,
}

pub(super) struct Track {
    pub rails: Vec<Rail>,
    pub nodes: Vec<Node>,
    /// Index of each diverging node among them.
    diverge_index: Vec<u32>,
    /// Per diverging node and target node: shortest distance (mm, rounded) and the index of the
    /// rail to take among the node's outgoing ones.
    distance: Vec<u32>,
    hop: Vec<u8>,
    /// Per node: the diverging node its single way leads to (index) and the distance there.
    chain: Vec<(u32, f64)>,
    /// Entry and exit order of each node in the tree of nodes leading to its diverging node:
    /// a node lies on another's way to their diverging node iff it is an ancestor there.
    enter: Vec<u32>,
    leave: Vec<u32>,
}

const NONE: u32 = u32::MAX;

impl Track {
    /// The track of `layout` with the speed limit of each rail given by `limit` (mm/ms).
    pub(super) fn new(layout: &Layout, limit: impl Fn(LinkId) -> f64) -> Self {
        let rails: Vec<Rail> = layout
            .links
            .iter()
            .enumerate()
            .map(|(id, link)| Rail {
                from: link.from,
                to: link.to,
                length: link.length,
                limit: limit(id),
                bay: link.bay,
                zone: link.zone,
            })
            .collect();
        let mut nodes: Vec<Node> = layout
            .nodes
            .iter()
            .map(|node| Node {
                out: Vec::new(),
                zone: node.zone,
                diverging: false,
            })
            .collect();
        for (id, rail) in rails.iter().enumerate() {
            nodes[rail.from].out.push(id);
        }
        for node in &mut nodes {
            node.diverging = node.out.len() > 1;
        }
        let count = nodes.len();
        let mut diverge_index = vec![NONE; count];
        let mut diverges = Vec::new();
        for (id, node) in nodes.iter().enumerate() {
            if node.diverging {
                diverge_index[id] = diverges.len() as u32;
                diverges.push(id);
            }
        }
        // The single way of every node up to its diverging node.
        let mut chain = vec![(NONE, 0.0); count];
        for start in 0..count {
            let mut walked = Vec::new();
            let mut node = start;
            let (index, mut rest) = loop {
                if diverge_index[node] != NONE {
                    break (diverge_index[node], 0.0);
                }
                if chain[node].0 != NONE {
                    break chain[node];
                }
                walked.push(node);
                assert!(walked.len() <= count, "a loop without diverging nodes");
                node = rails[nodes[node].out[0]].to;
            };
            for &node in walked.iter().rev() {
                rest += rails[nodes[node].out[0]].length;
                chain[node] = (index, rest);
            }
            if diverge_index[start] != NONE {
                chain[start] = (diverge_index[start], 0.0);
            }
        }
        // Trees of the single ways: children are the nodes whose single way enters a node.
        let mut children: Vec<Vec<NodeId>> = vec![Vec::new(); count];
        for (id, node) in nodes.iter().enumerate() {
            if !node.diverging {
                children[rails[node.out[0]].to].push(id);
            }
        }
        let (mut enter, mut leave) = (vec![0; count], vec![0; count]);
        let mut clock = 0;
        for &root in &diverges {
            let mut stack = vec![(root, false)];
            while let Some((node, done)) = stack.pop() {
                if done {
                    leave[node] = clock;
                    clock += 1;
                    continue;
                }
                enter[node] = clock;
                clock += 1;
                stack.push((node, true));
                for &child in children[node].iter().rev() {
                    if !nodes[child].diverging {
                        stack.push((child, false));
                    }
                }
            }
        }
        let mut distance = vec![u32::MAX; diverges.len() * count];
        let mut hop = vec![0u8; diverges.len() * count];
        let mut best = vec![f64::INFINITY; count];
        let mut first = vec![0u8; count];
        for (index, &source) in diverges.iter().enumerate() {
            best.fill(f64::INFINITY);
            best[source] = 0.0;
            let mut heap = BinaryHeap::new();
            heap.push(Entry(0.0, source));
            while let Some(Entry(dist, node)) = heap.pop() {
                if dist > best[node] {
                    continue;
                }
                for (way, &rail) in nodes[node].out.iter().enumerate() {
                    let next = rails[rail].to;
                    let through = dist + rails[rail].length;
                    if through < best[next] {
                        best[next] = through;
                        first[next] = if node == source {
                            way as u8
                        } else {
                            first[node]
                        };
                        heap.push(Entry(through, next));
                    }
                }
            }
            let row = index * count;
            for node in 0..count {
                distance[row + node] = best[node].round() as u32;
                hop[row + node] = first[node];
            }
        }
        Self {
            rails,
            nodes,
            diverge_index,
            distance,
            hop,
            chain,
            enter,
            leave,
        }
    }

    /// Node `on` lies on the single way from `node` to their diverging node.
    fn on_way(&self, on: NodeId, node: NodeId) -> bool {
        self.chain[on].0 == self.chain[node].0
            && self.enter[on] <= self.enter[node]
            && self.leave[node] <= self.leave[on]
    }

    /// Shortest distance from node `from` to node `to` (mm).
    pub(super) fn node_distance(&self, from: NodeId, to: NodeId) -> f64 {
        if from == to {
            return 0.0;
        }
        let (diverge, rest) = self.chain[from];
        if self.on_way(to, from) {
            return rest - self.chain[to].1;
        }
        rest + f64::from(self.distance[diverge as usize * self.nodes.len() + to])
    }

    /// Shortest distance from `offset` mm into rail `from` to `to_offset` mm into rail `to`.
    pub(super) fn distance(&self, from: LinkId, offset: f64, to: LinkId, to_offset: f64) -> f64 {
        if from == to && to_offset >= offset {
            return to_offset - offset;
        }
        self.rails[from].length - offset
            + self.node_distance(self.rails[from].to, self.rails[to].from)
            + to_offset
    }

    /// The rail to take from `node` toward node `target`.
    pub(super) fn next(&self, node: NodeId, target: NodeId) -> LinkId {
        let out = &self.nodes[node].out;
        if out.len() == 1 {
            return out[0];
        }
        let index = self.diverge_index[node] as usize * self.nodes.len() + target;
        out[usize::from(self.hop[index])]
    }

    /// Appends to `path` the rails after `from` (offset `offset`) up to rail `to` (offset
    /// `to_offset`), which is included; nothing if `to` lies ahead on `from`.
    pub(super) fn route(
        &self,
        from: LinkId,
        offset: f64,
        to: LinkId,
        to_offset: f64,
        path: &mut Vec<LinkId>,
    ) {
        if from == to && to_offset >= offset {
            return;
        }
        let target = self.rails[to].from;
        let mut node = self.rails[from].to;
        let mut steps = 0;
        while node != target {
            let rail = self.next(node, target);
            path.push(rail);
            node = self.rails[rail].to;
            steps += 1;
            assert!(steps <= self.nodes.len(), "routing loops");
        }
        path.push(to);
    }
}

/// Min-heap entry: distance, node.
struct Entry(f64, NodeId);

impl PartialEq for Entry {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Entry {}

impl PartialOrd for Entry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Entry {
    /// Reversed: the smallest distance, then the smallest node, first.
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .0
            .total_cmp(&self.0)
            .then_with(|| other.1.cmp(&self.1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::smat::tests::loop_layout;

    fn track() -> (Layout, Track) {
        let layout = loop_layout();
        let track = Track::new(&layout, |_| 1.0);
        (layout, track)
    }

    fn rail(layout: &Layout, name: &str) -> LinkId {
        layout
            .links
            .iter()
            .position(|link| link.name == name)
            .unwrap()
    }

    #[test]
    fn distances_follow_the_rails() {
        let (layout, track) = track();
        let node = |name: &str| {
            layout
                .nodes
                .iter()
                .position(|node| node.name == name)
                .unwrap()
        };
        // n0 → n1 (2 m) → shortcut n8, n7 (3 m) → n4 (1 m): 6 m, shorter than around (10 m).
        assert_eq!(track.node_distance(node("n0"), node("n4")), 6_000.0);
        // n2 lies on n9's single way: 1 m.
        assert_eq!(track.node_distance(node("n9"), node("n2")), 1_000.0);
        // From n2 back to n1: around the loop, 4 + 1 + 1 + 2 + 4 + 2 = 14 m.
        assert_eq!(track.node_distance(node("n2"), node("n1")), 14_000.0);
        // Rail positions: 500 mm into r9 to 1,000 mm into r0, around.
        let (r9, r0) = (rail(&layout, "r9"), rail(&layout, "r0"));
        assert_eq!(
            track.distance(r9, 500.0, r0, 1_000.0),
            500.0 + 12_000.0 + 1_000.0
        );
        assert_eq!(track.distance(r0, 200.0, r0, 900.0), 700.0);
    }

    #[test]
    fn routes_take_the_shortcut() {
        let (layout, track) = track();
        let mut path = Vec::new();
        track.route(
            rail(&layout, "r0"),
            0.0,
            rail(&layout, "r4"),
            500.0,
            &mut path,
        );
        let names: Vec<_> = path
            .iter()
            .map(|&id| layout.links[id].name.as_str())
            .collect();
        assert_eq!(names, ["s1", "s8", "s7", "r4"]);
        // Behind on the same rail: once around.
        path.clear();
        track.route(
            rail(&layout, "r0"),
            900.0,
            rail(&layout, "r0"),
            100.0,
            &mut path,
        );
        let names: Vec<_> = path
            .iter()
            .map(|&id| layout.links[id].name.as_str())
            .collect();
        assert_eq!(names, ["s1", "s8", "s7", "r4", "r5", "r0"]);
    }
}
