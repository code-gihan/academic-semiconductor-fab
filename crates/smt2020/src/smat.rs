//! Loader of the SMAT2022 AMHS tables (Lee et al., 2022; generic format `SMAT_2022_generic`):
//! the sheets Address, ZCU, Rail, Bay, Equipment, PortType, Port, VehicleType and Vehicle, which
//! the CLI reads from the dataset's xlsx. They are validated against the SMT2020 dataset they
//! extend and become its [`Layout`]; input the AMHS model does not support is an error, never
//! silently dropped.

use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt;

use crate::data::{Dataset, ToolGroupId};
use crate::layout::{
    Arc, Bay, BufferSide, Layout, LinkId, NodeId, Port, PortKind, RailLink, RailNode, Station,
    Vehicle, VehicleType, ZoneId, ZoneRole,
};

/// A cell of a sheet.
#[derive(Clone, Debug, PartialEq)]
pub enum Cell {
    Empty,
    Text(String),
    Number(f64),
}

/// A sheet: its rows, the first one holding the column names.
#[derive(Clone, Debug, PartialEq)]
pub struct Sheet {
    pub name: String,
    pub rows: Vec<Vec<Cell>>,
}

#[derive(Debug)]
pub struct Error(String);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

fn error(message: impl Into<String>) -> Error {
    Error(message.into())
}

/// The sheets of a SMAT2022 dataset.
pub const SHEETS: [&str; 9] = [
    "Address",
    "ZCU",
    "Rail",
    "Bay",
    "Equipment",
    "PortType",
    "Port",
    "VehicleType",
    "Vehicle",
];

/// Equipment tool groups that are no SMT2020 tool group.
const COMMIT: &str = "COMMIT";
const COMPLETE: &str = "COMPLETE";
const STOCKER: &str = "STOCKER";

/// The layout of `sheets` (by name, [`SHEETS`]) for `data`, the SMT2020 dataset they extend.
pub fn layout(data: &Dataset, sheets: &[Sheet]) -> Result<Layout, Error> {
    let sheet = |name: &str| -> Result<Table<'_>, Error> {
        let sheet = sheets
            .iter()
            .find(|sheet| sheet.name == name)
            .ok_or_else(|| error(format!("no sheet {name}")))?;
        Table::new(sheet)
    };
    let bays = read_bays(&sheet("Bay")?)?;
    let (zones, zone_names) = read_zones(&sheet("ZCU")?)?;
    let (mut nodes, node_names) = read_nodes(&sheet("Address")?, &zone_names)?;
    let (mut links, link_names) = read_links(&sheet("Rail")?, &mut nodes, &node_names, &bays)?;
    check_connected(&nodes, &links)?;
    derive_zones(&zones, &nodes, &mut links)?;
    let equipment = read_equipment(&sheet("Equipment")?, data, &bays)?;
    let ports = read_ports(
        &sheet("PortType")?,
        &sheet("Port")?,
        &equipment,
        &links,
        &link_names,
    )?;
    let vehicle_types = read_vehicle_types(&sheet("VehicleType")?)?;
    let vehicles = read_vehicles(&sheet("Vehicle")?, &vehicle_types, &links, &link_names)?;
    let layout = Layout {
        nodes,
        links,
        zones,
        bays: bays.items,
        tools: equipment.tools,
        stockers: equipment.stockers,
        commits: equipment.commits,
        completes: equipment.completes,
        ports,
        vehicle_types,
        vehicles,
        skipped: equipment.skipped,
    };
    check_vehicles(&layout)?;
    Ok(layout)
}

/// Names in definition order, indexed for lookup.
#[derive(Default)]
struct Names {
    index: HashMap<String, usize>,
}

impl Names {
    fn define(&mut self, row: &Row, name: &str, what: &str) -> Result<usize, Error> {
        let id = self.index.len();
        if self.index.insert(name.to_owned(), id).is_some() {
            return Err(row.error(format_args!("duplicate {what} {name}")));
        }
        Ok(id)
    }

    fn get(&self, row: &Row, name: &str, what: &str) -> Result<usize, Error> {
        self.index
            .get(name)
            .copied()
            .ok_or_else(|| row.error(format_args!("unknown {what} {name}")))
    }
}

struct Table<'a> {
    sheet: &'a Sheet,
    columns: HashMap<String, usize>,
    /// Column names by index.
    names: Vec<String>,
}

impl<'a> Table<'a> {
    fn new(sheet: &'a Sheet) -> Result<Self, Error> {
        let header = sheet
            .rows
            .first()
            .ok_or_else(|| error(format!("{}: no header row", sheet.name)))?;
        let names: Vec<String> = header
            .iter()
            .map(|cell| text(cell).unwrap_or_default())
            .collect();
        let columns = names
            .iter()
            .enumerate()
            .filter(|(_, name)| !name.is_empty())
            .map(|(index, name)| (name.clone(), index))
            .collect();
        Ok(Self {
            sheet,
            columns,
            names,
        })
    }

    fn col(&self, name: &str) -> Result<usize, Error> {
        self.columns
            .get(name)
            .copied()
            .ok_or_else(|| error(format!("{}: no column {name}", self.sheet.name)))
    }

    fn cols<const N: usize>(&self, names: [&str; N]) -> Result<[usize; N], Error> {
        let mut cols = [0; N];
        for (col, name) in cols.iter_mut().zip(names) {
            *col = self.col(name)?;
        }
        Ok(cols)
    }

    /// The data rows, empty ones left out.
    fn rows(&self) -> impl Iterator<Item = Row<'_>> {
        self.sheet
            .rows
            .iter()
            .enumerate()
            .skip(1)
            .filter(|(_, cells)| cells.iter().any(|cell| text(cell).is_some()))
            .map(|(index, cells)| Row {
                sheet: &self.sheet.name,
                line: index + 1,
                cells,
                names: &self.names,
            })
    }
}

