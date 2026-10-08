//! AMHS layout of a dataset (SMAT2022, Lee et al. 2022): the OHT rail network with its zone
//! control units (ZCU), the bays, where the tools, stockers and commit and complete stations
//! stand, the ports vehicles serve, and the vehicles. Lengths in mm, speeds in mm/s,
//! accelerations in mm/s². [`crate::smat`] builds and validates it; the simulation runs its AMHS
//! on it.

use serde::{Deserialize, Serialize};

use crate::data::ToolGroupId;

pub type NodeId = usize;
pub type LinkId = usize;
pub type ZoneId = usize;
pub type BayId = usize;
pub type PortId = usize;

#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct Layout {
    pub nodes: Vec<RailNode>,
    pub links: Vec<RailLink>,
    /// ZCU names; a zone's stop and reset nodes carry its index.
    pub zones: Vec<String>,
    pub bays: Vec<Bay>,
    /// Every tool of the dataset, tool group by tool group in dataset order (as the simulation
    /// numbers them); none for the tools of skipped groups.
    pub tools: Vec<Option<Station>>,
    pub stockers: Vec<Station>,
    pub commits: Vec<Station>,
    pub completes: Vec<Station>,
    pub ports: Vec<Port>,
    pub vehicle_types: Vec<VehicleType>,
    pub vehicles: Vec<Vehicle>,
    /// Tool groups without equipment: their steps are left out (SMAT2022 drops `Delay_32`).
    pub skipped: Vec<ToolGroupId>,
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct RailNode {
    pub name: String,
    pub x: f64,
    pub y: f64,
    /// The node starts (stop node) or ends (reset node) a zone.
    pub zone: Option<(ZoneId, ZoneRole)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ZoneRole {
    /// A vehicle needs the zone to pass this node.
    Stop,
    /// A vehicle passing this node leaves the zone.
    Reset,
}

/// A one-way rail from `from` to `to`.
#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct RailLink {
    pub name: String,
    pub from: NodeId,
    pub to: NodeId,
    pub bay: BayId,
    /// Speed limit (mm/s).
    pub max_speed: f64,
    /// Length (mm): the chord of a straight link, the arc of a curve.
    pub length: f64,
    pub arc: Option<Arc>,
    /// The zone whose region the link lies in: from one of its stop nodes to one of its reset
    /// nodes.
    pub zone: Option<ZoneId>,
}

/// Circular arc of a curved link.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Arc {
    pub cx: f64,
    pub cy: f64,
    pub radius: f64,
    /// Angle of the start point seen from the center (rad).
    pub start: f64,
    /// Signed sweep (rad): positive counterclockwise.
    pub sweep: f64,
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct Bay {
    pub name: String,
    pub interbay: bool,
    /// Holds lithography tools.
    pub reticle: bool,
    pub neighbors: Vec<BayId>,
    /// Row and column of an intrabay named row letters + column number (A12): bay distances
    /// count columns, plus one between rows.
    pub grid: Option<(String, u32)>,
}

