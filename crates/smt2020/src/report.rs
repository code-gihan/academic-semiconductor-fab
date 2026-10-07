//! Measures of run results in the papers' units, their statistics over replications and the
//! comparison of two configurations pair by pair: the tables of the CLI and the web page and
//! their CSV.

use std::collections::{HashMap, HashSet};
use std::fmt::Write;

use des_core::{DAY, HOUR, SECOND, Time};
use serde::{Deserialize, Serialize};

use crate::sim::{
    AmhsReport, CqtReport, CqtTimes, Error, LotKind, PeriodReport, Results, StateTimes,
};

named_enum! {
    /// What a measure describes.
    pub enum Scope {
        /// The whole fab; no item.
        Fab = "fab",
        /// Lots of one kind over all parts; no item.
        Kind = "kind",
        /// Lots of one part (item) and kind.
        Lot = "lot",
        /// A tool group (item).
        ToolGroup = "tool_group",
        /// An area (item): its tool groups' tool time together.
        Area = "area",
        /// CQT segments with stepper steps, the others, and all (item `litho`, `rest`, `total`).
        Cqt = "cqt",
        /// A CQT segment (item `route:entry-exit`, its route and step indices).
        CqtSegment = "cqt_segment",
        /// A step of a CQT segment (item `route:entry-exit:step`).
        CqtStep = "cqt_step",
        /// The AMHS of a dataset with a layout; no item.
        Amhs = "amhs",
        /// Loaded drives of a bay distance (\[SMAT2022\] Table 4; item `0`–`9`, `10+`, or
        /// `other` for drives with an end outside the intrabays).
        BayDistance = "bay_distance",
    }
}

named_enum! {
    /// Measures; the name's suffix is the unit: `_pct` percent, `_d` days, `_h` hours, `_s`
    /// seconds, otherwise counts, lots or ratios.
    pub enum Measure {
        /// Lots released in the window.
        Started = "started",
        /// Lots completed in the window (throughput); for CQT, segments completed.
        Completed = "completed",
        /// Time-averaged lots in the fab.
        Wip = "wip",
        /// Completed lots finished by their due date.
        OnTimePct = "on_time_pct",
        /// Cycle time mean of completed lots.
        CtMeanD = "ct_mean_d",
        /// Cycle time population standard deviation of completed lots.
        CtStdD = "ct_std_d",
        /// Mean flow factor (cycle time over raw processing time).
        FfMean = "ff_mean",
        /// Flow factor percentiles.
        FfP0 = "ff_p0",
        FfP5 = "ff_p5",
        FfP25 = "ff_p25",
        FfP50 = "ff_p50",
        FfP75 = "ff_p75",
        FfP95 = "ff_p95",
        FfP100 = "ff_p100",
        /// Shares of tool time per state.
        DownPct = "down_pct",
        PmPct = "pm_pct",
        SetupPct = "setup_pct",
        ProcessPct = "process_pct",
        LoadPct = "load_pct",
        UnloadPct = "unload_pct",
        IdlePct = "idle_pct",
        /// Setup, load, unload and processing.
        UtilPct = "util_pct",
        /// Neither down nor in PM.
        AvailabilityPct = "availability_pct",
        /// PM share of the downtime (scheduled down time).
        SdtSharePct = "sdt_share_pct",
        /// Highest tool group utilization of an area.
        UtilMaxPct = "util_max_pct",
        /// Segment completions with a violated limit.
        VlPct = "vl_pct",
        /// Violations longer than 1, 2 and 4 hours per segment completion.
        Vl1hPct = "vl1h_pct",
        Vl2hPct = "vl2h_pct",
        Vl4hPct = "vl4h_pct",
        /// Mean excess over the limits per completed segment.
        AvlH = "avl_h",
        /// Mean slack under the limits per completed segment.
        AontH = "aont_h",
        /// Mean transport, queue and processing time per visit of a segment step, of the
        /// completions within the limit (`ok`) and over it (`vl`).
        TransportOkH = "transport_ok_h",
        QueueOkH = "queue_ok_h",
        ProcessOkH = "process_ok_h",
        TransportVlH = "transport_vl_h",
        QueueVlH = "queue_vl_h",
        ProcessVlH = "process_vl_h",
        /// AMHS deliveries done in the window, and the tool-to-tool share of them.
        Deliveries = "deliveries",
        T2tPct = "t2t_pct",
        /// Mean times per delivery: from the request to the drop-off, waiting for a vehicle, its
        /// drive to the pickup, the loaded drive, and the loaded drive alone on the rails with
        /// the zones free (raw transport time).
        DeliveryS = "delivery_s",
        VehicleWaitS = "vehicle_wait_s",
        EmptyDriveS = "empty_drive_s",
        LoadedDriveS = "loaded_drive_s",
        RawDriveS = "raw_drive_s",
        /// Share of vehicle time on transports (driving to the pickup, hoisting, carrying).
        VehicleBusyPct = "vehicle_busy_pct",
        /// Mean wait of the zone requests that had to wait.
        ZoneWaitS = "zone_wait_s",
    }
}