/// Text of a cell; an integral number reads as its digits.
fn text(cell: &Cell) -> Option<String> {
    match cell {
        Cell::Empty => None,
        Cell::Text(text) if text.trim().is_empty() => None,
        Cell::Text(text) => Some(text.trim().to_owned()),
        Cell::Number(number) if number.fract() == 0.0 && number.abs() < 1e15 => {
            Some(format!("{}", *number as i64))
        }
        Cell::Number(number) => Some(number.to_string()),
    }
}

#[derive(Clone, Copy)]
struct Row<'a> {
    sheet: &'a str,
    /// Line in the sheet, from 1.
    line: usize,
    cells: &'a [Cell],
    /// Column names by index.
    names: &'a [String],
}

impl Row<'_> {
    fn error(&self, message: impl fmt::Display) -> Error {
        error(format!("{}:{}: {message}", self.sheet, self.line))
    }

    fn cell(&self, col: usize) -> &Cell {
        self.cells.get(col).unwrap_or(&Cell::Empty)
    }

    fn text(&self, col: usize) -> Result<String, Error> {
        text(self.cell(col)).ok_or_else(|| self.error(format_args!("empty {}", self.names[col])))
    }

    /// Text, or none for an empty cell or the placeholder `,` of the SMAT2022 files.
    fn opt_text(&self, col: usize) -> Option<String> {
        text(self.cell(col)).filter(|text| text != ",")
    }

    fn number(&self, col: usize) -> Result<f64, Error> {
        let value = match self.cell(col) {
            Cell::Number(number) => Some(*number),
            Cell::Text(text) => text.trim().parse().ok(),
            Cell::Empty => None,
        };
        value
            .filter(|value: &f64| value.is_finite())
            .ok_or_else(|| self.error(format_args!("{} is not a number", self.names[col])))
    }

    fn positive(&self, col: usize, what: &str) -> Result<f64, Error> {
        let value = self.number(col)?;
        if value > 0.0 {
            Ok(value)
        } else {
            Err(self.error(format_args!("{what} must be positive")))
        }
    }

    fn flag(&self, col: usize) -> Result<bool, Error> {
        match self.number(col)? {
            0.0 => Ok(false),
            1.0 => Ok(true),
            _ => Err(self.error(format_args!("{} must be 0 or 1", self.names[col]))),
        }
    }
}

struct Bays {
    items: Vec<Bay>,
    names: Names,
}

fn read_bays(table: &Table) -> Result<Bays, Error> {
    let [name, kind, reticle, neighbors] =
        table.cols(["NAME", "TYPE", "RETICLE", "NEIGHBOR_BAY"])?;
    let (mut items, mut names) = (Vec::new(), Names::default());
    let mut neighbor_names = Vec::new();
    for row in table.rows() {
        let bay = row.text(name)?;
        names.define(&row, &bay, "bay")?;
        let interbay = match row.text(kind)?.to_ascii_uppercase().as_str() {
            "INTRABAY" => false,
            "INTERBAY" => true,
            _ => return Err(row.error("TYPE must be INTRABAY or INTERBAY")),
        };
        let grid = (!interbay).then(|| grid(&bay)).flatten();
        neighbor_names.push((row, row.opt_text(neighbors).unwrap_or_default()));
        items.push(Bay {
            name: bay,
            interbay,
            reticle: row.flag(reticle)?,
            neighbors: Vec::new(),
            grid,
        });
    }
    for (bay, (row, list)) in items.iter_mut().zip(neighbor_names) {
        for neighbor in list
            .split(',')
            .map(str::trim)
            .filter(|name| !name.is_empty())
        {
            let id = names.get(&row, neighbor, "neighbor bay")?;
            if !bay.neighbors.contains(&id) {
                bay.neighbors.push(id);
            }
        }
    }
    Ok(Bays { items, names })
}

/// Row letters and column number of a bay named like `A12`.
fn grid(name: &str) -> Option<(String, u32)> {
    let digits = name.find(|c: char| c.is_ascii_digit())?;
    let (row, column) = name.split_at(digits);
    (!row.is_empty() && row.chars().all(|c| c.is_ascii_alphabetic()))
        .then(|| column.parse().ok().map(|column| (row.to_owned(), column)))
        .flatten()
}

fn read_zones(table: &Table) -> Result<(Vec<String>, Names), Error> {
    let name = table.col("NAME")?;
    let (mut zones, mut names) = (Vec::new(), Names::default());
    for row in table.rows() {
        let zone = row.text(name)?;
        names.define(&row, &zone, "ZCU")?;
        zones.push(zone);
    }
    Ok((zones, names))
}

fn read_nodes(table: &Table, zones: &Names) -> Result<(Vec<RailNode>, Names), Error> {
    let [name, x, y, zone, kind] =
        table.cols(["NAME", "POSITION_X", "POSITION_Y", "ZCU_NAME", "ZCU_TYPE"])?;
    let (mut nodes, mut names) = (Vec::new(), Names::default());
    for row in table.rows() {
        let node = row.text(name)?;
        names.define(&row, &node, "node")?;
        let role = match row.text(kind)?.to_ascii_uppercase().as_str() {
            "STOP" => Some(ZoneRole::Stop),
            "RESET" => Some(ZoneRole::Reset),
            // A plain node may carry a zone name; only stop and reset nodes delimit zones.
            "NONE" => None,
            _ => return Err(row.error("ZCU_TYPE must be STOP, RESET or NONE")),
        };
        let zone = match role {
            None => None,
            Some(role) => {
                let zone_name = row
                    .opt_text(zone)
                    .ok_or_else(|| row.error("a stop or reset node needs its ZCU"))?;
                Some((zones.get(&row, &zone_name, "ZCU")?, role))
            }
        };
        nodes.push(RailNode {
            name: node,
            x: row.number(x)?,
            y: row.number(y)?,
            zone,
        });
    }
    Ok((nodes, names))
}

