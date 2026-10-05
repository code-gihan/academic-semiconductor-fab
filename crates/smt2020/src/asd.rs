//! Loader for SMT2020 AutoSched AP model directories (`*.asd`). Reads the files named in
//! `options.def` and returns a validated [`Dataset`]; input the model does not support is an
//! error, never silently dropped. Label-only columns (lot, order and step descriptions, IGNORE)
//! are not read into the dataset; [`orders`] reads the order labels the AutoSched reports use.

mod table;

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::path::Path;

use des_core::Time;

use crate::data::{
    BatchCriterion, BatchSize, Breakdown, Cqt, Dataset, LotRelease, Part, Period, Pm, PmTrigger,
    Rank, ReleaseStream, Rework, Route, Rule, SetupChange, SetupGroup, Step, StepIndex, StepSetup,
    ToolGroup, Transport, Unit,
};
use table::{COMMENT, Row, Table, civil_ms, read_text};

#[derive(Debug)]
pub struct Error(String);

impl Error {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

/// Step action list of every SMT2020 tool group; the step model follows its sequence
/// (skip, travel, setup, load, process, unload, rework).
const ACTION_LIST: &str = "Custom_actlist_ASISemiOpersDuringSetupAndAdditionalLoadUnload";

/// Loads the model in `dir` with the active files of its `options.def`.
pub fn load(dir: &Path) -> Result<Dataset, Error> {
    let options = Options::read(dir)?;
    let orders = options.files("ORDER_FILES")?;
    build(dir, &options, &orders)
}

/// [`load`] with the order files `orders` of `dir` in place of the active ORDER_FILES, e.g. the
/// inactive periodic `order.txt` or another due-date list.
pub fn load_with_orders(dir: &Path, orders: &[&str]) -> Result<Dataset, Error> {
    build(dir, &Options::read(dir)?, orders)
}

/// AutoSched order: the label its reports (`order.rep`) group lots by.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Order {
    pub name: String,
    pub part: String,
    pub priority: u32,
}

/// Orders of the active ORDER_FILES of `dir` whose lots share one part and priority; orders mixing
/// them (the initial WIP) are left out.
pub fn orders(dir: &Path) -> Result<Vec<Order>, Error> {
    let options = Options::read(dir)?;
    let mut orders: Vec<(Order, bool)> = Vec::new();
    for file in options.files("ORDER_FILES")? {
        let table = Table::read(dir, file)?;
        let [order, part, priority] = table.cols(["ORDER", "PART", "PRIOR"])?;
        for row in table.rows() {
            let (name, part, priority) = (row.text(order)?, row.text(part)?, row.count(priority)?);
            match orders.iter_mut().find(|(known, _)| known.name == name) {
                Some((known, mixed)) => *mixed |= known.part != part || known.priority != priority,
                None => orders.push((
                    Order {
                        name: name.to_owned(),
                        part: part.to_owned(),
                        priority,
                    },
                    false,
                )),
            }
        }
    }
    Ok(orders
        .into_iter()
        .filter_map(|(order, mixed)| (!mixed).then_some(order))
        .collect())
}

fn build(dir: &Path, options: &Options, orders: &[&str]) -> Result<Dataset, Error> {
    let table = |key: &str| Table::read(dir, options.file(key)?);

    let mut setups = Names::default();
    let (setup_groups, setup_group_names) =
        read_setup_groups(&table("SETUPGROUP_FILES")?, &mut setups)?;
    let (mut areas, mut locations) = (Names::default(), Names::default());
    let (mut tool_groups, tool_group_names) = read_tool_groups(
        &table("STATION_FILES")?,
        &setup_group_names,
        &mut areas,
        &mut locations,
    )?;
    let setup_changes = read_setup_changes(&table("SETUP_FILES")?, &mut setups)?;
    let (parts, part_names, routes) = read_parts(
        dir,
        &table("PRODUCT_FILES")?,
        &tool_groups,
        &tool_group_names,
        &mut setups,
    )?;
    read_calendars(
        &table("DOWNCAL_FILES")?,
        &table("PMCAL_FILES")?,
        &table("ATTACH_FILES")?,
        &mut tool_groups,
        &tool_group_names,
        &areas,
    )?;
    let transports = read_transports(&table("FROMTO_FILES")?, &locations)?;
    let (streams, lots) = read_orders(dir, orders, options.epoch, &parts, &part_names, &routes)?;
    let periods = read_periods(&table("PERIOD_FILE")?, options.epoch)?;
    Ok(Dataset {
        areas: areas.names,
        locations: locations.names,
        tool_groups,
        setups: setups.names,
        setup_changes,
        setup_groups,
        routes,
        parts,
        transports,
        streams,
        lots,
        periods,
    })
}

