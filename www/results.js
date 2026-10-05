// The overview of a finished run: its setup, the headline measures, charts and tables of a report
// period (CQT violations per segment, where the waits of one segment go, the days), and the
// replications' digests and run times. Values are replication means ± the half-width of their
// 95% confidence interval.
import { barChart, lineChart } from "./charts.js";
import { formatNumber, t } from "./i18n.js";
import { DAY, segmentKey, segmentLabel, segmentTitle, stepLabel } from "./labels.js";
import { countUp, reveal } from "./motion.js";

const KINDS = ["PRL", "PHL", "SHL", "ERL", "EHL"];
/** Tool states as stacked in the tool group chart: busy, then outages, then idle. */
const STATES = ["process", "setup", "load", "unload", "down", "pm", "idle"];
/** Tool groups and segments the charts show until all are asked for. */
const TOP = 15;
/** Parts of a CQT wait and their colours. */
const PARTS = [
  ["queue", "warning"],
  ["transport", "load"],
  ["process", "process"],
];
/** Day-by-day measures offered: [scope, measure, decimals]. */
const DAILY = [
  ["cqt", "vl_pct", 1],
  ["fab", "wip", 0],
  ["fab", "completed", 0],
  ["cqt", "completed", 0],
];

/** Headline measures, each shown when the period has it. */
const KPIS = [
  { label: "kpi.started", scope: "fab", measure: "started", decimals: 0 },
  { label: "kpi.completed", scope: "fab", measure: "completed", decimals: 0 },
  { label: "kpi.wip", scope: "fab", measure: "wip", decimals: 0 },
  { label: "kpi.prlCt", scope: "kind", kind: "PRL", measure: "ct_mean_d", decimals: 2 },
  { label: "kpi.prlOnTime", scope: "kind", kind: "PRL", measure: "on_time_pct", decimals: 1 },
  { label: "kpi.erlCt", scope: "kind", kind: "ERL", measure: "ct_mean_d", decimals: 2 },
  { label: "kpi.cqt", scope: "cqt", item: "total", measure: "vl_pct", decimals: 2 },
];

/** Table columns: measure, label key, decimals, label values. */
const LOT_COLUMNS = [
  ["completed", "col.completed", 0],
  ["ct_mean_d", "col.ctMean", 2],
  ["ct_std_d", "col.ctStd", 2],
  ["on_time_pct", "col.onTime", 1],
  ["ff_mean", "col.ffMean", 3],
];

/** Tables of one scope each; `name` selects their texts (table.<name>.*). */
const TABLES = [
  { name: "kind", scope: "kind", columns: LOT_COLUMNS },
  {
    name: "ff",
    scope: "kind",
    columns: [0, 5, 25, 50, 75, 95, 100].map((p) => [`ff_p${p}`, "col.percentile", 3, { p }]),
  },
  { name: "lot", scope: "lot", columns: LOT_COLUMNS },
  {
    name: "cqt",
    scope: "cqt",
    columns: [
      ["completed", "col.completed", 0],
      ["vl_pct", "col.vl", 2],
      ["vl1h_pct", "col.vl1h", 2],
      ["vl2h_pct", "col.vl2h", 2],
      ["vl4h_pct", "col.vl4h", 2],
      ["avl_h", "col.avl", 3],
      ["aont_h", "col.aont", 3],
    ],
  },
  {
    name: "area",
    scope: "area",
    columns: [
      ["availability_pct", "col.availability", 2],
      ["sdt_share_pct", "col.sdtShare", 1],
      ["util_pct", "col.util", 2],
      ["util_max_pct", "col.utilMax", 2],
    ],
  },
  {
    name: "toolGroup",
    scope: "tool_group",
    sortBy: "util_pct",
    columns: [
      ["util_pct", "col.util", 2],
      ["availability_pct", "col.availability", 2],
      ["down_pct", "col.down", 2],
      ["pm_pct", "col.pm", 2],
      ["setup_pct", "col.setup", 2],
      ["process_pct", "col.process", 2],
      ["load_pct", "col.load", 2],
      ["unload_pct", "col.unload", 2],
      ["idle_pct", "col.idle", 2],
    ],
  },
];