fn read_links(
    table: &Table,
    nodes: &mut [RailNode],
    node_names: &Names,
    bays: &Bays,
) -> Result<(Vec<RailLink>, Names), Error> {
    let [name, bay, from, to, speed, curve] = table.cols([
        "NAME",
        "BAY_NAME",
        "FROM_NODE",
        "TO_NODE",
        "MAX_SPEED",
        "CURVE",
    ])?;
    let (mut links, mut names) = (Vec::new(), Names::default());
    let mut curves = Vec::new();
    for row in table.rows() {
        let link = row.text(name)?;
        names.define(&row, &link, "rail")?;
        let (from_id, to_id) = (
            node_names.get(&row, &row.text(from)?, "node")?,
            node_names.get(&row, &row.text(to)?, "node")?,
        );
        if from_id == to_id {
            return Err(row.error("a rail must join two nodes"));
        }
        let (a, b) = (&nodes[from_id], &nodes[to_id]);
        let chord = (b.x - a.x).hypot(b.y - a.y);
        if chord <= 0.0 {
            return Err(row.error("a rail must have a length"));
        }
        if row.flag(curve)? {
            curves.push((links.len(), row));
        }
        links.push(RailLink {
            name: link,
            from: from_id,
            to: to_id,
            bay: bays.names.get(&row, &row.text(bay)?, "bay")?,
            // MAX_SPEED is in m/s (the SMAT2022 simulator multiplies it by 1,000).
            max_speed: row.positive(speed, "MAX_SPEED")? * 1_000.0,
            length: chord,
            arc: None,
            zone: None,
        });
    }
    let mut incoming: Vec<Vec<LinkId>> = vec![Vec::new(); nodes.len()];
    let mut outgoing: Vec<Vec<LinkId>> = vec![Vec::new(); nodes.len()];
    for (id, link) in links.iter().enumerate() {
        incoming[link.to].push(id);
        outgoing[link.from].push(id);
    }
    for (id, row) in curves {
        let arc =
            arc(&links, id, nodes, &incoming, &outgoing).map_err(|message| row.error(message))?;
        links[id].length = arc.radius * arc.sweep.abs();
        links[id].arc = Some(arc);
    }
    Ok((links, names))
}

/// Arc of curve `id`, as the SMAT2022 simulator draws it: radius = the x distance of its ends,
/// center on the side the rail turns to (seen from the rail entering its start, else from the
/// rail leaving its end), the shorter arc.
fn arc(
    links: &[RailLink],
    id: LinkId,
    nodes: &[RailNode],
    incoming: &[Vec<LinkId>],
    outgoing: &[Vec<LinkId>],
) -> Result<Arc, String> {
    let link = &links[id];
    let (a, b) = (&nodes[link.from], &nodes[link.to]);
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let chord = dx.hypot(dy);
    let radius = dx.abs();
    if 2.0 * radius < chord {
        return Err(format!(
            "curve radius {radius:.0} mm (the x distance of its ends) is shorter than half its chord"
        ));
    }
    let direction = |link: LinkId| {
        let link = &links[link];
        let (from, to) = (&nodes[link.from], &nodes[link.to]);
        (to.x - from.x, to.y - from.y)
    };
    // Turn: + left (counterclockwise), − right.
    let cross = |(ux, uy): (f64, f64), (vx, vy): (f64, f64)| ux * vy - uy * vx;
    let before = incoming[link.from]
        .iter()
        .map(|&other| cross(direction(other), (dx, dy)))
        .find(|turn| turn.abs() > 1e-9 * chord * chord);
    let after = || {
        outgoing[link.to]
            .iter()
            .map(|&other| cross((dx, dy), direction(other)))
            .find(|turn| turn.abs() > 1e-9 * chord * chord)
    };
    let turn = before
        .or_else(after)
        .ok_or("the curve continues its neighbors straight")?;
    let half = 0.5 * chord;
    let offset = (radius * radius - half * half).max(0.0).sqrt();
    // Unit normal on the left of the chord.
    let (nx, ny) = (-dy / chord, dx / chord);
    let side = turn.signum();
    let (cx, cy) = (
        a.x + 0.5 * dx + side * offset * nx,
        a.y + 0.5 * dy + side * offset * ny,
    );
    let sweep = side * 2.0 * libm::asin((half / radius).min(1.0));
    Ok(Arc {
        cx,
        cy,
        radius,
        start: libm::atan2(a.y - cy, a.x - cx),
        sweep,
    })
}

/// Every node reaches every other one: vehicles reach every port from anywhere.
fn check_connected(nodes: &[RailNode], links: &[RailLink]) -> Result<(), Error> {
    let mut forward = vec![Vec::new(); nodes.len()];
    let mut backward = vec![Vec::new(); nodes.len()];
    for link in links {
        forward[link.from].push(link.to);
        backward[link.to].push(link.from);
    }
    for adjacent in [&forward, &backward] {
        let mut seen = vec![false; nodes.len()];
        let mut stack = vec![0];
        seen[0] = true;
        while let Some(node) = stack.pop() {
            for &next in &adjacent[node] {
                if !seen[next] {
                    seen[next] = true;
                    stack.push(next);
                }
            }
        }
        if let Some(lost) = seen.iter().position(|&seen| !seen) {
            return Err(error(format!(
                "Rail: node {} is cut off from node {}: the network must be strongly connected",
                nodes[lost].name, nodes[0].name
            )));
        }
    }
    Ok(())
}