/// Names in definition order, indexed for lookup (the map is never iterated).
#[derive(Default)]
struct Names {
    names: Vec<String>,
    index: HashMap<String, usize>,
}

impl Names {
    fn intern(&mut self, name: &str) -> usize {
        if let Some(&id) = self.index.get(name) {
            return id;
        }
        self.index.insert(name.to_owned(), self.names.len());
        self.names.push(name.to_owned());
        self.names.len() - 1
    }

    fn get(&self, name: &str) -> Option<usize> {
        self.index.get(name).copied()
    }
}

/// Adds the name in `col`, which must be new.
fn define<'a>(names: &mut Names, row: Row<'a>, col: usize, what: &str) -> Result<&'a str, Error> {
    let name = row.text(col)?;
    if names.get(name).is_some() {
        return Err(row.error(format_args!("duplicate {what} {name}")));
    }
    names.intern(name);
    Ok(name)
}

fn lookup(row: Row, col: usize, names: &Names, what: &str) -> Result<usize, Error> {
    let name = row.text(col)?;
    names
        .get(name)
        .ok_or_else(|| row.error(format_args!("unknown {what} {name}")))
}

/// `options.def`: key, then values; a line with an empty key continues the previous key.
struct Options {
    /// SIM_START as a civil ms value; dates become ms after it.
    epoch: Time,
    entries: Vec<(String, Vec<String>)>,
}

impl Options {
    fn read(dir: &Path) -> Result<Self, Error> {
        const FILE: &str = "options.def";
        let mut entries: Vec<(String, Vec<String>)> = Vec::new();
        for (index, line) in read_text(dir, FILE)?.lines().enumerate() {
            let error = |message: &str| Error::new(format!("{FILE}:{}: {message}", index + 1));
            let mut cells = line.split('\t').map(str::trim);
            let key = cells.next().unwrap_or_default();
            if key == "COMMENT_CHARACTER" {
                if cells.next() != Some(COMMENT) {
                    return Err(error("only COMMENT_CHARACTER ~ is supported"));
                }
                continue;
            }
            let values: Vec<String> = cells
                .take_while(|cell| !cell.starts_with(COMMENT))
                .filter(|cell| !cell.is_empty())
                .map(str::to_owned)
                .collect();
            let key = match (key, entries.last()) {
                ("", _) if values.is_empty() => continue,
                ("", Some((previous, _))) => previous.clone(),
                ("", None) => return Err(error("continuation line before any key")),
                (key, _) => key.to_owned(),
            };
            entries.push((key, values));
        }
        let mut options = Self { epoch: 0, entries };
        let start = options.value("SIM_START")?;
        options.epoch = civil_ms(start).ok_or_else(|| {
            Error::new(format!(
                "{FILE}: SIM_START {start} is not MM/DD/YY HH:MM:SS"
            ))
        })?;
        // The step model follows these settings.
        for (key, required) in [("SEQ_ADDS_SETUP_DELAYS", "N"), ("USE_CALENDARS(Y/N)", "Y")] {
            if options.value(key)? != required {
                return Err(Error::new(format!(
                    "{FILE}: only {key} {required} is supported"
                )));
            }
        }
        Ok(options)
    }

    fn values<'s>(&'s self, key: &str) -> impl Iterator<Item = &'s [String]> {
        self.entries
            .iter()
            .filter(move |(entry, _)| entry == key)
            .map(|(_, values)| values.as_slice())
    }

    fn value(&self, key: &str) -> Result<&str, Error> {
        let mut values = self.values(key);
        match (values.next(), values.next()) {
            (Some([value]), None) => Ok(value.as_str()),
            _ => Err(Error::new(format!(
                "options.def: {key} needs exactly one value"
            ))),
        }
    }

    /// Files under `key`; a `file` entry whose name is commented out is inactive.
    fn files(&self, key: &str) -> Result<Vec<&str>, Error> {
        let mut files = Vec::new();
        for values in self.values(key) {
            match values {
                [kind, file] if kind == "file" => files.push(file.as_str()),
                [kind] if kind == "file" || kind == "none" => {}
                _ => {
                    return Err(Error::new(format!(
                        "options.def: unsupported {key} {values:?}"
                    )));
                }
            }
        }
        Ok(files)
    }

    fn file(&self, key: &str) -> Result<&str, Error> {
        match self.files(key)?.as_slice() {
            [file] => Ok(file),
            files => Err(Error::new(format!(
                "options.def: {key} needs exactly one active file, found {}",
                files.len()
            ))),
        }
    }
}

