// The papers' key measures of a run as tiles: four main ones large ([P2] CQT violations, [P1]
// cycle time, throughput and on-time delivery) and details small. A tile shows the mean of the
// core's replication summary with its interval or, against a baseline, the core's paired
// difference and its verdict, which the Compare view's differences share.
import { formatNumber, t } from "./i18n.js";
import { countUp } from "./motion.js";
import { infoButton, withTooltip } from "./tooltip.js";

/** Main measures: summary fields, decimals, unit and which way is better (-1: lower). */
export const MAIN = [
  { key: "cqt", scope: "cqt", item: "total", measure: "vl_pct", decimals: 1, unit: "percent", better: -1 },
  { key: "ct", scope: "kind", kind: "PRL", measure: "ct_mean_d", decimals: 1, unit: "days", better: -1 },
  { key: "th", scope: "fab", measure: "completed", decimals: 0, unit: "lots", better: 1 },
  { key: "onTime", scope: "kind", kind: "PRL", measure: "on_time_pct", decimals: 1, unit: "percent", better: 1 },
];

/** Detail measures, shown small when the period has them. */
export const DETAIL = [
  { key: "started", scope: "fab", measure: "started", decimals: 0, unit: "lots" },
  { key: "wip", scope: "fab", measure: "wip", decimals: 0, unit: "lots" },
  { key: "ff", scope: "kind", kind: "PRL", measure: "ff_mean", decimals: 2 },
  { key: "erlCt", scope: "kind", kind: "ERL", measure: "ct_mean_d", decimals: 1, unit: "days" },
  { key: "vl1h", scope: "cqt", item: "total", measure: "vl1h_pct", decimals: 1, unit: "percent" },
  { key: "avl", scope: "cqt", item: "total", measure: "avl_h", decimals: 2, unit: "hours" },
  { key: "aont", scope: "cqt", item: "total", measure: "aont_h", decimals: 2, unit: "hours" },
  { key: "t2t", scope: "amhs", measure: "t2t_pct", decimals: 1, unit: "percent" },
  { key: "delivery", scope: "amhs", measure: "delivery_s", decimals: 1, unit: "seconds" },
  { key: "vehicleBusy", scope: "amhs", measure: "vehicle_busy_pct", decimals: 1, unit: "percent" },
];

/** The key of a summary or comparison row, or of a measure spec. */
export function rowKey(row) {
  return `${row.scope}|${row.item ?? ""}|${row.kind ?? null}|${row.measure}`;
}

/** The rows of `period` by measure: `find(spec)` is the row of a spec, if any. */
export function lookup(rows, period) {
  const map = new Map(rows.filter((row) => row.period === period).map((row) => [rowKey(row), row]));
  return (spec) => map.get(rowKey(spec));
}

/**
 * Whether a paired difference (a comparison row) is better or worse than the baseline beyond
 * chance (its 95% interval excludes 0), within chance, or untested (one pair, no interval).
 */
export function verdict(row, better) {
  if (row.ci95 == null) return "untested";
  if (Math.abs(row.difference) <= row.ci95) return "chance";
  return Math.sign(row.difference) * better > 0 ? "better" : "worse";
}

/** A value of `spec` with its unit, as text. */
export function valueText(spec, value) {
  const number = formatNumber(value, spec.decimals);
  return spec.unit ? t(`kpi.suffix.${spec.unit}`, { value: number }) : number;
}

/**
 * The tile of measure `spec` with `summary`'s mean: with `difference` (the comparison row against
 * a baseline) the difference and its verdict, otherwise the mean's 95% interval; `note` is a
 * line below it (the baseline's value, say). `animated` counts the value up after `delay` ms.
 */
export function tile(spec, summary, { difference, note, animated, delay = 0 } = {}) {
  const number = element("span", "kpi-number");
  const format = (value) => formatNumber(value, spec.decimals);
  if (animated) countUp(number, summary.mean, format, delay);
  else number.textContent = format(summary.mean);
  const value = element("div", "kpi-value");
  value.append(number);
  if (spec.unit) value.append(element("span", "kpi-unit", t(`kpi.unit.${spec.unit}`)));

  const head = element("div", "kpi-head");
  head.append(element("span", "kpi-label", t(`measure.${spec.key}`)), infoButton(() => t(`measure.${spec.key}.info`)));

  const card = element("article", "kpi");
  card.append(head, value);
  if (difference) {
    const judged = verdict(difference, spec.better);
    card.dataset.verdict = judged;
    card.append(delta(spec, difference, judged));
  } else if (summary.ci95 != null) {
    const interval = element("div", "kpi-note", `± ${valueText(spec, summary.ci95)}`);
    interval.tabIndex = 0;
    withTooltip(interval, () => t("results.ci", { runs: summary.n }));
    card.append(interval);
  }
  if (note) card.append(element("div", "kpi-note", note));
  return card;
}

/** The difference to the baseline: an arrow, the amount and the verdict in words. */
function delta(spec, row, judged) {
  const amount = formatNumber(Math.abs(row.difference), spec.decimals);
  const sign = row.difference > 0 ? "+" : row.difference < 0 ? "−" : "±";
  const text = spec.unit ? t(`kpi.delta.${spec.unit}`, { value: amount }) : amount;
  const arrow = element("span", "kpi-arrow", row.difference > 0 ? "▲" : row.difference < 0 ? "▼" : "■");
  arrow.setAttribute("aria-hidden", "true");
  const line = element("div", "kpi-delta");
  line.append(
    arrow,
    element("span", "kpi-amount", `${sign}${text}`),
    element("span", "kpi-verdict", t(`verdict.${judged}`)),
  );
  line.tabIndex = 0;
  withTooltip(line, () =>
    [
      t("kpi.deltaTip", {
        difference: `${sign}${amount}`,
        ci: row.ci95 == null ? "–" : formatNumber(row.ci95, spec.decimals),
        n: row.n,
      }),
      t(`verdict.${judged}.tip`),
    ].join("\n"),
  );
  return line;
}

/** A small measure: its label and mean ± interval, in its unit. */
export function stat(spec, summary) {
  const mean = formatNumber(summary.mean, spec.decimals);
  const text = summary.ci95 == null ? mean : `${mean} ± ${formatNumber(summary.ci95, spec.decimals)}`;
  const pair = document.createElement("div");
  pair.append(
    element("dt", "", t(`measure.${spec.key}`)),
    element("dd", "", spec.unit ? t(`kpi.suffix.${spec.unit}`, { value: text }) : text),
  );
  return pair;
}

function element(tag, className, text) {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text != null) node.textContent = text;
  return node;
}