/// The region of each zone: the rails from its stop nodes up to its reset nodes. A vehicle needs
/// the zone from its stop node until it passes a reset node, so regions must be entered at stop
/// nodes only, lead to reset nodes without loops and stay apart. Merges are left to the zones
/// (vehicles only see those ahead on their own path), so each merging node lies in one region.
fn derive_zones(zones: &[String], nodes: &[RailNode], links: &mut [RailLink]) -> Result<(), Error> {
    let mut outgoing: Vec<Vec<LinkId>> = vec![Vec::new(); nodes.len()];
    let mut incoming: Vec<Vec<LinkId>> = vec![Vec::new(); nodes.len()];
    for (id, link) in links.iter().enumerate() {
        outgoing[link.from].push(id);
        incoming[link.to].push(id);
    }
    // Zone whose region a node lies inside (neither its stop nor its reset node).
    let mut interior: Vec<Option<ZoneId>> = vec![None; nodes.len()];
    for (zone, name) in zones.iter().enumerate() {
        let role_of = |node: NodeId| nodes[node].zone.filter(|&(z, _)| z == zone).map(|(_, r)| r);
        let stops: Vec<NodeId> = (0..nodes.len())
            .filter(|&node| role_of(node) == Some(ZoneRole::Stop))
            .collect();
        let resets: HashSet<NodeId> = (0..nodes.len())
            .filter(|&node| role_of(node) == Some(ZoneRole::Reset))
            .collect();
        if stops.is_empty() || resets.is_empty() {
            return Err(error(format!("ZCU {name} needs stop and reset nodes")));
        }
        let mut reached = HashSet::new();
        let mut region = Vec::new();
        let mut frontier: VecDeque<NodeId> = stops.iter().copied().collect();
        while let Some(node) = frontier.pop_front() {
            for &link in &outgoing[node] {
                if let Some(other) = links[link].zone {
                    return Err(error(format!(
                        "rail {} lies in the regions of ZCUs {} and {name}",
                        links[link].name, zones[other]
                    )));
                }
                links[link].zone = Some(zone);
                let next = links[link].to;
                if resets.contains(&next) {
                    reached.insert(next);
                    continue;
                }
                if nodes[next].zone.is_some() {
                    return Err(error(format!(
                        "ZCU {name} reaches node {} of another zone before a reset node",
                        nodes[next].name
                    )));
                }
                match interior[next] {
                    Some(other) if other != zone => {
                        return Err(error(format!(
                            "node {} lies in the regions of ZCUs {} and {name}",
                            nodes[next].name, zones[other]
                        )));
                    }
                    Some(_) => {}
                    None => {
                        interior[next] = Some(zone);
                        region.push(next);
                        frontier.push_back(next);
                    }
                }
            }
        }
        if reached.len() != resets.len() {
            return Err(error(format!(
                "ZCU {name} has reset nodes its stop nodes do not reach"
            )));
        }
        // Entered at stop nodes only: whatever leads into the region comes from inside it.
        for &node in region.iter().chain(&resets) {
            for &link in &incoming[node] {
                let from = links[link].from;
                if interior[from] != Some(zone) && role_of(from) != Some(ZoneRole::Stop) {
                    return Err(error(format!(
                        "rail {} enters ZCU {name} without passing a stop node",
                        links[link].name
                    )));
                }
            }
        }
        // No loops inside: every interior node is left for good (topological order exists).
        let mut indegree: HashMap<NodeId, usize> = region
            .iter()
            .map(|&node| {
                let inside = incoming[node]
                    .iter()
                    .filter(|&&link| interior[links[link].from] == Some(zone))
                    .count();
                (node, inside)
            })
            .collect();
        let mut ready: Vec<NodeId> = region
            .iter()
            .copied()
            .filter(|node| indegree[node] == 0)
            .collect();
        let mut ordered = 0;
        while let Some(node) = ready.pop() {
            ordered += 1;
            for &link in &outgoing[node] {
                let next = links[link].to;
                if let Some(count) = indegree.get_mut(&next) {
                    *count -= 1;
                    if *count == 0 {
                        ready.push(next);
                    }
                }
            }
        }
        if ordered != region.len() {
            return Err(error(format!("the region of ZCU {name} has a loop")));
        }
    }
    // Vehicles from different rails meet at a merging node only one zone holder at a time: the
    // rails into it lie in the region of one zone.
    for (node, rails) in incoming.iter().enumerate() {
        let zone = rails.first().and_then(|&link| links[link].zone);
        if rails.len() > 1 && (zone.is_none() || rails.iter().any(|&link| links[link].zone != zone))
        {
            return Err(error(format!(
                "the rails into merging node {} must lie in the region of one ZCU",
                nodes[node].name
            )));
        }
    }
    Ok(())
}

/// The placement of the dataset's tools (tool group by tool group), stockers, commit and
/// complete stations, and the tool groups without equipment.
struct Equipment {
    tools: Vec<Option<Station>>,
    stockers: Vec<Station>,
    commits: Vec<Station>,
    completes: Vec<Station>,
    skipped: Vec<ToolGroupId>,
    /// Equipment name → what it is.
    names: HashMap<String, Placed>,
}

#[derive(Clone, Copy)]
enum Placed {
    Tool(usize),
    Stocker(usize),
    Commit(usize),
    Complete(usize),
}