fn read_setup_groups(table: &Table, setups: &mut Names) -> Result<(Vec<SetupGroup>, Names), Error> {
    let [group, setup, min_run] = table.cols(["SETUPGRP", "SETUP", "MINRUN"])?;
    let other_criteria: Vec<usize> = ["MINQUEUE", "MAXQUEUE", "MINNUMSTN", "MAXNUMSTN"]
        .into_iter()
        .filter_map(|name| table.opt_col(name))
        .collect();
    let (mut groups, mut names) = (Vec::<SetupGroup>::new(), Names::default());
    for row in table.rows() {
        if row.opt(group).is_some() {
            let name = define(&mut names, row, group, "setup group")?;
            groups.push(SetupGroup {
                name: name.to_owned(),
                min_run: Vec::new(),
            });
        }
        if other_criteria.iter().any(|&col| row.opt(col).is_some()) {
            return Err(row.error("only the MINRUN setup criterion is supported"));
        }
        let current = groups
            .last_mut()
            .ok_or_else(|| row.error("setup before any setup group"))?;
        current
            .min_run
            .push((setups.intern(row.text(setup)?), row.count(min_run)?));
    }
    Ok((groups, names))
}

fn read_tool_groups(
    table: &Table,
    setup_groups: &Names,
    areas: &mut Names,
    locations: &mut Names,
) -> Result<(Vec<ToolGroup>, Names), Error> {
    let [
        name,
        rule,
        rank,
        wake,
        criterion,
        batch_unit,
        load,
        load_unit,
        unload,
        unload_unit,
        capacity,
        quantity,
        area,
        action_list,
        location,
        pre_rule,
        setup_group,
    ] = table.cols([
        "STNFAM",
        "RULE",
        "FWLRANK",
        "WAKERESRANK",
        "BATCHCRITF",
        "BATCHPER",
        "LTIME",
        "LTUNITS",
        "ULTIME",
        "ULTUNITS",
        "STNCAP",
        "STNQTY",
        "STNGRP",
        "STNFAMSTEP_ACTLIST",
        "STNFAMLOC",
        "PRERULERWL",
        "SETUPGRP",
    ])?;
    let (mut groups, mut names) = (Vec::<ToolGroup>::new(), Names::default());
    for row in table.rows() {
        if row.opt(name).is_none() {
            // Continuation row: the next ranking criterion of the previous tool group.
            row.only(&[rank])?;
            let previous = groups
                .last_mut()
                .ok_or_else(|| row.error("ranking row before any tool group"))?;
            previous.ranks.push(parse_rank(row, rank)?);
            continue;
        }
        let group_name = define(&mut names, row, name, "tool group")?;
        if row.text(action_list)? != ACTION_LIST {
            return Err(row.error("unsupported STNFAMSTEP_ACTLIST"));
        }
        if row.flag(pre_rule)? {
            return Err(row.error("PRERULERWL = yes is not supported"));
        }
        let batching = match (row.opt(batch_unit), row.opt(criterion)) {
            (None, None) => None,
            (Some("piece"), Some("crit_sameroutestep")) => Some(BatchCriterion::SameRouteStep),
            (Some("piece"), Some("crit_samepartfam|crit_samestepname")) => {
                Some(BatchCriterion::SameFamilyStepName)
            }
            _ => return Err(row.error("unsupported batching BATCHPER/BATCHCRITF")),
        };
        let cascading = match row.opt(capacity) {
            None | Some("1") => false,
            Some("2") => true,
            Some(_) => return Err(row.error("STNCAP must be 1 or 2")),
        };
        let dispatch = match (row.text(rule)?, row.opt(setup_group)) {
            ("rule_HotLotFIRST", None) => Rule::HotLotFirst,
            ("rule_LSSU", Some(_)) => {
                Rule::SetupRun(lookup(row, setup_group, setup_groups, "setup group")?)
            }
            _ => return Err(row.error("unsupported RULE/SETUPGRP combination")),
        };
        let wake_least_setup = match row.opt(wake) {
            None => false,
            Some("wake_LeastSetupTime") => true,
            Some(_) => return Err(row.error("unsupported WAKERESRANK")),
        };
        // STNQTY defaults to one tool.
        let tools = match row.opt(quantity) {
            None => 1,
            Some(_) => row.count(quantity)?,
        };
        if tools == 0 {
            return Err(row.error("STNQTY must be positive"));
        }
        groups.push(ToolGroup {
            name: group_name.to_owned(),
            area: areas.intern(row.text(area)?),
            location: locations.intern(row.text(location)?),
            tools,
            load: row.duration(load, load_unit)?,
            unload: row.duration(unload, unload_unit)?,
            cascading,
            batching,
            rule: dispatch,
            ranks: vec![parse_rank(row, rank)?],
            wake_least_setup,
            breakdowns: Vec::new(),
            pms: Vec::new(),
        });
    }
    for group in &groups {
        let distinct = group
            .ranks
            .iter()
            .enumerate()
            .all(|(index, rank)| !group.ranks[..index].contains(rank));
        let both = group.ranks.contains(&Rank::Fifo) && group.ranks.contains(&Rank::CriticalRatio);
        if !distinct || both {
            return Err(Error::new(format!(
                "{}: tool group {} repeats a rank or combines rank_FIFO with rank_CR",
                table.file(),
                group.name
            )));
        }
    }
    Ok((groups, names))
}