const $ = (id) => document.getElementById(id);
/** The charts show every tool group, every segment. */
let allToolGroups = false;
let allSegments = false;
/** The run and period shown, the segment whose waits are broken down, the daily measure. */
let shown = { run: null, period: null };
let selected = null;
let daily = 0;
/** The charts drawn now play their entrance (new values, not a choice within them). */
let animate = false;
/** Called with the segment the user chooses. */
let chosen = () => {};

/** Name of the last report period before Drain: the longest window inside the horizon. */
export function defaultPeriod(run) {
  const periods = run.done[0].results.periods;
  return (periods.at(-2) ?? periods[0]).name;
}

/** The CQT segment chosen in the overview (index), for the details. */
export function selectedSegment() {
  return selected;
}

/** Calls `callback(segment)` whenever the user chooses a CQT segment in the overview. */
export function onSegmentChosen(callback) {
  chosen = callback;
}

/** Shows `run` (a finished run of the page) for the report period named `period`; `animated`
 * plays the entrance, as for new values (not for a new language). */
export function showResults(run, period, animated) {
  if (run !== shown.run) selected = null;
  shown = { run, period };
  animate = animated;
  const rows = run.summary.filter((row) => row.period === period);
  const at = new Map(rows.map((row) => [key(row.scope, row.item, row.kind, row.measure), row]));
  const value = (scope, item, kind, measure) => at.get(key(scope, item, kind, measure));

  $("run-setup").replaceChildren(
    ...run.setup.map(([label, text]) => {
      const pair = document.createElement("div");
      pair.append(element("dt", t(label)), element("dd", text()));
      return pair;
    }),
  );
  $("period").replaceChildren(
    ...run.done[0].results.periods.map(({ name, start, end }) => {
      const label = t("results.periodOption", {
        name,
        start: formatNumber(start / DAY, 0),
        end: formatNumber(end / DAY, 0),
      });
      return new Option(label, name, false, name === period);
    }),
  );
  $("kpis").replaceChildren(
    ...KPIS.flatMap((spec) => {
      const summary = value(spec.scope, spec.item ?? "", spec.kind ?? null, spec.measure);
      return summary ? [kpi(t(spec.label), summary, spec.decimals, animated)] : [];
    }),
  );
  // Lot outcomes side by side, the CQT segments and where their waits go, the days, then the
  // capacity by area and by tool group, a row each.
  const segments = rankedSegments(run, value);
  if (segments.length > 0 && !segments.some((segment) => segment.index === selected)) {
    selected = segments[0].index;
  }
  const charts = [
    kindChart(value),
    cqtChart(value),
    segmentChart(run, value, segments),
    breakdownChart(run, value, segments),
    dailyChart(run),
    areaChart(rows, value),
    toolGroupChart(rows, value),
  ];
  $("charts").replaceChildren(...charts.filter(Boolean));
  $("tables").replaceChildren(...TABLES.map((spec) => table(spec, rows, value)).filter(Boolean));

  $("digests").replaceChildren(
    ...run.done.map(({ config, digest, seconds }) => {
      const text = t("results.digest", {
        replication: config.replication,
        digest,
        time: secondsText(seconds),
      });
      return element("li", text);
    }),
  );
  const busy = run.done.reduce((sum, replication) => sum + replication.seconds, 0);
  const events = run.done.reduce((sum, replication) => sum + replication.results.events, 0);
  const memory = Math.max(...run.done.map((replication) => replication.memoryBytes));
  $("performance").textContent = t("results.performance", {
    count: run.count,
    workers: run.threads,
    time: secondsText(run.seconds),
    perReplication: secondsText(busy / run.count),
    events: formatNumber(events / busy / 1e6, 2),
    memory: formatNumber(memory / 1e6, 0),
  });
  if (animated) reveal($("analysis-body").querySelectorAll(".kpi, .chart"));
}

/** Shows the same run and period again (a choice within the charts). */
function redraw() {
  showResults(shown.run, shown.period, false);
}

/** Breaks down the waits of segment `index` and lists its violations in the details. */
function choose(index) {
  selected = index;
  redraw();
  chosen(selected);
}

