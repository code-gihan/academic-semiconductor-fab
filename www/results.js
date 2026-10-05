// Results of a finished run: its setup, the headline measures and tables of a report period, and
// the replications' digests and run times. Values are replication means ± the half-width of their
// 95% confidence interval.
import { formatNumber, t } from "./i18n.js";

const DAY = 86_400_000;
const KINDS = ["PRL", "PHL", "SHL", "ERL", "EHL"];

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

/** Name of the last report period before Drain: the longest window inside the horizon. */
export function defaultPeriod(run) {
  const periods = run.done[0].results.periods;
  return (periods.at(-2) ?? periods[0]).name;
}

/** Shows `run` (a finished run of the page) for the report period named `period`. */
export function showResults(run, period) {
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
      return summary ? [kpi(t(spec.label), summary, spec.decimals)] : [];
    }),
  );
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
}

function key(scope, item, kind, measure) {
  return `${scope}|${item}|${kind}|${measure}`;
}

function kpi(label, summary, decimals) {
  const card = element("div", "", "kpi");
  card.append(
    element("div", label, "hint"),
    element("div", formatNumber(summary.mean, decimals), "value"),
  );
  if (summary.ci95 != null) {
    card.append(element("div", `± ${formatNumber(summary.ci95, decimals)}`, "hint"));
  }
  return card;
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