/// A tool, stocker or commit or complete station: its bay and footprint.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Station {
    pub name: String,
    pub bay: BayId,
    /// Center.
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// Where a vehicle stops to load or unload a FOUP.
#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct Port {
    pub name: String,
    pub kind: PortKind,
    pub link: LinkId,
    /// Position on the link (mm from its start).
    pub offset: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PortKind {
    /// Load port of a tool (index in [`Layout::tools`]).
    Tool(usize),
    /// Port of a stocker.
    Stocker(usize),
    /// Lots enter the fab here (pickup only).
    Commit(usize),
    /// Lots leave the fab here (drop-off only).
    Complete(usize),
    /// Track buffer: one FOUP.
    Buffer(BufferSide),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BufferSide {
    /// Side track buffers (LSTB, RSTB).
    Left,
    Right,
    /// Under track buffer (UTB).
    Under,
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct VehicleType {
    pub name: String,
    /// Length on the rail (mm).
    pub length: f64,
    pub max_speed: f64,
    pub acceleration: f64,
    pub deceleration: f64,
    /// Least distance to the vehicle ahead (mm).
    pub min_gap: f64,
}

/// A vehicle and its position at time 0.
#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct Vehicle {
    pub name: String,
    pub kind: usize,
    pub link: LinkId,
    /// Position of its front on the link (mm from the link's start).
    pub offset: f64,
}

named_enum! {
    /// What a port serves.
    pub enum PortRole {
        Tool = "tool",
        Buffer = "buffer",
        Stocker = "stocker",
        Commit = "commit",
        Complete = "complete",
    }
}

/// The layout for drawing (mm): the node points, the rails between them (curves on their arcs),
/// the bays with the extent of their rails, the footprints of the tools and stations, the points
/// of the ports, and the vehicles' lengths.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Drawing {
    /// Least x and y and greatest x and y of the nodes and footprints.
    pub bounds: [f64; 4],
    pub nodes: Vec<[f64; 2]>,
    pub rails: Vec<RailDrawing>,
    pub bays: Vec<BayDrawing>,
    /// As [`Layout::tools`].
    pub tools: Vec<Option<Station>>,
    pub stockers: Vec<Station>,
    pub commits: Vec<Station>,
    pub completes: Vec<Station>,
    pub ports: Vec<PortDrawing>,
    /// Each vehicle's length (mm), by id.
    pub vehicle_lengths: Vec<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RailDrawing {
    /// Nodes (indices in [`Drawing::nodes`]).
    pub from: NodeId,
    pub to: NodeId,
    pub bay: BayId,
    /// As [`RailLink::length`].
    pub length: f64,
    pub arc: Option<Arc>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct BayDrawing {
    pub name: String,
    pub interbay: bool,
    /// Extent of its rails' nodes, as [`Drawing::bounds`].
    pub bounds: [f64; 4],
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PortDrawing {
    pub x: f64,
    pub y: f64,
    pub role: PortRole,
    /// The tool, stocker, commit or complete station it belongs to (index in its list).
    pub station: Option<usize>,
}

/// Extent of points, as [`Drawing::bounds`].
fn extent(points: impl Iterator<Item = (f64, f64)>) -> [f64; 4] {
    points.fold(
        [
            f64::INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NEG_INFINITY,
        ],
        |[x0, y0, x1, y1], (x, y)| [x0.min(x), y0.min(y), x1.max(x), y1.max(y)],
    )
}

impl Layout {
    /// The layout for drawing.
    pub fn drawing(&self) -> Drawing {
        let footprints = self
            .tools
            .iter()
            .flatten()
            .chain(&self.stockers)
            .chain(&self.commits)
            .chain(&self.completes);
        let corners = footprints.flat_map(|station| {
            let (dx, dy) = (station.width / 2.0, station.height / 2.0);
            [
                (station.x - dx, station.y - dy),
                (station.x + dx, station.y + dy),
            ]
        });
        let nodes = self.nodes.iter().map(|node| (node.x, node.y));
        let bays = self
            .bays
            .iter()
            .enumerate()
            .map(|(bay, spec)| BayDrawing {
                name: spec.name.clone(),
                interbay: spec.interbay,
                bounds: extent(
                    self.links
                        .iter()
                        .filter(|link| link.bay == bay)
                        .flat_map(|link| [&self.nodes[link.from], &self.nodes[link.to]])
                        .map(|node| (node.x, node.y)),
                ),
            })
            .collect();
        Drawing {
            bounds: extent(nodes.clone().chain(corners)),
            nodes: nodes.map(|(x, y)| [x, y]).collect(),
            rails: self
                .links
                .iter()
                .map(|link| RailDrawing {
                    from: link.from,
                    to: link.to,
                    bay: link.bay,
                    length: link.length,
                    arc: link.arc,
                })
                .collect(),
            bays,
            tools: self.tools.clone(),
            stockers: self.stockers.clone(),
            commits: self.commits.clone(),
            completes: self.completes.clone(),
            ports: self
                .ports
                .iter()
                .map(|port| {
                    let (x, y, _) = self.point(port.link, port.offset);
                    let (role, station) = match port.kind {
                        PortKind::Tool(tool) => (PortRole::Tool, Some(tool)),
                        PortKind::Stocker(stocker) => (PortRole::Stocker, Some(stocker)),
                        PortKind::Commit(commit) => (PortRole::Commit, Some(commit)),
                        PortKind::Complete(complete) => (PortRole::Complete, Some(complete)),
                        PortKind::Buffer(_) => (PortRole::Buffer, None),
                    };
                    PortDrawing {
                        x,
                        y,
                        role,
                        station,
                    }
                })
                .collect(),
            vehicle_lengths: self
                .vehicles
                .iter()
                .map(|vehicle| self.vehicle_types[vehicle.kind].length)
                .collect(),
        }
    }

    /// Point and heading (rad) of the position `offset` mm into `link`.
    pub fn point(&self, link: LinkId, offset: f64) -> (f64, f64, f64) {
        let rail = &self.links[link];
        match rail.arc {
            None => {
                let (from, to) = (&self.nodes[rail.from], &self.nodes[rail.to]);
                let share = if rail.length > 0.0 {
                    (offset / rail.length).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                (
                    from.x + (to.x - from.x) * share,
                    from.y + (to.y - from.y) * share,
                    libm::atan2(to.y - from.y, to.x - from.x),
                )
            }
            Some(arc) => {
                let share = if rail.length > 0.0 {
                    (offset / rail.length).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                let angle = arc.start + arc.sweep * share;
                let turn = if arc.sweep >= 0.0 {
                    std::f64::consts::FRAC_PI_2
                } else {
                    -std::f64::consts::FRAC_PI_2
                };
                (
                    arc.cx + arc.radius * libm::cos(angle),
                    arc.cy + arc.radius * libm::sin(angle),
                    angle + turn,
                )
            }
        }
    }

    /// Point of the rear of a vehicle `length` mm long whose front is `offset` mm into `link`,
    /// having come from rail `behind`: there while the front is less than `length` into its rail
    /// (rails are longer than vehicles).
    pub fn rear(&self, link: LinkId, offset: f64, behind: LinkId, length: f64) -> (f64, f64) {
        let (rail, at) = if offset >= length {
            (link, offset - length)
        } else {
            (behind, self.links[behind].length - (length - offset))
        };
        let (x, y, _) = self.point(rail, at);
        (x, y)
    }

    /// Bay distance of a move between two bays (\[SMAT2022\] §4.2): the column difference of two
    /// intrabays, plus one between rows; `None` if a bay is not an intrabay of the grid.
    pub fn bay_distance(&self, from: BayId, to: BayId) -> Option<u32> {
        let (Some((row_a, column_a)), Some((row_b, column_b))) =
            (&self.bays[from].grid, &self.bays[to].grid)
        else {
            return None;
        };
        Some(column_a.abs_diff(*column_b) + u32::from(row_a != row_b))
    }
}