/** The CQT segments with completions in the period, most violations first. */
function rankedSegments(run, value) {
  const info = run.info;
  const segments = info.segments
    .map((_, index) => {
      const item = segmentKey(info, index);
      return {
        index,
        item,
        share: value("cqt_segment", item, null, "vl_pct"),
        completed: value("cqt_segment", item, null, "completed"),
      };
    })
    .filter((segment) => segment.share);
  const violations = (segment) => segment.share.mean * segment.completed.mean;
  return segments.sort((a, b) => violations(b) - violations(a));
}

/** Share over the limit per CQT segment; a row chooses the segment. */
function segmentChart(run, value, segments) {
  if (segments.length === 0) return null;
  const info = run.info;
  const listed = allSegments ? segments : segments.slice(0, TOP);
  const rows = listed.map((segment) => ({
    label: segmentLabel(info, segment.index),
    values: [segment.share.mean],
    ci: segment.share.ci95,
    text: `${cell(segment.share, 1)} %`,
    title: [
      segmentTitle(info, segment.index),
      `${t("col.completed")}: ${cell(segment.completed, 0)}`,
      `${t("col.vl")}: ${cell(segment.share, 2)} %`,
      `${t("col.avl")}: ${cell(value("cqt_segment", segment.item, null, "avl_h"), 2)}`,
    ].join("\n"),
    selected: segment.index === selected,
  }));
  const result = figure(
    "chart.segments",
    barChart({
      rows,
      parts: [{ kind: "warning", label: t("col.vl") }],
      label: t("chart.segments"),
      onSelect: (row) => choose(listed[row].index),
      animate,
    }),
    element("p", t("chart.segmentsHint"), "hint"),
  );
  result.classList.add("wide");
  if (segments.length > TOP) {
    const toggle = element("button", t(allSegments ? "chart.top" : "chart.all", {
      count: allSegments ? TOP : segments.length,
    }));
    toggle.type = "button";
    toggle.addEventListener("click", () => {
      allSegments = !allSegments;
      redraw();
    });
    result.firstElementChild.append(toggle);
  }
  return result;
}

/** Where the waits of the chosen segment go: hours per visit of each step, over and within the
 * limit; the segment is chosen here too. */
function breakdownChart(run, value, segments) {
  if (selected === null) return null;
  const info = run.info;
  const segment = info.segments[selected];
  const item = segmentKey(info, selected);
  const rows = [];
  for (let step = segment.entry + 1; step <= segment.exit; step++) {
    for (const outcome of ["vl", "ok"]) {
      const parts = PARTS.map(([part]) => ({
        part,
        summary: value("cqt_step", `${item}:${step}`, null, `${part}_${outcome}_h`),
      }));
      if (parts.every((each) => !each.summary)) continue;
      const total = parts.reduce((sum, each) => sum + (each.summary?.mean ?? 0), 0);
      rows.push({
        label: `${stepLabel(info, segment.route, step)} · ${t(`breakdown.${outcome}`)}`,
        values: parts.map((each) => each.summary?.mean ?? 0),
        text: t("unit.hours", { value: formatNumber(total, 2) }),
        title: [
          stepLabel(info, segment.route, step),
          t(`breakdown.${outcome}`),
          ...parts.map((each) => `${t(`part.${each.part}`)}: ${cell(each.summary, 2)} h`),
        ].join("\n"),
      });
    }
  }
  if (rows.length === 0) return null;
  const choice = document.createElement("select");
  choice.setAttribute("aria-label", t("details.segment"));
  for (const each of segments) {
    const text = `${segmentLabel(info, each.index)} (${cell(each.share, 1)} %)`;
    choice.append(new Option(text, String(each.index), false, each.index === selected));
  }
  choice.addEventListener("change", () => choose(Number(choice.value)));
  const result = figure(
    "chart.breakdown",
    element("p", segmentTitle(info, selected), "hint"),
    barChart({
      rows,
      parts: PARTS.map(([part, kind]) => ({ kind, label: t(`part.${part}`) })),
      label: t("chart.breakdown"),
      animate,
    }),
  );
  result.firstElementChild.append(choice);
  result.classList.add("wide");
  return result;
}