fn parse_rank(row: Row, col: usize) -> Result<Rank, Error> {
    match row.text(col)? {
        "rank_HP" => Ok(Rank::Priority),
        "rank_RSETUP" => Ok(Rank::LeastSetup),
        "rank_FIFO" => Ok(Rank::Fifo),
        "rank_CR" => Ok(Rank::CriticalRatio),
        other => Err(row.error(format_args!("unsupported rank {other}"))),
    }
}

fn read_setup_changes(table: &Table, setups: &mut Names) -> Result<Vec<SetupChange>, Error> {
    let [from, to, time, unit] = table.cols(["CURSETUP", "NEWSETUP", "STIME", "STUNITS"])?;
    let (kind, time2) = (table.opt_col("SDIST"), table.opt_col("STIME2"));
    let mut changes = Vec::new();
    let mut seen = HashSet::new();
    for row in table.rows() {
        let change = SetupChange {
            from: row.opt(from).map(|name| setups.intern(name)),
            to: setups.intern(row.text(to)?),
            time: row.dist(kind, time, time2, unit)?,
        };
        if !seen.insert((change.from, change.to)) {
            return Err(row.error("duplicate setup change"));
        }
        changes.push(change);
    }
    Ok(changes)
}

fn read_parts(
    dir: &Path,
    table: &Table,
    tool_groups: &[ToolGroup],
    tool_group_names: &Names,
    setups: &mut Names,
) -> Result<(Vec<Part>, Names, Vec<Route>), Error> {
    let [group, family, name, file, route] =
        table.cols(["PARTGRP", "PARTFAM", "PART", "ROUTEFILE", "ROUTE"])?;
    let (mut parts, mut names) = (Vec::new(), Names::default());
    let (mut routes, mut route_files) = (Vec::<Route>::new(), Names::default());
    for row in table.rows() {
        let part_name = define(&mut names, row, name, "part")?;
        let engineering = match row.text(group)? {
            "Saleable" => false,
            "Engineering" => true,
            _ => return Err(row.error("PARTGRP must be Saleable or Engineering")),
        };
        let (route_file, route_name) = (row.text(file)?, row.text(route)?);
        let route_id = route_files.intern(route_file);
        if route_id == routes.len() {
            routes.push(read_route(
                dir,
                route_file,
                route_name,
                tool_groups,
                tool_group_names,
                setups,
            )?);
        } else if routes[route_id].name != route_name {
            return Err(row.error(format_args!("{route_file} is used with two route names")));
        }
        parts.push(Part {
            name: part_name.to_owned(),
            family: row.text(family)?.to_owned(),
            engineering,
            route: route_id,
        });
    }
    Ok((parts, names, routes))
}

