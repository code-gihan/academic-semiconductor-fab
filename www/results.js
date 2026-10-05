// Results of a finished run: its setup, the headline measures, charts and tables of a report
// period, and the replications' digests and run times. Values are replication means ± the
// half-width of their 95% confidence interval.
import { barChart, legend } from "./charts.js";
import { formatNumber, t } from "./i18n.js";
import { countUp, grow, reveal } from "./motion.js";

const DAY = 86_400_000;
const KINDS = ["PRL", "PHL", "SHL", "ERL", "EHL"];
/** Tool states as stacked in the tool group chart: busy, then outages, then idle. */
const STATES = ["process", "setup", "load", "unload", "down", "pm", "idle"];
/** Tool groups the chart shows until all are asked for. */
const TOP = 15;

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
/** The tool group chart shows every group. */
let allToolGroups = false;

/** Name of the last report period before Drain: the longest window inside the horizon. */
export function defaultPeriod(run) {
  const periods = run.done[0].results.periods;
  return (periods.at(-2) ?? periods[0]).name;
}

/** Shows `run` (a finished run of the page) for the report period named `period`; `animated`
 * plays the entrance, as for new values (not for a new language). */
export function showResults(run, period, animated) {
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
  // Lot outcomes side by side, then the capacity by area and by tool group, a row each.
  const charts = [
    kindChart(value),
    cqtChart(value),
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
  if (animated) {
    reveal($("results-body").querySelectorAll(".kpi, .chart"));
    grow($("charts").querySelectorAll(".bar-fill, .bar-ci"));
  }
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
        segments: [{ value: summary.mean, kind: "measure" }],
        ci: summary.ci95,
        value: cell(summary, 1),
        title: `${t(`kind.${kind}`)}\n${t("col.ctMean")}: ${cell(summary, 2)}`,
      },
    ];
  });
  return rows.length > 0 ? figure("chart.kinds", barChart(rows, scaleOf(rows))) : null;
}

/** Share of CQT intervals over the limit, stepper intervals and the others. */
function cqtChart(value) {
  const rows = ["litho", "rest", "total"].flatMap((item) => {
    const summary = value("cqt", item, null, "vl_pct");
    if (!summary) return [];
    return [
      {
        label: t(`cqt.${item}`),
        segments: [{ value: summary.mean, kind: "warning" }],
        ci: summary.ci95,
        value: `${cell(summary, 1)} %`,
        title: `${t(`cqt.${item}`)}\n${t("col.vl")}: ${cell(summary, 2)} %`,
      },
    ];
  });
  return rows.length > 0 ? figure("chart.cqt", barChart(rows, scaleOf(rows))) : null;
}

/** Tool time by state of the busiest tool groups, or of all. */
function toolGroupChart(rows, value) {
  const groups = rows
    .filter((row) => row.scope === "tool_group" && row.measure === "util_pct")
    .sort((a, b) => b.mean - a.mean);
  if (groups.length === 0) return null;
  const shown = allToolGroups ? groups : groups.slice(0, TOP);
  const bars = barChart(
    shown.map(({ item, mean }) => {
      const share = (state) => value("tool_group", item, null, `${state}_pct`);
      const shares = STATES.map((state) => [state, share(state)]);
      return {
        label: item,
        segments: shares.map(([state, summary]) => ({ value: summary?.mean ?? 0, kind: state })),
        value: `${formatNumber(mean, 1)} %`,
        title: [
          item,
          `${t("col.util")}: ${cell(value("tool_group", item, null, "util_pct"), 2)}`,
          ...shares.map(([state, summary]) => `${t(`col.${state}`)}: ${cell(summary, 2)} %`),
        ].join("\n"),
      };
    }),
    100,
  );
  const result = figure("chart.toolGroups", bars, legend(STATES, (state) => t(`col.${state}`)));
  if (groups.length > TOP) {
    const toggle = element("button", t(allToolGroups ? "chart.top" : "chart.all", {
      count: allToolGroups ? TOP : groups.length,
    }));
    toggle.type = "button";
    toggle.addEventListener("click", () => {
      allToolGroups = !allToolGroups;
      const next = toolGroupChart(rows, value);
      result.replaceWith(next);
      grow(next.querySelectorAll(".bar-fill"));
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
  const bars = barChart(
    areas.map((summary) => {
      const other = (measure) => cell(value("area", summary.item, null, measure), 2);
      return {
        label: summary.item,
        segments: [{ value: summary.mean, kind: "measure" }],
        ci: summary.ci95,
        value: `${formatNumber(summary.mean, 1)} %`,
        title: [
          summary.item,
          `${t("col.util")}: ${cell(summary, 2)}`,
          `${t("col.availability")}: ${other("availability_pct")}`,
          `${t("col.utilMax")}: ${other("util_max_pct")}`,
        ].join("\n"),
      };
    }),
    100,
  );
  const result = figure("chart.areas", bars);
  result.classList.add("wide");
  return result;
}

/** A scale a little above the largest bar with its interval. */
function scaleOf(rows) {
  return Math.max(...rows.map((row) => row.segments[0].value + (row.ci ?? 0))) * 1.08 || 1;
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