fn read_equipment(table: &Table, data: &Dataset, bays: &Bays) -> Result<Equipment, Error> {
    let [name, group, bay, width, height, x, y] = table.cols([
        "NAME",
        "TOOL_GROUP",
        "BAY",
        "WIDTH",
        "HEIGHT",
        "POSITION_X",
        "POSITION_Y",
    ])?;
    // Tool group names as the dataset spells them; SMAT2022 writes DefMEt_FE_118 as
    // DefMet_FE_118, so a name differing only in case matches if it is unique so.
    let exact: HashMap<&str, ToolGroupId> = data
        .tool_groups
        .iter()
        .enumerate()
        .map(|(id, group)| (group.name.as_str(), id))
        .collect();
    let mut folded: HashMap<String, Option<ToolGroupId>> = HashMap::new();
    for (id, group) in data.tool_groups.iter().enumerate() {
        folded
            .entry(group.name.to_ascii_lowercase())
            .and_modify(|entry| *entry = None)
            .or_insert(Some(id));
    }
    let mut per_group: Vec<Vec<Station>> = data.tool_groups.iter().map(|_| Vec::new()).collect();
    let (mut stockers, mut commits, mut completes) = (Vec::new(), Vec::new(), Vec::new());
    let mut seen = HashSet::new();
    let mut kinds = Vec::new();
    for row in table.rows() {
        let station_name = row.text(name)?;
        if !seen.insert(station_name.clone()) {
            return Err(row.error(format_args!("duplicate equipment {station_name}")));
        }
        let station = Station {
            name: station_name.clone(),
            bay: bays.names.get(&row, &row.text(bay)?, "bay")?,
            x: row.number(x)?,
            y: row.number(y)?,
            width: row.positive(width, "WIDTH")?,
            height: row.positive(height, "HEIGHT")?,
        };
        let group_name = row.text(group)?;
        let kind = match group_name.as_str() {
            COMMIT => {
                commits.push(station);
                Kind::Commit(commits.len() - 1)
            }
            COMPLETE => {
                completes.push(station);
                Kind::Complete(completes.len() - 1)
            }
            STOCKER => {
                stockers.push(station);
                Kind::Stocker(stockers.len() - 1)
            }
            _ => {
                let id = exact
                    .get(group_name.as_str())
                    .copied()
                    .or_else(|| {
                        folded
                            .get(&group_name.to_ascii_lowercase())
                            .copied()
                            .flatten()
                    })
                    .ok_or_else(|| row.error(format_args!("unknown tool group {group_name}")))?;
                per_group[id].push(station);
                Kind::Tool(id, per_group[id].len() - 1)
            }
        };
        kinds.push((station_name, kind));
    }
    enum Kind {
        Tool(ToolGroupId, usize),
        Stocker(usize),
        Commit(usize),
        Complete(usize),
    }
    let mut skipped = Vec::new();
    let mut first_tool = Vec::with_capacity(per_group.len());
    let mut tools = Vec::new();
    for (id, (spec, stations)) in data.tool_groups.iter().zip(per_group).enumerate() {
        first_tool.push(tools.len());
        if stations.is_empty() {
            skipped.push(id);
            // The skipped group's tool numbers are kept free in the layout.
            tools.extend((0..spec.tools).map(|_| None));
            continue;
        }
        if stations.len() != spec.tools as usize {
            return Err(error(format!(
                "Equipment: tool group {} has {} tools, the dataset {}",
                spec.name,
                stations.len(),
                spec.tools
            )));
        }
        tools.extend(stations.into_iter().map(Some));
    }
    check_skipped(data, &skipped)?;
    let names = kinds
        .into_iter()
        .map(|(name, kind)| {
            let placed = match kind {
                Kind::Tool(group, index) => Placed::Tool(first_tool[group] + index),
                Kind::Stocker(index) => Placed::Stocker(index),
                Kind::Commit(index) => Placed::Commit(index),
                Kind::Complete(index) => Placed::Complete(index),
            };
            (name, placed)
        })
        .collect();
    if commits.is_empty() || completes.is_empty() {
        return Err(error(
            "Equipment: the fab needs COMMIT and COMPLETE stations",
        ));
    }
    Ok(Equipment {
        tools,
        stockers,
        commits,
        completes,
        skipped,
        names,
    })
}

/// Steps of tool groups without equipment are left out like unsampled steps: no CQT segment may
/// begin or end at one, and none may batch, run setups, dedicate or rework.
fn check_skipped(data: &Dataset, skipped: &[ToolGroupId]) -> Result<(), Error> {
    for &group in skipped {
        let spec = &data.tool_groups[group];
        if spec.batching.is_some() || matches!(spec.rule, crate::data::Rule::SetupRun(_)) {
            return Err(error(format!(
                "Equipment: tool group {} has no tools but batches or runs setups",
                spec.name
            )));
        }
    }
    for route in &data.routes {
        for (index, step) in route.steps.iter().enumerate() {
            let skips = |step: usize| skipped.contains(&route.steps[step].tool_group);
            let refers = step.cqt.is_some_and(|cqt| skips(index) || skips(cqt.until))
                || step.dedicate_to.is_some_and(|to| skips(index) || skips(to))
                || step
                    .rework
                    .is_some_and(|rework| skips(index) || skips(rework.to));
            if refers {
                return Err(error(format!(
                    "Equipment: step {} of route {} needs a tool group without tools",
                    step.name, route.name
                )));
            }
        }
    }
    Ok(())
}

fn read_ports(
    types: &Table,
    table: &Table,
    equipment: &Equipment,
    links: &[RailLink],
    link_names: &Names,
) -> Result<Vec<Port>, Error> {
    #[derive(Clone, Copy, PartialEq)]
    enum Type {
        Equipment,
        Commit,
        Complete,
        Buffer(BufferSide),
    }
    let [name, inout] = types.cols(["NAME", "INOUT_TYPE"])?;
    let mut port_types = HashMap::new();
    for row in types.rows() {
        let type_name = row.text(name)?;
        let (kind, expected) = match type_name.as_str() {
            "COMMIT" => (Type::Commit, "OUT"),
            "COMPLETE" => (Type::Complete, "IN"),
            "EQPPort" => (Type::Equipment, "BOTH"),
            "LSTB" => (Type::Buffer(BufferSide::Left), "BOTH"),
            "RSTB" => (Type::Buffer(BufferSide::Right), "BOTH"),
            "UTB" => (Type::Buffer(BufferSide::Under), "BOTH"),
            _ => return Err(row.error(format_args!("unsupported port type {type_name}"))),
        };
        if row.text(inout)?.to_ascii_uppercase() != expected {
            return Err(row.error(format_args!(
                "{type_name} ports must be INOUT_TYPE {expected}"
            )));
        }
        if port_types.insert(type_name.clone(), kind).is_some() {
            return Err(row.error(format_args!("duplicate port type {type_name}")));
        }
    }
    let [name, kind, owner, link, offset] =
        table.cols(["NAME", "PORT_TYPE", "EQP_NAME", "RAILLINE_NAME", "DISTANCE"])?;
    let mut ports = Vec::new();
    let mut names = Names::default();
    let mut served = vec![false; equipment.tools.len()];
    let mut commits = vec![false; equipment.commits.len()];
    let mut completes = vec![false; equipment.completes.len()];
    for row in table.rows() {
        let port = row.text(name)?;
        names.define(&row, &port, "port")?;
        let type_name = row.text(kind)?;
        let port_type = *port_types
            .get(&type_name)
            .ok_or_else(|| row.error(format_args!("unknown port type {type_name}")))?;
        let placed = match row.opt_text(owner) {
            None => None,
            Some(owner) => Some(
                *equipment
                    .names
                    .get(&owner)
                    .ok_or_else(|| row.error(format_args!("unknown equipment {owner}")))?,
            ),
        };
        let kind = match (port_type, placed) {
            (Type::Equipment, Some(Placed::Tool(tool))) => {
                served[tool] = true;
                PortKind::Tool(tool)
            }
            (Type::Equipment, Some(Placed::Stocker(stocker))) => PortKind::Stocker(stocker),
            (Type::Commit, Some(Placed::Commit(commit))) => {
                commits[commit] = true;
                PortKind::Commit(commit)
            }
            (Type::Complete, Some(Placed::Complete(complete))) => {
                completes[complete] = true;
                PortKind::Complete(complete)
            }
            (Type::Buffer(side), None) => PortKind::Buffer(side),
            _ => return Err(row.error("the port type does not fit its equipment")),
        };
        let link = link_names.get(&row, &row.text(link)?, "rail")?;
        let position = row.number(offset)?;
        if !(0.0..=links[link].length).contains(&position) {
            return Err(row.error(format_args!(
                "DISTANCE {position} lies off rail {} ({:.0} mm)",
                links[link].name, links[link].length
            )));
        }
        ports.push(Port {
            name: port,
            kind,
            link,
            offset: position,
        });
    }
    let unserved = equipment
        .tools
        .iter()
        .zip(&served)
        .find_map(|(tool, &served)| tool.as_ref().filter(|_| !served));
    if let Some(tool) = unserved {
        return Err(error(format!("Port: tool {} has no port", tool.name)));
    }
    if commits.contains(&false) || completes.contains(&false) {
        return Err(error(
            "Port: every COMMIT and COMPLETE station needs a port",
        ));
    }
    Ok(ports)
}