fn read_route(
    dir: &Path,
    file: &str,
    route_name: &str,
    tool_groups: &[ToolGroup],
    tool_group_names: &Names,
    setups: &mut Names,
) -> Result<Route, Error> {
    let table = Table::read(dir, file)?;
    let [
        route,
        step,
        tool_group,
        kind,
        time,
        time2,
        time_unit,
        per,
        batch_min,
        batch_max,
        setup,
        when,
        setup_time,
        setup_unit,
        dedicated,
        dedicated_step,
        lot_interval,
        lot_interval_unit,
        wafer_interval,
        wafer_interval_unit,
        rework_step,
        rework,
        rework_type,
        percent,
        cqt_step,
        cqt,
        cqt_unit,
    ] = table.cols([
        "ROUTE",
        "STEP",
        "STNFAM",
        "PDIST",
        "PTIME",
        "PTIME2",
        "PTUNITS",
        "PTPER",
        "BATCHMN",
        "BATCHMX",
        "SETUP",
        "WHEN",
        "STIME",
        "STUNITS",
        "SVESTN",
        "FORSTEP",
        "BatchInterval",
        "BatchIntUnits",
        "PartInterval",
        "PartIntUnits",
        "RWKSTEP",
        "REWORK",
        "RWKTYPE",
        "StepPercent",
        "STEP_CQT",
        "CQT",
        "CQTUNITS",
    ])?;
    // Dedication, rework and CQT refer to steps by name, including later steps.
    let mut step_names = Names::default();
    for row in table.rows() {
        define(&mut step_names, row, step, "step")?;
    }
    let mut steps = Vec::new();
    for (index, row) in table.rows().enumerate() {
        if row.text(route)? != route_name {
            return Err(row.error(format_args!("ROUTE differs from {route_name} of the part")));
        }
        let group_id = lookup(row, tool_group, tool_group_names, "tool group")?;
        let group = &tool_groups[group_id];
        let unit = match row.text(per)? {
            "per_lot" => Unit::Lot,
            "per_piece" => Unit::Wafer,
            "per_batch" => Unit::Batch,
            _ => return Err(row.error("PTPER must be per_lot, per_piece or per_batch")),
        };
        let process_time = row.dist(Some(kind), time, Some(time2), time_unit)?;
        let batch = match (row.opt(batch_min), row.opt(batch_max)) {
            (None, None) => None,
            _ => Some(BatchSize {
                min: row.count(batch_min)?,
                max: row.count(batch_max)?,
            }),
        };
        let batch_step = unit == Unit::Batch;
        if batch_step != group.batching.is_some()
            || batch_step != batch.is_some()
            || batch.is_some_and(|size| size.min == 0 || size.min > size.max)
        {
            return Err(
                row.error("per_batch steps need a batching tool group and 0 < BATCHMN <= BATCHMX")
            );
        }
        let cascade_interval = match (
            row.opt_duration(lot_interval, lot_interval_unit)?,
            row.opt_duration(wafer_interval, wafer_interval_unit)?,
            unit,
        ) {
            (None, None, _) => None,
            (Some(interval), None, Unit::Lot) | (None, Some(interval), Unit::Wafer) => {
                Some(interval)
            }
            _ => return Err(row.error("BatchInterval needs per_lot, PartInterval per_piece")),
        };
        if cascade_interval.is_some() != group.cascading
            || cascade_interval
                .is_some_and(|interval| interval == 0 || interval > process_time.min())
        {
            return Err(
                row.error("cascading tool groups need an interval in (0, minimum processing time]")
            );
        }
        let step_setup = match row.opt(setup) {
            None => {
                if [when, setup_time, setup_unit]
                    .iter()
                    .any(|&col| row.opt(col).is_some())
                {
                    return Err(row.error("WHEN/STIME without SETUP"));
                }
                None
            }
            Some(setup_name) => Some(StepSetup {
                setup: setups.intern(setup_name),
                always: match row.text(when)? {
                    "need" => false,
                    "always" => true,
                    _ => return Err(row.error("WHEN must be need or always")),
                },
                time: row.opt_duration(setup_time, setup_unit)?,
            }),
        };
        let sampling = match row.opt(percent) {
            None => 1.0,
            Some(_) => row.percent(percent)?,
        };
        let step_rework = match (row.opt(rework_step), row.opt(rework), row.opt(rework_type)) {
            (None, None, None) => None,
            (Some(_), Some(_), Some("lot")) => Some(Rework {
                probability: row.percent(rework)?,
                to: step_ref(row, rework_step, &step_names, index, false)?,
            }),
            _ => return Err(row.error("rework needs RWKSTEP, REWORK and RWKTYPE lot")),
        };
        if step_rework.is_some_and(|rework| rework.probability >= 1.0) {
            return Err(row.error("REWORK must be below 100 %, or lots loop forever"));
        }
        let dedicate_to = match (row.flag(dedicated)?, row.opt(dedicated_step)) {
            (false, None) => None,
            (true, Some(_)) => Some(step_ref(row, dedicated_step, &step_names, index, true)?),
            _ => return Err(row.error("SVESTN yes and FORSTEP go together")),
        };
        let step_cqt = match (row.opt(cqt_step), row.opt_duration(cqt, cqt_unit)?) {
            (None, None) => None,
            (Some(_), Some(limit)) => Some(Cqt {
                until: step_ref(row, cqt_step, &step_names, index, true)?,
                limit,
            }),
            _ => return Err(row.error("STEP_CQT and CQT go together")),
        };
        steps.push(Step {
            name: row.text(step)?.to_owned(),
            tool_group: group_id,
            unit,
            time: process_time,
            cascade_interval,
            batch,
            setup: step_setup,
            sampling,
            rework: step_rework,
            dedicate_to,
            cqt: step_cqt,
        });
    }
    // CQT waits run from the end of the entrance step to the start of the exit step.
    for (index, step) in steps.iter().enumerate() {
        if let Some(cqt) = step.cqt
            && (step.sampling < 1.0 || steps[cqt.until].sampling < 1.0)
        {
            return Err(Error::new(format!(
                "{file}: CQT segment from step {} has a sampled entrance or exit",
                steps[index].name
            )));
        }
    }
    // Batches and setup runs wait for lots that can still come: every lot before such a step
    // must reach it, and no lot after it may return.
    for (index, step) in steps.iter().enumerate() {
        let run = matches!(tool_groups[step.tool_group].rule, Rule::SetupRun(_));
        let waits = step.batch.is_some() || (run && step.setup.is_some());
        let returns = steps[index..]
            .iter()
            .any(|later| later.rework.is_some_and(|rework| rework.to <= index));
        if waits && (step.sampling < 1.0 || returns) {
            return Err(Error::new(format!(
                "{file}: batch or setup-run step {} is sampled or inside a rework loop",
                step.name
            )));
        }
    }
    Ok(Route {
        name: route_name.to_owned(),
        steps,
    })
}