const FLOW_FACTOR_MEASURES: [Measure; 7] = [
    Measure::FfP0,
    Measure::FfP5,
    Measure::FfP25,
    Measure::FfP50,
    Measure::FfP75,
    Measure::FfP95,
    Measure::FfP100,
];

/// One measure of a reporting period.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Metric {
    pub period: String,
    pub scope: Scope,
    /// Part, tool group, area or CQT segment set; empty for the fab and kinds.
    pub item: String,
    /// Lot kind of kind and lot measures.
    pub kind: Option<LotKind>,
    pub measure: Measure,
    pub value: f64,
}

/// A measure over replications.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Summary {
    pub period: String,
    pub scope: Scope,
    pub item: String,
    pub kind: Option<LotKind>,
    pub measure: Measure,
    /// Replications with the measure.
    pub n: usize,
    pub mean: f64,
    /// Sample standard deviation and half-width of the 95% confidence interval of the mean
    /// (Student t); `None` for a single replication.
    pub std: Option<f64>,
    pub ci95: Option<f64>,
}

/// Every measure of every reporting period of `results`. Measures without a basis (cycle times
/// without completions, shares of empty windows) are left out.
pub fn metrics(results: &Results) -> Vec<Metric> {
    let mut metrics = Vec::new();
    for period in &results.periods {
        let mut add = |scope, item: &str, kind, measure, value| {
            metrics.push(Metric {
                period: period.name.clone(),
                scope,
                item: item.to_owned(),
                kind,
                measure,
                value,
            });
        };
        period_metrics(period, &mut add);
    }
    metrics
}

