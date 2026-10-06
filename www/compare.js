// The Compare view: the scenarios of a run side by side, under the same settings and random
// numbers. Each scenario is compared with the baseline replication pair by replication pair (the
// core's compare): the mean difference and its 95% confidence interval show whether a strategy
// changed a measure beyond chance.
import { compare, comparisonCsv } from "./pkg/fab_wasm.js";
import { barChart } from "./charts.js";
import { download } from "./files.js";
import { formatNumber, t } from "./i18n.js";
import { lookup, verdict } from "./kpis.js";
import { periodLabel, segmentKey, segmentLabel, segmentTitle } from "./labels.js";
import { reveal } from "./motion.js";

/** Measures compared: label key, scope, item, kind, measure, decimals and which way is better
 * (-1: lower). */
const MEASURES = [
  { label: "kpi.cqt", scope: "cqt", item: "total", measure: "vl_pct", decimals: 2, better: -1 },
  { label: "compare.cqtLitho", scope: "cqt", item: "litho", measure: "vl_pct", decimals: 2, better: -1 },
  { label: "compare.cqtRest", scope: "cqt", item: "rest", measure: "vl_pct", decimals: 2, better: -1 },
  { label: "compare.avl", scope: "cqt", item: "total", measure: "avl_h", decimals: 3, better: -1 },
  { label: "kpi.completed", scope: "fab", measure: "completed", decimals: 0, better: 1 },
  { label: "kpi.wip", scope: "fab", measure: "wip", decimals: 0, better: -1 },
  { label: "kpi.prlCt", scope: "kind", kind: "PRL", measure: "ct_mean_d", decimals: 2, better: -1 },
  { label: "kpi.prlOnTime", scope: "kind", kind: "PRL", measure: "on_time_pct", decimals: 1, better: 1 },
  { label: "kpi.erlCt", scope: "kind", kind: "ERL", measure: "ct_mean_d", decimals: 2, better: -1 },
];
/** Run settings the scenarios share, as the Setup view describes them. */
const SHARED = [
  "dataset.heading",
  "settings.horizon.label",
  "settings.warmUp.label",
  "settings.replications.label",
  "settings.seed.label",
  "settings.load.label",
];
/** CQT segments in the segment table. */
const SEGMENTS = 10;

const $ = (id) => document.getElementById(id);
/** The run compared and the choices made: baseline, period, measure of the chart. */
let shown = null;

/** Shows the scenarios of `finished` (a finished run of several scenarios). */
export function showComparison(finished, animated) {
  const periods = finished.scenarios[0].done[0].results.periods;
  shown = {
    finished,
    baseline: 0,
    period: (periods.at(-2) ?? periods[0]).name,
    measure: 0,
    comparisons: new Map(),
  };
  $("compare-empty").hidden = true;
  $("compare-body").hidden = false;
  render(animated);
}

/** Draws the comparison again (a new language). */
export function renderComparison() {
  if (shown) render(false);
}

/** The comparison rows of scenario `index` against the baseline, by key. */
function comparisonOf(index) {
  const key = `${shown.baseline}|${index}`;
  if (!shown.comparisons.has(key)) {
    const results = (scenario) => scenario.done.map((replication) => replication.results);
    const scenarios = shown.finished.scenarios;
    const rows = compare(results(scenarios[shown.baseline]), results(scenarios[index]));
    shown.comparisons.set(key, rows);
  }
  return shown.comparisons.get(key);
}