/// Step named in `col`, which must come after (`later`) or before step `index`.
fn step_ref(
    row: Row,
    col: usize,
    steps: &Names,
    index: StepIndex,
    later: bool,
) -> Result<StepIndex, Error> {
    let target = lookup(row, col, steps, "step")?;
    let ordered = if later {
        target > index
    } else {
        target < index
    };
    if ordered {
        Ok(target)
    } else {
        Err(row.error(format_args!(
            "step {} must come {} this step",
            row.text(col)?,
            if later { "after" } else { "before" }
        )))
    }
}

/// PM interval of a `pmcal` entry; the first-PM offset comes from `attach`.
#[derive(Clone, Copy)]
enum PmInterval {
    Calendar(Time),
    Wafers(u32),
}

fn read_calendars(
    downs: &Table,
    pms: &Table,
    attach: &Table,
    tool_groups: &mut [ToolGroup],
    tool_group_names: &Names,
    areas: &Names,
) -> Result<(), Error> {
    let [name, kind, ttf_kind, ttf, ttf_unit, ttr_kind, ttr, ttr_unit] = downs.cols([
        "DOWNCALNAME",
        "DOWNCALTYPE",
        "MTTFDIST",
        "MTTF",
        "MTTFUNITS",
        "MTTRDIST",
        "MTTR",
        "MTTRUNITS",
    ])?;
    let mut down_calendars = HashMap::new();
    for row in downs.rows() {
        if row.text(kind)? != "mttf_by_cal" {
            return Err(row.error("only DOWNCALTYPE mttf_by_cal is supported"));
        }
        let times = (
            row.dist(Some(ttf_kind), ttf, None, ttf_unit)?,
            row.dist(Some(ttr_kind), ttr, None, ttr_unit)?,
        );
        if down_calendars.insert(row.text(name)?, times).is_some() {
            return Err(row.error("duplicate DOWNCALNAME"));
        }
    }

    let [
        name,
        kind,
        interval,
        interval_unit,
        duration_kind,
        duration,
        duration2,
        duration_unit,
    ] = pms.cols([
        "PMCALNAME",
        "PMCALTYPE",
        "MTBPM",
        "MTBPMUNITS",
        "MTTRDIST",
        "MTTR",
        "MTTR2",
        "MTTRUNITS",
    ])?;
    let mut pm_calendars = HashMap::new();
    for row in pms.rows() {
        let every = match row.text(kind)? {
            "mtbpm_by_cal" => PmInterval::Calendar(row.duration(interval, interval_unit)?),
            "mtbpm_by_pieces" if row.text(interval_unit)? == "pieces" => {
                PmInterval::Wafers(row.count(interval)?)
            }
            _ => return Err(row.error("PMCALTYPE must be mtbpm_by_cal or mtbpm_by_pieces")),
        };
        let pm = (
            every,
            row.dist(
                Some(duration_kind),
                duration,
                Some(duration2),
                duration_unit,
            )?,
        );
        if pm_calendars.insert(row.text(name)?, pm).is_some() {
            return Err(row.error("duplicate PMCALNAME"));
        }
    }

    let [
        calendar,
        kind,
        resource_kind,
        resource,
        first_kind,
        first,
        first_unit,
    ] = attach.cols([
        "CALNAME", "CALTYPE", "RESTYPE", "RESNAME", "FOADIST", "FOA", "FOAUNITS",
    ])?;
    for row in attach.rows() {
        let targets: Vec<usize> = match row.text(resource_kind)? {
            "stngrp" => {
                let area = lookup(row, resource, areas, "area")?;
                (0..tool_groups.len())
                    .filter(|&group| tool_groups[group].area == area)
                    .collect()
            }
            "stnfam" => vec![lookup(row, resource, tool_group_names, "tool group")?],
            _ => return Err(row.error("RESTYPE must be stngrp or stnfam")),
        };
        let calendar_name = row.text(calendar)?;
        let unknown = || row.error(format_args!("unknown calendar {calendar_name}"));
        match row.text(kind)? {
            "down" => {
                let &(ttf, ttr) = down_calendars.get(calendar_name).ok_or_else(unknown)?;
                let breakdown = Breakdown {
                    first: row.dist(Some(first_kind), first, None, first_unit)?,
                    ttf,
                    ttr,
                };
                for group in targets {
                    tool_groups[group].breakdowns.push(breakdown);
                }
            }
            "pm" => {
                let &(every, duration) = pm_calendars.get(calendar_name).ok_or_else(unknown)?;
                if row.text(first_kind)? != "constant" {
                    return Err(row.error("PM FOADIST must be constant"));
                }
                let trigger = match every {
                    PmInterval::Calendar(interval) => PmTrigger::Calendar {
                        interval,
                        first: row.duration(first, first_unit)?,
                    },
                    PmInterval::Wafers(interval) if row.opt(first_unit).is_none() => {
                        PmTrigger::Wafers {
                            interval,
                            first: row.count(first)?,
                        }
                    }
                    PmInterval::Wafers(_) => {
                        return Err(row.error("wafer-count PMs take FOA without FOAUNITS"));
                    }
                };
                for group in targets {
                    tool_groups[group].pms.push(Pm { trigger, duration });
                }
            }
            _ => return Err(row.error("CALTYPE must be down or pm")),
        }
    }
    Ok(())
}