fn period_metrics(
    period: &PeriodReport,
    add: &mut impl FnMut(Scope, &str, Option<LotKind>, Measure, f64),
) {
    let lots = &period.lots;
    add(
        Scope::Fab,
        "",
        None,
        Measure::Started,
        lots.iter().map(|lot| lot.started as f64).sum(),
    );
    add(
        Scope::Fab,
        "",
        None,
        Measure::Completed,
        lots.iter().map(|lot| lot.completed as f64).sum(),
    );
    add(Scope::Fab, "", None, Measure::Wip, period.wip);

    for &kind in LotKind::ALL {
        let of_kind = || lots.iter().filter(move |lot| lot.kind == kind);
        if of_kind().next().is_none() {
            continue;
        }
        // Moments pooled over parts: n, Σ ct, Σ (σ² + μ²)·n, Σ ff, on time.
        let (mut n, mut sum, mut squares, mut flow_factors, mut on_time) = (0.0, 0.0, 0.0, 0.0, 0);
        for lot in of_kind() {
            if let (Some(mean), Some(std), Some(ff)) = (
                lot.cycle_time_mean,
                lot.cycle_time_std,
                lot.flow_factor_mean,
            ) {
                let count = lot.completed as f64;
                n += count;
                sum += mean * count;
                squares += (std * std + mean * mean) * count;
                flow_factors += ff * count;
                on_time += lot.on_time;
            }
        }
        let started = of_kind().map(|lot| lot.started as f64).sum();
        add(Scope::Kind, "", Some(kind), Measure::Started, started);
        add(Scope::Kind, "", Some(kind), Measure::Completed, n);
        if n > 0.0 {
            let mean = sum / n;
            add(
                Scope::Kind,
                "",
                Some(kind),
                Measure::OnTimePct,
                100.0 * on_time as f64 / n,
            );
            add(
                Scope::Kind,
                "",
                Some(kind),
                Measure::CtMeanD,
                mean / DAY as f64,
            );
            let variance = (squares / n - mean * mean).max(0.0);
            add(
                Scope::Kind,
                "",
                Some(kind),
                Measure::CtStdD,
                variance.sqrt() / DAY as f64,
            );
            add(
                Scope::Kind,
                "",
                Some(kind),
                Measure::FfMean,
                flow_factors / n,
            );
        }
        if let Some(percentiles) = period.flow_factors.iter().find(|ff| ff.kind == kind) {
            for (measure, value) in FLOW_FACTOR_MEASURES
                .into_iter()
                .zip(percentiles.percentiles)
            {
                add(Scope::Kind, "", Some(kind), measure, value);
            }
        }
    }

    for lot in lots {
        let kind = Some(lot.kind);
        add(
            Scope::Lot,
            &lot.part,
            kind,
            Measure::Started,
            lot.started as f64,
        );
        add(
            Scope::Lot,
            &lot.part,
            kind,
            Measure::Completed,
            lot.completed as f64,
        );
        if let (Some(mean), Some(std), Some(ff)) = (
            lot.cycle_time_mean,
            lot.cycle_time_std,
            lot.flow_factor_mean,
        ) {
            let on_time = 100.0 * lot.on_time as f64 / lot.completed as f64;
            add(Scope::Lot, &lot.part, kind, Measure::OnTimePct, on_time);
            add(
                Scope::Lot,
                &lot.part,
                kind,
                Measure::CtMeanD,
                mean / DAY as f64,
            );
            add(
                Scope::Lot,
                &lot.part,
                kind,
                Measure::CtStdD,
                std / DAY as f64,
            );
            add(Scope::Lot, &lot.part, kind, Measure::FfMean, ff);
        }
    }

    let mut areas: Vec<(&str, StateTimes, f64)> = Vec::new();
    for group in &period.tool_groups {
        let time = group.time;
        let total = total(&time);
        if total <= 0.0 {
            continue;
        }
        let share = |part: i64| 100.0 * part as f64 / total;
        let util = share(busy(&time));
        let item = group.name.as_str();
        for (measure, part) in [
            (Measure::DownPct, time.down),
            (Measure::PmPct, time.pm),
            (Measure::SetupPct, time.setup),
            (Measure::ProcessPct, time.process),
            (Measure::LoadPct, time.load),
            (Measure::UnloadPct, time.unload),
            (Measure::IdlePct, time.idle),
        ] {
            add(Scope::ToolGroup, item, None, measure, share(part));
        }
        add(Scope::ToolGroup, item, None, Measure::UtilPct, util);
        add(
            Scope::ToolGroup,
            item,
            None,
            Measure::AvailabilityPct,
            100.0 - share(time.down + time.pm),
        );
        if let Some(sdt) = sdt_share(&time) {
            add(Scope::ToolGroup, item, None, Measure::SdtSharePct, sdt);
        }
        match areas.iter_mut().find(|(area, ..)| *area == group.area) {
            Some((_, sum, max)) => {
                *sum = sum_times(sum, &time);
                *max = max.max(util);
            }
            None => areas.push((&group.area, time, util)),
        }
    }
    for (area, time, max) in areas {
        let total = total(&time);
        let availability = 100.0 - 100.0 * (time.down + time.pm) as f64 / total;
        add(
            Scope::Area,
            area,
            None,
            Measure::AvailabilityPct,
            availability,
        );
        if let Some(sdt) = sdt_share(&time) {
            add(Scope::Area, area, None, Measure::SdtSharePct, sdt);
        }
        let util = 100.0 * busy(&time) as f64 / total;
        add(Scope::Area, area, None, Measure::UtilPct, util);
        add(Scope::Area, area, None, Measure::UtilMaxPct, max);
    }

    let total_cqt = period.cqt_litho.merge(&period.cqt_rest);
    for (item, cqt) in [
        ("litho", &period.cqt_litho),
        ("rest", &period.cqt_rest),
        ("total", &total_cqt),
    ] {
        cqt_metrics(add, Scope::Cqt, item, cqt);
    }
    for segment in &period.cqt_segments {
        let item = format!("{}:{}-{}", segment.route, segment.entry, segment.exit);
        cqt_metrics(add, Scope::CqtSegment, &item, &segment.cqt);
        for step in &segment.steps {
            let item = format!("{item}:{}", step.step);
            for (times, measures) in [
                (
                    &step.met,
                    [
                        Measure::TransportOkH,
                        Measure::QueueOkH,
                        Measure::ProcessOkH,
                    ],
                ),
                (
                    &step.violated,
                    [
                        Measure::TransportVlH,
                        Measure::QueueVlH,
                        Measure::ProcessVlH,
                    ],
                ),
            ] {
                if times.visits > 0 {
                    for (measure, time) in measures.into_iter().zip(times_of(times)) {
                        let hours = time as f64 / times.visits as f64 / HOUR as f64;
                        add(Scope::CqtStep, &item, None, measure, hours);
                    }
                }
            }
        }
    }
    if let Some(amhs) = &period.amhs {
        amhs_metrics(add, amhs);
    }
}

/// The AMHS measures (\[SMAT2022\] §4): deliveries with their tool-to-tool share and mean
/// times, the vehicles' busy share and the zone waits, then the loaded drives by bay distance.
fn amhs_metrics(
    add: &mut impl FnMut(Scope, &str, Option<LotKind>, Measure, f64),
    amhs: &AmhsReport,
) {
    let seconds = |time: Time, count: u64| time as f64 / count as f64 / SECOND as f64;
    let deliveries = amhs.moves.total();
    add(
        Scope::Amhs,
        "",
        None,
        Measure::Deliveries,
        deliveries as f64,
    );
    if deliveries > 0 {
        let t2t = 100.0 * amhs.moves.tool_to_tool as f64 / deliveries as f64;
        add(Scope::Amhs, "", None, Measure::T2tPct, t2t);
        let raw = amhs
            .bay_distances
            .iter()
            .map(|class| class.unobstructed)
            .sum();
        for (measure, time) in [
            (Measure::DeliveryS, amhs.delivery),
            (Measure::VehicleWaitS, amhs.vehicle_wait),
            (Measure::EmptyDriveS, amhs.empty_drive),
            (Measure::LoadedDriveS, amhs.loaded_drive),
            (Measure::RawDriveS, raw),
        ] {
            add(Scope::Amhs, "", None, measure, seconds(time, deliveries));
        }
    }
    let times = amhs.vehicle_time;
    let total = times.idle + times.to_pickup + times.loading + times.to_dropoff + times.unloading;
    if total > 0 {
        let busy = 100.0 * (total - times.idle) as f64 / total as f64;
        add(Scope::Amhs, "", None, Measure::VehicleBusyPct, busy);
    }
    if amhs.zone_waits > 0 {
        let wait = seconds(amhs.zone_wait, amhs.zone_waits);
        add(Scope::Amhs, "", None, Measure::ZoneWaitS, wait);
    }
    for (class, drives) in amhs.bay_distances.iter().enumerate() {
        let item = match class {
            0..=9 => class.to_string(),
            10 => "10+".into(),
            _ => "other".into(),
        };
        add(
            Scope::BayDistance,
            &item,
            None,
            Measure::Deliveries,
            drives.drives as f64,
        );
        if drives.drives > 0 {
            let (time, raw) = (drives.time, drives.unobstructed);
            add(
                Scope::BayDistance,
                &item,
                None,
                Measure::LoadedDriveS,
                seconds(time, drives.drives),
            );
            add(
                Scope::BayDistance,
                &item,
                None,
                Measure::RawDriveS,
                seconds(raw, drives.drives),
            );
        }
    }
}