function render(animated) {
  const { finished, period } = shown;
  const scenarios = finished.scenarios;
  const baseline = scenarios[shown.baseline];
  const summaries = scenarios.map((scenario) => lookup(scenario.summary, period));
  const differences = scenarios.map((_, index) =>
    index === shown.baseline ? null : lookup(comparisonOf(index), period),
  );

  // Toolbar: the baseline, the period, the download.
  const baselineChoice = select(
    t("compare.baseline"),
    scenarios.map((scenario, index) => [String(index), scenario.name]),
    String(shown.baseline),
    (value) => {
      shown.baseline = Number(value);
      render(false);
    },
  );
  const periodChoice = select(
    t("results.period"),
    baseline.done[0].results.periods.map((each) => [each.name, periodLabel(each)]),
    period,
    (value) => {
      shown.period = value;
      render(false);
    },
  );
  const csvButton = button(t("results.downloadCsv"), downloadCsv);
  $("compare-tools").replaceChildren(
    labelled(t("compare.baseline"), baselineChoice),
    labelled(t("results.period"), periodChoice),
    csvButton,
  );
  $("compare-setup").replaceChildren(
    ...baseline.setup
      .filter(([label]) => SHARED.includes(label))
      .map(([label, text]) => el("div", {}, el("dt", {}, t(label)), el("dd", {}, text()))),
  );

  // Strategies that changed nothing: every replication identical to the baseline's.
  const same = scenarios.filter(
    (scenario, index) =>
      index !== shown.baseline && scenario.done.every((each, at) => each.digest === baseline.done[at].digest),
  );
  $("compare-same").hidden = same.length === 0;
  $("compare-same").textContent = t("compare.same", { names: same.map((scenario) => scenario.name).join(", ") });

  $("compare-table").replaceChildren(measureTable(summaries, differences));
  $("compare-charts").replaceChildren(measureChart(summaries, animated), segmentTable(summaries, differences));
  if (animated) reveal($("compare-body").querySelectorAll(".chart, .table"));
}

/** Measures × scenarios: the means and, against the baseline, the paired differences. */
function measureTable(summaries, differences) {
  const scenarios = shown.finished.scenarios;
  const head = el(
    "tr",
    {},
    el("th", { scope: "col", class: "text" }, t("compare.measure")),
    ...scenarios.map((scenario, index) =>
      el("th", { scope: "col" }, scenario.name, index === shown.baseline ? el("span", { class: "tag" }, t("scenarios.baseline")) : ""),
    ),
  );
  const body = el("tbody");
  for (const spec of MEASURES) {
    if (!summaries[shown.baseline](spec)) continue;
    body.append(
      el(
        "tr",
        {},
        el("th", { scope: "row", class: "text" }, t(spec.label)),
        ...scenarios.map((_, index) => {
          const summary = summaries[index](spec);
          const cell = el("td", {}, summary ? meanText(summary, spec.decimals) : "–");
          const difference = differences[index]?.(spec);
          if (difference) cell.append(el("br"), differenceText(difference, spec));
          return cell;
        }),
      ),
    );
  }
  return el(
    "section",
    { class: "table" },
    el("p", { class: "hint" }, t("compare.tableHint")),
    el("div", { class: "scroll" }, el("table", { class: "compare" }, el("thead", {}, head), body)),
  );
}

/** One measure per scenario with its interval; the measure is chosen here. */
function measureChart(summaries, animated) {
  const spec = MEASURES[shown.measure];
  const scenarios = shown.finished.scenarios;
  const rows = scenarios.flatMap((scenario, index) => {
    const summary = summaries[index](spec);
    if (!summary) return [];
    return [
      {
        label: scenario.name,
        values: [summary.mean],
        ci: summary.ci95,
        text: meanText(summary, spec.decimals),
        title: `${scenario.name}\n${t(spec.label)}: ${meanText(summary, spec.decimals)}`,
        selected: index === shown.baseline,
      },
    ];
  });
  const choice = select(
    t("chart.dailyMeasure"),
    MEASURES.map((each, index) => [String(index), t(each.label)]).filter(([index]) =>
      summaries[shown.baseline](MEASURES[Number(index)]),
    ),
    String(shown.measure),
    (value) => {
      shown.measure = Number(value);
      render(false);
    },
  );
  const caption = el("figcaption", {}, el("span", {}, t("compare.chart")), choice);
  const kind = spec.scope === "cqt" ? "warning" : "process";
  const chart = barChart({ rows, parts: [{ kind, label: t(spec.label) }], label: t(spec.label), animate: animated });
  return el("figure", { class: "chart wide" }, caption, chart);
}

