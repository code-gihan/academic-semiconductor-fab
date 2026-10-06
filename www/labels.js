// Names of what the dataset info indexes (CQT segments, steps, tool groups), of lot kinds, report
// periods and simulated times, in the page's language.
import { formatNumber, t } from "./i18n.js";

export const DAY = 86_400_000;
export const HOUR = 3_600_000;

/** The summary item of segment `index` (report scope cqt_segment). */
export function segmentKey(info, index) {
  const segment = info.segments[index];
  return `${info.routes[segment.route].name}:${segment.entry}-${segment.exit}`;
}

/** The products made on route `route`: their names, or the route's if none. */
export function routeName(info, route) {
  const parts = info.parts.filter((part) => part.route === route).map((part) => part.name);
  return parts.length > 0 ? parts.join("/") : info.routes[route].name;
}

/** Segment `index` in short: its product, steps and the tool groups it runs between. */
export function segmentLabel(info, index) {
  const segment = info.segments[index];
  const steps = info.routes[segment.route].steps;
  return t("segment.label", {
    product: routeName(info, segment.route),
    entry: steps[segment.entry].name,
    exit: steps[segment.exit].name,
    from: info.tool_groups[steps[segment.entry].tool_group].name,
    to: info.tool_groups[steps[segment.exit].tool_group].name,
  });
}

/** Segment `index` in full: product, steps, tool groups and limit. */
export function segmentTitle(info, index) {
  const segment = info.segments[index];
  const steps = info.routes[segment.route].steps;
  return t("segment.title", {
    product: routeName(info, segment.route),
    entry: steps[segment.entry].name,
    exit: steps[segment.exit].name,
    from: info.tool_groups[steps[segment.entry].tool_group].name,
    to: info.tool_groups[steps[segment.exit].tool_group].name,
    limit: hours(segment.limit),
  });
}

/** Step `step` of route `route`: its name and tool group. */
export function stepLabel(info, route, step) {
  const spec = info.routes[route].steps[step];
  return t("step.label", { step: spec.name, group: info.tool_groups[spec.tool_group].name });
}

/** A lot kind (PRL, PHL, SHL, ERL, EHL) in words, short. */
export function kindLabel(kind) {
  return t(`kind.short.${kind}`);
}

/** A report period ({name, start, end}, times in ms): what it covers and its days. The warm-up
 * is discarded, Period_n measures from the warm-up's end on, Drain holds the lots completed after
 * the end time. */
export function periodLabel({ name, start, end }) {
  const days = { start: formatNumber(start / DAY, 0), end: formatNumber(end / DAY, 0) };
  if (name === "WarmUp") return t("period.warmUp", days);
  if (name === "Drain") return t("period.drain", days);
  return t("period.measured", days);
}

/** A duration in hours. */
export function hours(ms) {
  return t("unit.hours", { value: formatNumber(ms / HOUR, 1) });
}

/** A simulated time as its day and clock time. */
export function at(ms) {
  const day = Math.floor(ms / DAY);
  const minutes = Math.floor((ms - day * DAY) / 60_000);
  const clock = `${String(Math.floor(minutes / 60)).padStart(2, "0")}:${String(minutes % 60).padStart(2, "0")}`;
  return t("time.at", { day: formatNumber(day, 0), clock });
}