fn times_of(times: &CqtTimes) -> [i64; 3] {
    [times.transport, times.queue, times.process]
}

/// The measures of CQT completions: their count, then (with completions) the shares over the
/// limit and the mean excess and slack.
fn cqt_metrics(
    add: &mut impl FnMut(Scope, &str, Option<LotKind>, Measure, f64),
    scope: Scope,
    item: &str,
    cqt: &CqtReport,
) {
    add(scope, item, None, Measure::Completed, cqt.completed as f64);
    if cqt.completed == 0 {
        return;
    }
    let completed = cqt.completed as f64;
    let share = |count: u64| 100.0 * count as f64 / completed;
    for (measure, count) in [
        (Measure::VlPct, cqt.violated),
        (Measure::Vl1hPct, cqt.violated_1h),
        (Measure::Vl2hPct, cqt.violated_2h),
        (Measure::Vl4hPct, cqt.violated_4h),
    ] {
        add(scope, item, None, measure, share(count));
    }
    let hours = |time: i64| time as f64 / completed / HOUR as f64;
    add(scope, item, None, Measure::AvlH, hours(cqt.violation));
    add(scope, item, None, Measure::AontH, hours(cqt.slack));
}

fn total(time: &StateTimes) -> f64 {
    (time.down + time.pm + time.setup + time.process + time.load + time.unload + time.idle) as f64
}

fn busy(time: &StateTimes) -> i64 {
    time.setup + time.process + time.load + time.unload
}

fn sdt_share(time: &StateTimes) -> Option<f64> {
    let down = time.down + time.pm;
    (down > 0).then(|| 100.0 * time.pm as f64 / down as f64)
}

fn sum_times(a: &StateTimes, b: &StateTimes) -> StateTimes {
    StateTimes {
        down: a.down + b.down,
        pm: a.pm + b.pm,
        setup: a.setup + b.setup,
        process: a.process + b.process,
        load: a.load + b.load,
        unload: a.unload + b.unload,
        idle: a.idle + b.idle,
    }
}

/// Every measure of the replications' results (runs of one configuration, replication numbers
/// differing), in the order of first appearance.
pub fn summarize(replications: &[Results]) -> Vec<Summary> {
    type Key = (String, Scope, String, Option<LotKind>, Measure);
    let mut keys: Vec<Key> = Vec::new();
    let mut values: Vec<Vec<f64>> = Vec::new();
    let mut index: HashMap<Key, usize> = HashMap::new();
    for results in replications {
        for metric in metrics(results) {
            let key = (
                metric.period,
                metric.scope,
                metric.item,
                metric.kind,
                metric.measure,
            );
            match index.get(&key) {
                Some(&at) => values[at].push(metric.value),
                None => {
                    index.insert(key.clone(), keys.len());
                    keys.push(key);
                    values.push(vec![metric.value]);
                }
            }
        }
    }
    keys.into_iter()
        .zip(values)
        .map(|((period, scope, item, kind, measure), values)| {
            let (mean, std, ci95) = statistics(&values);
            Summary {
                period,
                scope,
                item,
                kind,
                measure,
                n: values.len(),
                mean,
                std,
                ci95,
            }
        })
        .collect()
}

/// A measure of two configurations whose replications share their random numbers pair by pair.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Comparison {
    pub period: String,
    pub scope: Scope,
    pub item: String,
    pub kind: Option<LotKind>,
    pub measure: Measure,
    /// Pairs with the measure in both runs.
    pub n: usize,
    /// Means over those pairs of the baseline and of the other configuration.
    pub baseline: f64,
    pub other: f64,
    /// Mean difference (other − baseline), sample standard deviation of the differences and
    /// half-width of the 95% confidence interval of the mean difference (Student t); `None` for
    /// one pair.
    pub difference: f64,
    pub std: Option<f64>,
    pub ci95: Option<f64>,
}