fn read_vehicle_types(table: &Table) -> Result<Vec<VehicleType>, Error> {
    let [name, size, speed, acceleration, deceleration, gap] = table.cols([
        "NAME",
        "SIZE",
        "MAX_SPEED",
        "ACCELERATION",
        "DECELERATION",
        "MIN_DISTANCE",
    ])?;
    let mut types = Vec::new();
    let mut names = Names::default();
    for row in table.rows() {
        let kind = row.text(name)?;
        names.define(&row, &kind, "vehicle type")?;
        let min_gap = row.number(gap)?;
        if min_gap < 0.0 {
            return Err(row.error("MIN_DISTANCE must not be negative"));
        }
        types.push(VehicleType {
            name: kind,
            length: row.positive(size, "SIZE")?,
            max_speed: row.positive(speed, "MAX_SPEED")?,
            acceleration: row.positive(acceleration, "ACCELERATION")?,
            deceleration: row.positive(deceleration, "DECELERATION")?,
            min_gap,
        });
    }
    if types.is_empty() {
        return Err(error("VehicleType: no vehicle type"));
    }
    Ok(types)
}

fn read_vehicles(
    table: &Table,
    types: &[VehicleType],
    links: &[RailLink],
    link_names: &Names,
) -> Result<Vec<Vehicle>, Error> {
    let [name, kind, link, offset] = table.cols(["NAME", "TYPE", "RAILLINE_NAME", "DISTANCE"])?;
    let mut vehicles = Vec::new();
    let mut names = Names::default();
    for row in table.rows() {
        let vehicle = row.text(name)?;
        names.define(&row, &vehicle, "vehicle")?;
        let type_name = row.text(kind)?;
        let kind = types
            .iter()
            .position(|kind| kind.name == type_name)
            .ok_or_else(|| row.error(format_args!("unknown vehicle type {type_name}")))?;
        let link = link_names.get(&row, &row.text(link)?, "rail")?;
        let position = row.number(offset)?;
        if !(0.0..=links[link].length).contains(&position) {
            return Err(row.error(format_args!("DISTANCE {position} lies off its rail")));
        }
        vehicles.push(Vehicle {
            name: vehicle,
            kind,
            link,
            offset: position,
        });
    }
    if vehicles.is_empty() {
        return Err(error("Vehicle: no vehicle"));
    }
    Ok(vehicles)
}