/** The baseline's most violated CQT segments: the share over the limit per scenario. */
function segmentTable(summaries, differences) {
  const info = shown.finished.info;
  const spec = (index) => ({ scope: "cqt_segment", item: segmentKey(info, index), kind: null, measure: "vl_pct" });
  const completed = (index) =>
    summaries[shown.baseline]({ ...spec(index), measure: "completed" })?.mean ?? 0;
  const ranked = info.segments
    .map((_, index) => index)
    .filter((index) => summaries[shown.baseline](spec(index)))
    .sort((a, b) => summaries[shown.baseline](spec(b)).mean * completed(b) - summaries[shown.baseline](spec(a)).mean * completed(a))
    .slice(0, SEGMENTS);
  if (ranked.length === 0) return el("span");
  const scenarios = shown.finished.scenarios;
  const head = el(
    "tr",
    {},
    el("th", { scope: "col", class: "text" }, t("details.segment")),
    ...scenarios.map((scenario) => el("th", { scope: "col" }, scenario.name)),
  );
  const body = el("tbody");
  for (const segment of ranked) {
    const measure = { ...MEASURES[0], ...spec(segment) };
    body.append(
      el(
        "tr",
        {},
        el("th", { scope: "row", class: "text", title: segmentTitle(info, segment) }, segmentLabel(info, segment)),
        ...scenarios.map((_, index) => {
          const summary = summaries[index](spec(segment));
          const cell = el("td", {}, summary ? `${formatNumber(summary.mean, 1)} %` : "–");
          const difference = differences[index]?.(spec(segment));
          if (difference) cell.append(el("br"), differenceText(difference, { ...measure, decimals: 1 }));
          return cell;
        }),
      ),
    );
  }
  return el(
    "section",
    { class: "chart wide" },
    el("h3", {}, t("compare.segments", { count: ranked.length })),
    el("p", { class: "hint" }, t("compare.segmentsHint")),
    el("div", { class: "scroll" }, el("table", { class: "compare" }, el("thead", {}, head), body)),
  );
}

/** Mean ± interval. */
function meanText(summary, decimals) {
  const mean = formatNumber(summary.mean, decimals);
  return summary.ci95 == null ? mean : `${mean} ± ${formatNumber(summary.ci95, decimals)}`;
}

/** The paired difference, marked better or worse when its interval excludes 0. */
function differenceText(row, spec) {
  const sign = row.difference > 0 ? "+" : row.difference < 0 ? "−" : "±";
  const interval = row.ci95 == null ? "" : ` ± ${formatNumber(row.ci95, spec.decimals)}`;
  const text = `Δ ${sign}${formatNumber(Math.abs(row.difference), spec.decimals)}${interval}`;
  const judged = verdict(row, spec.better);
  const span = el("span", { class: `difference ${judged}` }, text);
  span.title = t(`verdict.${judged}.tip`);
  return span;
}

/** The comparisons of every scenario with the baseline, as CSV with a scenario column. */
function downloadCsv() {
  const scenarios = shown.finished.scenarios;
  const lines = [];
  scenarios.forEach((scenario, index) => {
    if (index === shown.baseline) return;
    const [header, ...rows] = comparisonCsv(comparisonOf(index)).trimEnd().split("\n");
    if (lines.length === 0) lines.push(`scenario,baseline_scenario,${header}`);
    const prefix = `${field(scenario.name)},${field(scenarios[shown.baseline].name)},`;
    for (const row of rows) lines.push(prefix + row);
  });
  download(`${shown.finished.name}-comparison.csv`, `${lines.join("\n")}\n`, "text/csv");
}

/** A CSV field, quoted if it holds a comma, quote or line break. */
function field(text) {
  return /[",\n\r]/.test(text) ? `"${text.replace(/"/g, '""')}"` : text;
}

function select(label, options, value, onChange) {
  const choice = el("select", { "aria-label": label });
  for (const [option, text] of options) choice.append(new Option(text, option, false, option === value));
  choice.addEventListener("change", () => onChange(choice.value));
  return choice;
}

function labelled(text, control) {
  return el("label", { class: "field" }, el("span", {}, text), control);
}

function button(text, onClick) {
  const result = el("button", { type: "button" }, text);
  result.addEventListener("click", onClick);
  return result;
}

/** An element with attributes and children (strings become text). */
function el(tag, attributes = {}, ...children) {
  const node = document.createElement(tag);
  for (const [name, value] of Object.entries(attributes)) {
    if (value !== "") node.setAttribute(name, value);
  }
  node.append(...children);
  return node;
}