/// Every measure of `other` against `baseline`, the results of two configurations' replications
/// paired by seed and replication (common random numbers): each run needs exactly one partner.
/// Pairing removes the variation the two runs share, so the interval of the difference is
/// narrower than the two intervals suggest.
pub fn compare(baseline: &[Results], other: &[Results]) -> Result<Vec<Comparison>, Error> {
    let pair = |results: &Results| (results.seed, results.replication);
    let unpaired = |results: &Results, side: &str| {
        Error(format!(
            "the runs do not pair: {side} seed {} replication {}",
            results.seed, results.replication
        ))
    };
    let mut partners = HashMap::new();
    for (index, results) in baseline.iter().enumerate() {
        if partners.insert(pair(results), index).is_some() {
            return Err(unpaired(results, "a second baseline run of"));
        }
    }
    type Key = (String, Scope, String, Option<LotKind>, Measure);
    let key = |metric: Metric| -> (Key, f64) {
        (
            (
                metric.period,
                metric.scope,
                metric.item,
                metric.kind,
                metric.measure,
            ),
            metric.value,
        )
    };
    let mut keys: Vec<Key> = Vec::new();
    let mut values: Vec<Vec<(f64, f64)>> = Vec::new();
    let mut index: HashMap<Key, usize> = HashMap::new();
    let mut paired = HashSet::new();
    for results in other {
        let Some(&partner) = partners.get(&pair(results)) else {
            return Err(unpaired(results, "no baseline run of"));
        };
        if !paired.insert(pair(results)) {
            return Err(unpaired(results, "a second run of"));
        }
        let theirs: HashMap<Key, f64> = metrics(results).into_iter().map(key).collect();
        for (key, value) in metrics(&baseline[partner]).into_iter().map(key) {
            let Some(&their) = theirs.get(&key) else {
                continue;
            };
            match index.get(&key) {
                Some(&at) => values[at].push((value, their)),
                None => {
                    index.insert(key.clone(), keys.len());
                    keys.push(key);
                    values.push(vec![(value, their)]);
                }
            }
        }
    }
    if paired.len() < baseline.len() {
        let results = baseline
            .iter()
            .find(|results| !paired.contains(&pair(results)))
            .expect("an unpaired baseline run");
        return Err(unpaired(results, "no run to compare with baseline"));
    }
    Ok(keys
        .into_iter()
        .zip(values)
        .map(|((period, scope, item, kind, measure), pairs)| {
            let n = pairs.len();
            let mean =
                |side: fn(&(f64, f64)) -> f64| pairs.iter().map(side).sum::<f64>() / n as f64;
            let differences: Vec<f64> = pairs
                .iter()
                .map(|(baseline, other)| other - baseline)
                .collect();
            let (difference, std, ci95) = statistics(&differences);
            Comparison {
                period,
                scope,
                item,
                kind,
                measure,
                n,
                baseline: mean(|pair| pair.0),
                other: mean(|pair| pair.1),
                difference,
                std,
                ci95,
            }
        })
        .collect())
}

/// Mean, sample standard deviation and half-width of the 95% confidence interval of the mean
/// (Student t) of at least one value; no deviation or interval for one.
fn statistics(values: &[f64]) -> (f64, Option<f64>, Option<f64>) {
    let n = values.len();
    let mean = values.iter().sum::<f64>() / n as f64;
    if n < 2 {
        return (mean, None, None);
    }
    let variance = values
        .iter()
        .map(|value| (value - mean).powi(2))
        .sum::<f64>()
        / (n - 1) as f64;
    let std = variance.sqrt();
    (
        mean,
        Some(std),
        Some(t_975(n - 1) * std / (n as f64).sqrt()),
    )
}

/// A measure of one day over replications.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DaySummary {
    pub day: usize,
    /// `fab` (started, completed, wip) or `cqt` (completed, vl_pct, avl_h).
    pub scope: Scope,
    pub measure: Measure,
    /// Replications that ran the day (with CQT completions for the shares).
    pub n: usize,
    pub mean: f64,
    pub std: Option<f64>,
    pub ci95: Option<f64>,
}

/// The days of the replications' results, day by day: lots released, completed and in the fab,
/// CQT completions, their share over the limit and mean excess.
pub fn daily(replications: &[Results]) -> Vec<DaySummary> {
    let days = replications
        .iter()
        .map(|results| results.days.len())
        .max()
        .unwrap_or(0);
    let mut summaries = Vec::new();
    for day in 0..days {
        let reports: Vec<_> = replications
            .iter()
            .filter_map(|results| results.days.get(day))
            .collect();
        let with_cqt: Vec<_> = reports
            .iter()
            .filter(|report| report.cqt.completed > 0)
            .collect();
        let measures: [(Scope, Measure, Vec<f64>); 6] = [
            (
                Scope::Fab,
                Measure::Started,
                reports.iter().map(|report| report.started as f64).collect(),
            ),
            (
                Scope::Fab,
                Measure::Completed,
                reports
                    .iter()
                    .map(|report| report.completed as f64)
                    .collect(),
            ),
            (
                Scope::Fab,
                Measure::Wip,
                reports.iter().map(|report| report.wip).collect(),
            ),
            (
                Scope::Cqt,
                Measure::Completed,
                reports
                    .iter()
                    .map(|report| report.cqt.completed as f64)
                    .collect(),
            ),
            (
                Scope::Cqt,
                Measure::VlPct,
                with_cqt
                    .iter()
                    .map(|report| 100.0 * report.cqt.violated as f64 / report.cqt.completed as f64)
                    .collect(),
            ),
            (
                Scope::Cqt,
                Measure::AvlH,
                with_cqt
                    .iter()
                    .map(|report| {
                        report.cqt.violation as f64 / report.cqt.completed as f64 / HOUR as f64
                    })
                    .collect(),
            ),
        ];
        for (scope, measure, values) in measures {
            if values.is_empty() {
                continue;
            }
            let (mean, std, ci95) = statistics(&values);
            summaries.push(DaySummary {
                day,
                scope,
                measure,
                n: values.len(),
                mean,
                std,
                ci95,
            });
        }
    }
    summaries
}