fn read_transports(table: &Table, locations: &Names) -> Result<Vec<Transport>, Error> {
    let [from, to, kind, time, time2, unit] =
        table.cols(["FROMLOC", "TOLOC", "DDIST", "DTIME", "DTIME2", "DUNITS"])?;
    let mut transports: Vec<Transport> = Vec::new();
    for row in table.rows() {
        let transport = Transport {
            from: lookup(row, from, locations, "location")?,
            to: lookup(row, to, locations, "location")?,
            time: row.dist(Some(kind), time, Some(time2), unit)?,
        };
        if transports
            .iter()
            .any(|other| (other.from, other.to) == (transport.from, transport.to))
        {
            return Err(row.error("duplicate FROMLOC/TOLOC pair"));
        }
        transports.push(transport);
    }
    Ok(transports)
}

fn read_orders(
    dir: &Path,
    files: &[&str],
    epoch: Time,
    parts: &[Part],
    part_names: &Names,
    routes: &[Route],
) -> Result<(Vec<ReleaseStream>, Vec<LotRelease>), Error> {
    let (mut streams, mut lots) = (Vec::new(), Vec::new());
    for &file in files {
        let table = Table::read(dir, file)?;
        let [part, priority, pieces, start, due] =
            table.cols(["PART", "PRIOR", "PIECES", "START", "DUE"])?;
        let [
            hot_lot,
            repeat_kind,
            repeat,
            repeat_unit,
            repeats,
            lots_per_repeat,
            current_step,
        ] = [
            "HOTLOT",
            "RDIST",
            "REPEAT",
            "RUNITS",
            "RPT#",
            "LOTSPERRPT",
            "CURSTEP",
        ]
        .map(|name| table.opt_col(name));
        for row in table.rows() {
            let filled = |col: Option<usize>| col.and_then(|col| row.opt(col));
            let part_id = lookup(row, part, part_names, "part")?;
            let (release, due_date) = (row.date(start, epoch)?, row.date(due, epoch)?);
            if release < 0 {
                return Err(row.error("release before SIM_START"));
            }
            let wafers = row.count(pieces)?;
            if wafers == 0 {
                return Err(row.error("PIECES must be positive"));
            }
            let lot_priority = row.count(priority)?;
            let reserve = match hot_lot {
                Some(col) => row.flag(col)?,
                None => false,
            };
            if filled(repeat).is_some() {
                let (Some(repeat), Some(repeat_unit), Some(repeats)) =
                    (repeat, repeat_unit, repeats)
                else {
                    return Err(row.error("REPEAT needs RUNITS and RPT# columns"));
                };
                if filled(repeat_kind).is_some_and(|kind| kind != "constant") {
                    return Err(row.error("only RDIST constant is supported"));
                }
                if filled(current_step).is_some() {
                    return Err(row.error("CURSTEP with REPEAT"));
                }
                // LOTSPERRPT defaults to one lot.
                let lots_per_release = match lots_per_repeat {
                    Some(col) if row.opt(col).is_some() => row.count(col)?,
                    _ => 1,
                };
                let (interval, count) = (row.duration(repeat, repeat_unit)?, row.count(repeats)?);
                if interval == 0 || count == 0 || lots_per_release == 0 {
                    return Err(row.error("REPEAT, RPT# and LOTSPERRPT must be positive"));
                }
                streams.push(ReleaseStream {
                    part: part_id,
                    priority: lot_priority,
                    wafers,
                    start: release,
                    interval,
                    count,
                    lots: lots_per_release,
                    due_offset: due_date - release,
                    reserve,
                });
            } else {
                if [repeat_kind, repeat_unit, repeats, lots_per_repeat]
                    .into_iter()
                    .any(|col| filled(col).is_some())
                {
                    return Err(row.error("RDIST/RUNITS/RPT#/LOTSPERRPT without REPEAT"));
                }
                let step = match filled(current_step) {
                    None => None,
                    Some(step_name) => Some(
                        routes[parts[part_id].route]
                            .steps
                            .iter()
                            .position(|step| step.name == step_name)
                            .ok_or_else(|| {
                                row.error(format_args!("unknown CURSTEP {step_name}"))
                            })?,
                    ),
                };
                lots.push(LotRelease {
                    part: part_id,
                    priority: lot_priority,
                    wafers,
                    start: release,
                    due: due_date,
                    reserve,
                    step,
                });
            }
        }
    }
    Ok((streams, lots))
}

fn read_periods(table: &Table, epoch: Time) -> Result<Vec<Period>, Error> {
    let [name, start, report, reset] = table.cols(["PERIOD", "PERIODSTART", "REPORT", "RESET"])?;
    let mut periods: Vec<Period> = Vec::new();
    for row in table.rows() {
        let period = Period {
            name: row.text(name)?.to_owned(),
            start: row.date(start, epoch)?,
            report: row.flag(report)?,
            reset: row.flag(reset)?,
        };
        if periods
            .last()
            .is_some_and(|last| last.start >= period.start)
        {
            return Err(row.error("periods must start in ascending order"));
        }
        periods.push(period);
    }
    Ok(periods)
}