/** A measure day by day: mean over the replications with its 95% interval. */
function dailyChart(run) {
  const [scope, measure, decimals] = DAILY[daily];
  const rows = run.daily.filter((row) => row.scope === scope && row.measure === measure);
  if (rows.length < 2) return null;
  const name = t(`daily.${scope}.${measure}`);
  const chart = lineChart({
    series: [
      {
        label: name,
        kind: scope === "cqt" ? "warning" : "process",
        // The measures are not negative, nor is their interval shown below 0.
        points: rows.map((row) =>
          row.ci95 == null
            ? [row.day, row.mean]
            : [row.day, row.mean, Math.max(0, row.mean - row.ci95), row.mean + row.ci95],
        ),
      },
    ],
    x: (day) => t("time.day", { day: formatNumber(day, 0) }),
    y: (number) => formatNumber(number, decimals),
    label: `${t("chart.daily")}: ${name}`,
    animate,
  });
  const choice = document.createElement("select");
  choice.setAttribute("aria-label", t("chart.dailyMeasure"));
  DAILY.forEach(([each, measured], index) => {
    choice.append(new Option(t(`daily.${each}.${measured}`), String(index), false, index === daily));
  });
  choice.addEventListener("change", () => {
    daily = Number(choice.value);
    redraw();
  });
  const result = figure("chart.daily", chart, element("p", t("chart.dailyHint"), "hint"));
  result.firstElementChild.append(choice);
  result.classList.add("wide");
  return result;
}

function key(scope, item, kind, measure) {
  return `${scope}|${item}|${kind}|${measure}`;
}

function kpi(label, summary, decimals, animated) {
  const card = element("div", "", "kpi");
  const value = element("div", "", "value");
  const format = (number) => formatNumber(number, decimals);
  if (animated) {
    countUp(value, summary.mean, format);
  } else {
    value.textContent = format(summary.mean);
  }
  card.append(element("div", label, "hint"), value);
  if (summary.ci95 != null) {
    card.append(element("div", `± ${formatNumber(summary.ci95, decimals)}`, "hint"));
  }
  return card;
}

/** Average cycle time per lot kind. */
function kindChart(value) {
  const rows = KINDS.flatMap((kind) => {
    const summary = value("kind", "", kind, "ct_mean_d");
    if (!summary) return [];
    return [
      {
        label: kind,
        values: [summary.mean],
        ci: summary.ci95,
        text: cell(summary, 1),
        title: `${t(`kind.${kind}`)}\n${t("col.ctMean")}: ${cell(summary, 2)}`,
      },
    ];
  });
  if (rows.length === 0) return null;
  const parts = [{ kind: "process", label: t("col.ctMean") }];
  return figure("chart.kinds", barChart({ rows, parts, label: t("chart.kinds"), animate }));
}

/** Share of CQT intervals over the limit, stepper intervals and the others. */
function cqtChart(value) {
  const rows = ["litho", "rest", "total"].flatMap((item) => {
    const summary = value("cqt", item, null, "vl_pct");
    if (!summary) return [];
    return [
      {
        label: t(`cqt.${item}`),
        values: [summary.mean],
        ci: summary.ci95,
        text: `${cell(summary, 1)} %`,
        title: `${t(`cqt.${item}`)}\n${t("col.vl")}: ${cell(summary, 2)} %`,
      },
    ];
  });
  if (rows.length === 0) return null;
  const parts = [{ kind: "warning", label: t("col.vl") }];
  return figure("chart.cqt", barChart({ rows, parts, label: t("chart.cqt"), animate }));
}

/** Tool time by state of the busiest tool groups, or of all. */
function toolGroupChart(rows, value) {
  const groups = rows
    .filter((row) => row.scope === "tool_group" && row.measure === "util_pct")
    .sort((a, b) => b.mean - a.mean);
  if (groups.length === 0) return null;
  const listed = allToolGroups ? groups : groups.slice(0, TOP);
  const bars = barChart({
    rows: listed.map(({ item, mean }) => {
      const shares = STATES.map((state) => value("tool_group", item, null, `${state}_pct`));
      return {
        label: item,
        values: shares.map((summary) => summary?.mean ?? 0),
        text: `${formatNumber(mean, 1)} %`,
        title: [
          item,
          `${t("col.util")}: ${cell(value("tool_group", item, null, "util_pct"), 2)}`,
          ...STATES.map((state, index) => `${t(`col.${state}`)}: ${cell(shares[index], 2)} %`),
        ].join("\n"),
      };
    }),
    parts: STATES.map((state) => ({ kind: state, label: t(`col.${state}`) })),
    max: 100,
    label: t("chart.toolGroups"),
    animate,
  });
  const result = figure("chart.toolGroups", bars);
  if (groups.length > TOP) {
    const toggle = element("button", t(allToolGroups ? "chart.top" : "chart.all", {
      count: allToolGroups ? TOP : groups.length,
    }));
    toggle.type = "button";
    toggle.addEventListener("click", () => {
      allToolGroups = !allToolGroups;
      result.replaceWith(toolGroupChart(rows, value));
    });
    result.firstElementChild.append(toggle);
  }
  result.classList.add("wide");
  return result;
}