/// 0.975 quantile of Student's t distribution with `df` > 0 degrees of freedom: exact values to
/// 9 df, then the Cornish–Fisher expansion (Abramowitz & Stegun 26.7.5; error below 3e-5).
fn t_975(df: usize) -> f64 {
    const EXACT: [f64; 9] = [
        12.706_204_736,
        4.302_652_730,
        3.182_446_305,
        2.776_445_105,
        2.570_581_836,
        2.446_911_851,
        2.364_624_252,
        2.306_004_135,
        2.262_157_163,
    ];
    if let Some(&t) = EXACT.get(df - 1) {
        return t;
    }
    let z: f64 = 1.959_963_985;
    let v = df as f64;
    let g1 = (z.powi(3) + z) / 4.0;
    let g2 = (5.0 * z.powi(5) + 16.0 * z.powi(3) + 3.0 * z) / 96.0;
    let g3 = (3.0 * z.powi(7) + 19.0 * z.powi(5) + 17.0 * z.powi(3) - 15.0 * z) / 384.0;
    let g4 = (79.0 * z.powi(9) + 776.0 * z.powi(7) + 1482.0 * z.powi(5)
        - 1920.0 * z.powi(3)
        - 945.0 * z)
        / 92_160.0;
    z + g1 / v + g2 / v.powi(2) + g3 / v.powi(3) + g4 / v.powi(4)
}

/// Summaries as CSV: `period,scope,item,kind,measure,n,mean,std,ci95`; empty cells for none.
pub fn csv(summaries: &[Summary]) -> String {
    let mut csv = String::from("period,scope,item,kind,measure,n,mean,std,ci95\n");
    for summary in summaries {
        let optional =
            |value: Option<f64>| value.map(|value| value.to_string()).unwrap_or_default();
        let _ = writeln!(
            csv,
            "{},{},{},{},{},{},{},{},{}",
            field(&summary.period),
            summary.scope.name(),
            field(&summary.item),
            summary.kind.map(LotKind::name).unwrap_or_default(),
            summary.measure.name(),
            summary.n,
            summary.mean,
            optional(summary.std),
            optional(summary.ci95),
        );
    }
    csv
}

/// Comparisons as CSV: `period,scope,item,kind,measure,n,baseline,other,difference,std,ci95`;
/// empty cells for none.
pub fn comparison_csv(comparisons: &[Comparison]) -> String {
    let mut csv =
        String::from("period,scope,item,kind,measure,n,baseline,other,difference,std,ci95\n");
    for comparison in comparisons {
        let optional =
            |value: Option<f64>| value.map(|value| value.to_string()).unwrap_or_default();
        let _ = writeln!(
            csv,
            "{},{},{},{},{},{},{},{},{},{},{}",
            field(&comparison.period),
            comparison.scope.name(),
            field(&comparison.item),
            comparison.kind.map(LotKind::name).unwrap_or_default(),
            comparison.measure.name(),
            comparison.n,
            comparison.baseline,
            comparison.other,
            comparison.difference,
            optional(comparison.std),
            optional(comparison.ci95),
        );
    }
    csv
}