/// Vehicles at time 0 keep their least distance to the vehicle ahead on every way on, a body
/// reaching back over a node has a single rail behind it, and no two share a zone's region.
fn check_vehicles(layout: &Layout) -> Result<(), Error> {
    let mut incoming: Vec<Vec<LinkId>> = vec![Vec::new(); layout.nodes.len()];
    let mut outgoing: Vec<Vec<LinkId>> = vec![Vec::new(); layout.nodes.len()];
    for (id, link) in layout.links.iter().enumerate() {
        incoming[link.to].push(id);
        outgoing[link.from].push(id);
    }
    // Vehicles per rail, rearmost first.
    let mut on_link: Vec<Vec<usize>> = vec![Vec::new(); layout.links.len()];
    for (id, vehicle) in layout.vehicles.iter().enumerate() {
        on_link[vehicle.link].push(id);
    }
    for list in &mut on_link {
        list.sort_by(|&a, &b| {
            layout.vehicles[a]
                .offset
                .total_cmp(&layout.vehicles[b].offset)
        });
    }
    let spec = |vehicle: usize| &layout.vehicle_types[layout.vehicles[vehicle].kind];
    let reach = layout
        .vehicle_types
        .iter()
        .map(|kind| kind.length + kind.min_gap)
        .fold(0.0, f64::max);
    let mut holders: HashMap<ZoneId, usize> = HashMap::new();
    for (id, vehicle) in layout.vehicles.iter().enumerate() {
        let kind = spec(id);
        // The body behind the front.
        let (mut link, mut left) = (vehicle.link, kind.length - vehicle.offset);
        while left > 0.0 {
            let &[behind] = &incoming[layout.links[link].from][..] else {
                return Err(error(format!(
                    "Vehicle {} reaches back over a merge",
                    vehicle.name
                )));
            };
            link = behind;
            left -= layout.links[link].length;
        }
        if let Some(zone) = layout.links[vehicle.link].zone
            && let Some(other) = holders.insert(zone, id)
        {
            return Err(error(format!(
                "Vehicles {} and {} start in the region of ZCU {}",
                layout.vehicles[other].name, vehicle.name, layout.zones[zone]
            )));
        }
        // The nearest vehicle ahead on every way on.
        let mut ways = vec![(vehicle.link, -vehicle.offset)];
        while let Some((link, start)) = ways.pop() {
            let ahead = on_link[link].iter().copied().find(|&other| {
                other != id
                    && (link != vehicle.link || layout.vehicles[other].offset > vehicle.offset)
            });
            match ahead {
                Some(other) => {
                    let gap = start + layout.vehicles[other].offset - spec(other).length;
                    if gap < kind.min_gap - 1e-6 {
                        return Err(error(format!(
                            "Vehicles {} and {} start {gap:.0} mm apart, closer than {:.0} mm",
                            vehicle.name, layout.vehicles[other].name, kind.min_gap
                        )));
                    }
                }
                None => {
                    let end = start + layout.links[link].length;
                    if end < reach {
                        ways.extend(
                            outgoing[layout.links[link].to]
                                .iter()
                                .map(|&next| (next, end)),
                        );
                    }
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::data::tiny;

    fn sheet(name: &str, rows: &[&[&str]]) -> Sheet {
        Sheet {
            name: name.into(),
            rows: rows
                .iter()
                .map(|row| {
                    row.iter()
                        .map(|&cell| match cell.parse::<f64>() {
                            Ok(number) => Cell::Number(number),
                            Err(_) if cell.is_empty() => Cell::Empty,
                            Err(_) => Cell::Text(cell.into()),
                        })
                        .collect()
                })
                .collect(),
        }
    }

    /// A square loop of 4 m sides, counterclockwise, with a shortcut from the middle of the
    /// bottom to the middle of the top merging there under a ZCU; tiny()'s two tools in bay A1.
    pub(crate) fn loop_sheets() -> Vec<Sheet> {
        vec![
            sheet(
                "Bay",
                &[
                    &["NAME", "TYPE", "RETICLE", "NEIGHBOR_BAY"],
                    &["A1", "INTRABAY", "0", ""],
                ],
            ),
            sheet("ZCU", &[&["NAME"], &["Z1"], &["Z2"]]),
            sheet(
                "Address",
                &[
                    &["NAME", "POSITION_X", "POSITION_Y", "ZCU_NAME", "ZCU_TYPE"],
                    &["n0", "0", "0", ",", "NONE"],
                    &["n1", "2000", "0", "Z2", "STOP"],
                    &["n2", "4000", "0", ",", "NONE"],
                    &["n3", "4000", "4000", ",", "NONE"],
                    &["n4", "2000", "4000", "Z1", "RESET"],
                    &["n5", "0", "4000", ",", "NONE"],
                    &["n6", "3000", "4000", "Z1", "STOP"],
                    &["n7", "2000", "3000", "Z1", "STOP"],
                    &["n8", "2000", "1000", "Z2", "RESET"],
                    &["n9", "3000", "0", "Z2", "RESET"],
                ],
            ),
            sheet(
                "Rail",
                &[
                    &[
                        "NAME",
                        "BAY_NAME",
                        "FROM_NODE",
                        "TO_NODE",
                        "MAX_SPEED",
                        "CURVE",
                    ],
                    &["r0", "A1", "n0", "n1", "1", "0"],
                    &["r1", "A1", "n1", "n9", "1", "0"],
                    &["r9", "A1", "n9", "n2", "1", "0"],
                    &["r2", "A1", "n2", "n3", "1", "0"],
                    &["r3", "A1", "n3", "n6", "1", "0"],
                    &["r6", "A1", "n6", "n4", "1", "0"],
                    &["r4", "A1", "n4", "n5", "1", "0"],
                    &["r5", "A1", "n5", "n0", "1", "0"],
                    &["s1", "A1", "n1", "n8", "1", "0"],
                    &["s8", "A1", "n8", "n7", "1", "0"],
                    &["s7", "A1", "n7", "n4", "1", "0"],
                ],
            ),
            sheet(
                "Equipment",
                &[
                    &[
                        "NAME",
                        "TOOL_GROUP",
                        "BAY",
                        "WIDTH",
                        "HEIGHT",
                        "POSITION_X",
                        "POSITION_Y",
                    ],
                    &["ETCH1", "etch_1", "A1", "500", "500", "1000", "-1000"],
                    &["ETCH2", "Etch_1", "A1", "500", "500", "3000", "-1000"],
                    &["IN", "COMMIT", "A1", "500", "500", "-1000", "2000"],
                    &["OUT", "COMPLETE", "A1", "500", "500", "5000", "2000"],
                ],
            ),
            sheet(
                "PortType",
                &[
                    &["NAME", "SIZE", "INOUT_TYPE"],
                    &["COMMIT", "314", "OUT"],
                    &["COMPLETE", "314", "IN"],
                    &["EQPPort", "314", "BOTH"],
                    &["LSTB", "314", "BOTH"],
                ],
            ),
            sheet(
                "Port",
                &[
                    &["NAME", "PORT_TYPE", "EQP_NAME", "RAILLINE_NAME", "DISTANCE"],
                    &["E1", "EQPPort", "ETCH1", "r0", "1000"],
                    &["E2", "EQPPort", "ETCH2", "r9", "500"],
                    &["B1", "LSTB", ",", "r2", "2000"],
                    &["B2", "LSTB", ",", "r4", "1000"],
                    &["C1", "COMMIT", "IN", "r5", "2000"],
                    &["D1", "COMPLETE", "OUT", "r3", "1000"],
                ],
            ),
            sheet(
                "VehicleType",
                &[
                    &[
                        "NAME",
                        "SIZE",
                        "MAX_SPEED",
                        "ACCELERATION",
                        "DECELERATION",
                        "MIN_DISTANCE",
                    ],
                    &["V", "784", "1500", "2000", "3000", "125"],
                ],
            ),
            sheet(
                "Vehicle",
                &[
                    &["NAME", "TYPE", "RAILLINE_NAME", "DISTANCE"],
                    &["v0", "V", "r2", "2000"],
                    &["v1", "V", "r4", "1000"],
                ],
            ),
        ]
    }

    pub(crate) fn loop_layout() -> Layout {
        layout(&tiny(), &loop_sheets()).unwrap_or_else(|error| panic!("{error}"))
    }

    #[test]
    fn a_loop_with_a_shortcut_loads() {
        let layout = loop_layout();
        assert_eq!((layout.nodes.len(), layout.links.len()), (10, 11));
        // The group name matches in another case; the tools come in sheet order.
        let tools: Vec<_> = layout
            .tools
            .iter()
            .map(|tool| tool.as_ref().unwrap().name.as_str())
            .collect();
        assert_eq!(tools, ["ETCH1", "ETCH2"]);
        assert!(layout.skipped.is_empty());
        // Z1 covers the rails into its merge node n4, Z2 those out of its diverge node n1.
        let zone = |name: &str| {
            let link = layout.links.iter().find(|link| link.name == name).unwrap();
            link.zone.map(|zone| layout.zones[zone].as_str())
        };
        assert_eq!(
            (
                zone("r6"),
                zone("s7"),
                zone("r1"),
                zone("s1"),
                zone("r0"),
                zone("s8")
            ),
            (Some("Z1"), Some("Z1"), Some("Z2"), Some("Z2"), None, None)
        );
        assert_eq!(layout.links[0].max_speed, 1_000.0);
        assert_eq!(layout.bays[0].grid, Some(("A".into(), 1)));
        assert_eq!(layout.ports[4].kind, PortKind::Commit(0));
    }

    #[test]
    fn curves_take_the_simulators_arc() {
        let mut sheets = loop_sheets();
        // The corner n2 → n3 as a curve: radius = |dx| = 4 m would need dy ≤ 2·r.
        let rail = sheets
            .iter_mut()
            .find(|sheet| sheet.name == "Rail")
            .unwrap();
        rail.rows[4][5] = Cell::Number(1.0);
        let error = layout(&tiny(), &sheets).unwrap_err().to_string();
        assert!(error.contains("radius 0 mm"), "{error}");
        // r9 from n9 (3, 0) heading east as a left quarter turn to n2 moved to (4, 1): center
        // (3, 1).
        let mut sheets = loop_sheets();
        let address = sheets
            .iter_mut()
            .find(|sheet| sheet.name == "Address")
            .unwrap();
        address.rows[3][2] = Cell::Number(1000.0);
        let rail = sheets
            .iter_mut()
            .find(|sheet| sheet.name == "Rail")
            .unwrap();
        rail.rows[3][5] = Cell::Number(1.0);
        let layout = layout(&tiny(), &sheets).unwrap();
        let arc = layout.links[2].arc.unwrap();
        assert!((arc.radius - 1000.0).abs() < 1e-9);
        assert!((arc.cx - 3000.0).abs() < 1e-9 && (arc.cy - 1000.0).abs() < 1e-9);
        assert!((layout.links[2].length - 1000.0 * std::f64::consts::FRAC_PI_2).abs() < 1e-9);
        let (x, y, heading) = layout.point(2, layout.links[2].length);
        assert!((x - 4000.0).abs() < 1e-6 && (y - 1000.0).abs() < 1e-6);
        assert!((heading - std::f64::consts::FRAC_PI_2).abs() < 1e-9);
    }

    #[test]
    fn invalid_layouts_are_rejected() {
        let check = |edit: &dyn Fn(&mut Vec<Sheet>), expected: &str| {
            let mut sheets = loop_sheets();
            edit(&mut sheets);
            let message = layout(&tiny(), &sheets).unwrap_err().to_string();
            assert!(message.contains(expected), "{message} lacks {expected}");
        };
        fn row<'s>(sheets: &'s mut [Sheet], name: &str, row: usize) -> &'s mut Vec<Cell> {
            &mut sheets
                .iter_mut()
                .find(|sheet| sheet.name == name)
                .unwrap()
                .rows[row]
        }
        check(
            &|s| row(s, "Rail", 1)[3] = Cell::Text("n9x".into()),
            "unknown node n9x",
        );
        // A stop node without its reset: Z2's resets dropped.
        check(
            &|s| {
                row(s, "Address", 9)[4] = Cell::Text("NONE".into());
                row(s, "Address", 10)[4] = Cell::Text("NONE".into());
            },
            "ZCU Z2 needs stop and reset nodes",
        );
        // n6 no longer a stop node: r6 enters Z1 unchecked.
        check(
            &|s| row(s, "Address", 7)[4] = Cell::Text("NONE".into()),
            "rail r6 enters ZCU Z1 without passing a stop node",
        );
        // Z1 dropped: nothing keeps vehicles from r6 and s7 apart where they merge at n4.
        check(
            &|s| {
                for line in [5, 7, 8] {
                    row(s, "Address", line)[4] = Cell::Text("NONE".into());
                }
                s.iter_mut()
                    .find(|sheet| sheet.name == "ZCU")
                    .unwrap()
                    .rows
                    .remove(1);
            },
            "the rails into merging node n4 must lie in the region of one ZCU",
        );
        check(
            &|s| row(s, "Equipment", 2)[0] = Cell::Text("ETCH1".into()),
            "duplicate equipment",
        );
        check(
            &|s| {
                s.iter_mut()
                    .find(|sheet| sheet.name == "Equipment")
                    .unwrap()
                    .rows
                    .remove(2);
                s.iter_mut()
                    .find(|sheet| sheet.name == "Port")
                    .unwrap()
                    .rows
                    .remove(2);
            },
            "tool group Etch_1 has 1 tools, the dataset 2",
        );
        check(
            &|s| row(s, "Port", 1)[4] = Cell::Number(9000.0),
            "lies off rail r0",
        );
        check(
            &|s| row(s, "Port", 3)[2] = Cell::Text("ETCH1".into()),
            "does not fit",
        );
        check(
            &|s| {
                row(s, "Vehicle", 2)[2] = Cell::Text("r2".into());
                row(s, "Vehicle", 2)[3] = Cell::Number(1500.0);
            },
            "closer than 125 mm",
        );
        // r3 led to n0 instead: nothing reaches n6.
        check(
            &|s| row(s, "Rail", 5)[3] = Cell::Text("n0".into()),
            "node n6 is cut off",
        );
    }
}