/** Utilization per area. */
function areaChart(rows, value) {
  const areas = rows.filter((row) => row.scope === "area" && row.measure === "util_pct");
  if (areas.length === 0) return null;
  const bars = barChart({
    rows: areas.map((summary) => {
      const other = (measure) => cell(value("area", summary.item, null, measure), 2);
      return {
        label: summary.item,
        values: [summary.mean],
        ci: summary.ci95,
        text: `${formatNumber(summary.mean, 1)} %`,
        title: [
          summary.item,
          `${t("col.util")}: ${cell(summary, 2)}`,
          `${t("col.availability")}: ${other("availability_pct")}`,
          `${t("col.utilMax")}: ${other("util_max_pct")}`,
        ].join("\n"),
      };
    }),
    parts: [{ kind: "process", label: t("col.util") }],
    max: 100,
    label: t("chart.areas"),
    animate,
  });
  const result = figure("chart.areas", bars);
  result.classList.add("wide");
  return result;
}

function figure(title, ...content) {
  const caption = document.createElement("figcaption");
  caption.append(element("span", t(title)));
  const result = element("figure", "", "chart");
  result.append(caption, ...content.filter(Boolean));
  return result;
}

/** The table of `spec` for the period's rows; none if no row has its measures. */
function table(spec, rows, value) {
  const keys = new Map();
  for (const row of rows.filter((row) => row.scope === spec.scope)) {
    keys.set(`${row.item}|${row.kind}`, [row.item, row.kind]);
  }
  const items = [...keys.values()].filter(([item, kind]) =>
    spec.columns.some(([measure]) => value(spec.scope, item, kind, measure)),
  );
  if (items.length === 0) return null;
  if (spec.sortBy) {
    const order = ([item, kind]) => value(spec.scope, item, kind, spec.sortBy)?.mean ?? -Infinity;
    items.sort((a, b) => order(b) - order(a));
  } else if (spec.scope === "kind") {
    items.sort(([, a], [, b]) => KINDS.indexOf(a) - KINDS.indexOf(b));
  }

  const grid = document.createElement("table");
  const head = grid.createTHead().insertRow();
  const labels = spec.columns.map(([, label, , params]) => t(label, params));
  for (const label of [t(`table.${spec.name}.item`), ...labels]) {
    head.append(Object.assign(element("th", label), { scope: "col" }));
  }
  const body = grid.createTBody();
  for (const [item, kind] of items) {
    const row = body.insertRow();
    const label = spec.scope === "cqt" ? t(`cqt.${item}`) : [item, kind].filter(Boolean).join(" ");
    row.append(Object.assign(element("th", label), { scope: "row" }));
    for (const [measure, , decimals] of spec.columns) {
      row.insertCell().textContent = cell(value(spec.scope, item, kind, measure), decimals);
    }
  }
  const scroll = element("div", "", "scroll");
  scroll.append(grid);
  const section = element("section", "", "table");
  section.append(
    element("h3", t(`table.${spec.name}.title`)),
    element("p", t(`table.${spec.name}.note`), "hint"),
    scroll,
  );
  return section;
}

/** Mean ± the 95% confidence interval half-width; the mean alone for one replication. */
function cell(summary, decimals) {
  if (!summary) return "–";
  const mean = formatNumber(summary.mean, decimals);
  return summary.ci95 == null ? mean : `${mean} ± ${formatNumber(summary.ci95, decimals)}`;
}

function secondsText(seconds) {
  return `${formatNumber(seconds, 1)} s`;
}

function element(tag, text, className) {
  const node = document.createElement(tag);
  node.textContent = text;
  if (className) node.className = className;
  return node;
}