/// A CSV field, quoted if it holds a comma, quote or line break.
fn field(text: &str) -> String {
    if text.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", text.replace('"', "\"\""))
    } else {
        text.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::{
        CqtSegmentReport, CqtStepReport, DayReport, FlowFactors, LotReport, ToolGroupReport,
    };

    /// One period: two parts' regular lots, a tool group, CQT segments (one segment of two
    /// steps) and two days.
    fn results(ct: [f64; 2], down: i64) -> Results {
        let lot = |part: &str, completed: u64, mean: f64, std: f64| LotReport {
            part: part.into(),
            kind: LotKind::Prl,
            started: 10,
            completed,
            on_time: completed / 2,
            cycle_time_mean: Some(mean * DAY as f64),
            cycle_time_std: Some(std * DAY as f64),
            flow_factor_mean: Some(mean / 10.0),
        };
        let cqt = CqtReport {
            completed: 4,
            violated: 1,
            violated_1h: 1,
            violated_2h: 0,
            violated_4h: 0,
            violation: 2 * HOUR,
            slack: 6 * HOUR,
        };
        // The violation waited 2 h at the exit step; the others 1 h for transport in total.
        let times = |visits, transport, queue, process| CqtTimes {
            visits,
            transport,
            queue,
            process,
        };
        let steps = vec![
            CqtStepReport {
                step: 1,
                met: times(3, HOUR, 0, 3 * HOUR),
                violated: times(1, 0, 0, HOUR),
            },
            CqtStepReport {
                step: 2,
                met: times(3, 0, 6 * HOUR, 0),
                violated: times(1, 0, 4 * HOUR, 0),
            },
        ];
        let day = |started, wip, violated| DayReport {
            started,
            completed: 10,
            wip,
            cqt: CqtReport {
                completed: 2,
                violated,
                ..CqtReport::default()
            },
        };
        Results {
            seed: 1,
            replication: 0,
            periods: vec![PeriodReport {
                name: "Period_1".into(),
                start: 0,
                end: DAY,
                lots: vec![lot("part_1", 2, ct[0], 1.0), lot("part,2", 6, ct[1], 2.0)],
                flow_factors: vec![FlowFactors {
                    kind: LotKind::Prl,
                    percentiles: [1.0, 1.1, 1.2, 1.3, 1.4, 1.5, 1.6],
                }],
                wip: 12.5,
                tool_groups: vec![ToolGroupReport {
                    name: "Etch_1".into(),
                    area: "Etch".into(),
                    tools: 1,
                    time: StateTimes {
                        down,
                        pm: 10,
                        setup: 20,
                        process: 30,
                        load: 5,
                        unload: 5,
                        idle: 30 - down,
                    },
                }],
                cqt_litho: CqtReport::default(),
                cqt_rest: cqt,
                cqt_segments: vec![CqtSegmentReport {
                    route: "R1".into(),
                    entry: 0,
                    exit: 2,
                    litho: false,
                    cqt,
                    steps,
                }],
                amhs: None,
            }],
            days: vec![day(12, 10.0, 0), day(8, 12.0 + down as f64, 1)],
            released: 20,
            completed: 20,
            end: DAY,
            events: 1,
            step_flow_factors: Vec::new(),
        }
    }

    fn value(metrics: &[Metric], scope: Scope, item: &str, measure: Measure) -> Option<f64> {
        metrics
            .iter()
            .find(|metric| {
                metric.scope == scope && metric.item == item && metric.measure == measure
            })
            .map(|metric| metric.value)
    }

    #[test]
    fn measures_pool_parts_and_share_tool_time() {
        let metrics = metrics(&results([10.0, 20.0], 0));
        let kind = |measure| value(&metrics, Scope::Kind, "", measure).unwrap();
        // 2 lots at 10 ± 1 d and 6 at 20 ± 2 d.
        assert_eq!(kind(Measure::Completed), 8.0);
        assert!((kind(Measure::CtMeanD) - 17.5).abs() < 1e-12);
        let pooled = ((2.0 * 101.0 + 6.0 * 404.0) / 8.0 - 17.5f64.powi(2)).sqrt();
        assert!((kind(Measure::CtStdD) - pooled).abs() < 1e-12);
        assert_eq!(kind(Measure::OnTimePct), 50.0);
        assert_eq!(kind(Measure::FfP50), 1.3);
        assert_eq!(
            value(&metrics, Scope::Lot, "part,2", Measure::CtMeanD),
            Some(20.0)
        );
        let group = |measure| value(&metrics, Scope::ToolGroup, "Etch_1", measure).unwrap();
        assert_eq!(
            (group(Measure::UtilPct), group(Measure::AvailabilityPct)),
            (60.0, 90.0)
        );
        assert_eq!(group(Measure::SdtSharePct), 100.0);
        assert_eq!(
            value(&metrics, Scope::Area, "Etch", Measure::UtilMaxPct),
            Some(60.0)
        );
        let cqt = |item, measure| value(&metrics, Scope::Cqt, item, measure);
        assert_eq!(cqt("total", Measure::VlPct), Some(25.0));
        assert_eq!(
            (cqt("rest", Measure::AvlH), cqt("rest", Measure::AontH)),
            (Some(0.5), Some(1.5))
        );
        // No CQT segment of the stepper set completed: only the count.
        assert_eq!(cqt("litho", Measure::Completed), Some(0.0));
        assert_eq!(cqt("litho", Measure::VlPct), None);
        // Per segment as for the sets; per step the mean hours per visit.
        assert_eq!(
            value(&metrics, Scope::CqtSegment, "R1:0-2", Measure::VlPct),
            Some(25.0)
        );
        let step = |item, measure| value(&metrics, Scope::CqtStep, item, measure).unwrap();
        assert_eq!(
            (
                step("R1:0-2:2", Measure::QueueOkH),
                step("R1:0-2:2", Measure::QueueVlH)
            ),
            (2.0, 4.0)
        );
        assert_eq!(step("R1:0-2:1", Measure::ProcessVlH), 1.0);
        assert!((step("R1:0-2:1", Measure::TransportOkH) - 1.0 / 3.0).abs() < 1e-12);
    }

    #[test]
    fn days_carry_intervals_day_by_day() {
        let days = daily(&[results([10.0, 20.0], 0), results([12.0, 20.0], 10)]);
        let find = |day, scope, measure| {
            days.iter()
                .find(|row| row.day == day && row.scope == scope && row.measure == measure)
                .unwrap()
        };
        let wip = find(1, Scope::Fab, Measure::Wip);
        assert_eq!((wip.n, wip.mean), (2, 17.0));
        assert!((wip.std.unwrap() - 50f64.sqrt()).abs() < 1e-12);
        assert_eq!(find(0, Scope::Fab, Measure::Started).mean, 12.0);
        assert_eq!(find(1, Scope::Cqt, Measure::VlPct).mean, 50.0);
        assert_eq!(find(0, Scope::Cqt, Measure::VlPct).std, Some(0.0));
    }

    #[test]
    fn summaries_carry_student_t_intervals() {
        let summaries = summarize(&[results([10.0, 20.0], 0), results([12.0, 20.0], 10)]);
        let find = |scope, item: &str, measure| {
            summaries
                .iter()
                .find(|summary| {
                    summary.scope == scope && summary.item == item && summary.measure == measure
                })
                .unwrap()
        };
        let ct = find(Scope::Lot, "part_1", Measure::CtMeanD);
        assert_eq!((ct.n, ct.mean), (2, 11.0));
        let std = 2f64.sqrt();
        assert!((ct.std.unwrap() - std).abs() < 1e-12);
        assert!((ct.ci95.unwrap() - 12.706_204_736 * std / 2f64.sqrt()).abs() < 1e-9);
        let sdt = find(Scope::ToolGroup, "Etch_1", Measure::SdtSharePct);
        assert_eq!((sdt.n, sdt.mean), (2, 75.0));
        let single = summarize(&[results([10.0, 20.0], 0)]);
        assert!(
            single
                .iter()
                .all(|summary| summary.std.is_none() && summary.ci95.is_none())
        );
    }

    #[test]
    fn comparisons_pair_replications() {
        let second = |ct| Results {
            replication: 1,
            ..results(ct, 0)
        };
        let baseline = [results([10.0, 20.0], 0), second([12.0, 20.0])];
        // In the other order: pairs go by seed and replication.
        let other = [second([13.0, 20.0]), results([11.0, 20.0], 0)];
        let comparisons = compare(&baseline, &other).unwrap();
        let ct = comparisons
            .iter()
            .find(|comparison| {
                comparison.scope == Scope::Lot
                    && comparison.item == "part_1"
                    && comparison.measure == Measure::CtMeanD
            })
            .unwrap();
        // Pairs (10, 11) and (12, 13): the difference is 1 in both, without spread.
        assert_eq!(
            (ct.n, ct.baseline, ct.other, ct.difference, ct.std, ct.ci95),
            (2, 11.0, 12.0, 1.0, Some(0.0), Some(0.0))
        );
        let csv = comparison_csv(&comparisons);
        assert!(
            csv.starts_with(
                "period,scope,item,kind,measure,n,baseline,other,difference,std,ci95\n"
            )
        );
        assert!(csv.contains("Period_1,lot,part_1,PRL,ct_mean_d,2,11,12,1,0,0\n"));
        // Every run needs exactly one partner.
        assert!(compare(&baseline, &other[..1]).is_err());
        assert!(compare(&baseline[..1], &other).is_err());
        let twice = [results([10.0, 20.0], 0), results([11.0, 20.0], 0)];
        assert!(compare(&twice, &other).is_err());
        assert!(compare(&baseline, &twice).is_err());
    }

    #[test]
    fn t_quantiles() {
        assert_eq!(t_975(1), 12.706_204_736);
        for (df, exact) in [
            (10, 2.228_138_852),
            (30, 2.042_272_456),
            (120, 1.979_930_405),
        ] {
            assert!((t_975(df) - exact).abs() < 3e-5, "df {df}");
        }
    }

    #[test]
    fn csv_names_and_quotes() {
        let csv = csv(&summarize(&[results([10.0, 20.0], 0)]));
        let mut lines = csv.lines();
        assert_eq!(
            lines.next(),
            Some("period,scope,item,kind,measure,n,mean,std,ci95")
        );
        assert_eq!(lines.next(), Some("Period_1,fab,,,started,1,20,,"));
        assert!(csv.contains("Period_1,lot,\"part,2\",PRL,ct_mean_d,1,20,,\n"));
        assert!(csv.contains("Period_1,tool_group,Etch_1,,util_pct,1,60,,\n"));
    }

    #[test]
    fn names_are_the_serialized_form() {
        for &measure in Measure::ALL {
            let json = serde_json::to_string(&measure).unwrap();
            assert_eq!(json, format!("\"{}\"", measure.name()));
            assert_eq!(serde_json::from_str::<Measure>(&json).unwrap(), measure);
        }
        assert!(serde_json::from_str::<Scope>("\"fabs\"").is_err());
    }
}
