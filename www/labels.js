// Names of what the dataset info indexes (CQT segments, steps, tool groups) and of simulated
// times, in the page's language.
import { formatNumber, t } from "./i18n.js";

export const DAY = 86_400_000;
export const HOUR = 3_600_000;

/** The summary item of segment `index` (report scope cqt_segment). */
export function segmentKey(info, index) {
  const segment = info.segments[index];
  return `${info.routes[segment.route].name}:${segment.entry}-${segment.exit}`;
}

/** Segment `index` in short: its route, steps and the tool groups it runs between. */
export function segmentLabel(info, index) {
  const segment = info.segments[index];
  const steps = info.routes[segment.route].steps;
  return t("segment.label", {
    route: info.routes[segment.route].name,
    entry: steps[segment.entry].name,
    exit: steps[segment.exit].name,
    from: info.tool_groups[steps[segment.entry].tool_group].name,
    to: info.tool_groups[steps[segment.exit].tool_group].name,
  });
}

/** Segment `index` in full: route, steps, tool groups and limit. */
export function segmentTitle(info, index) {
  const segment = info.segments[index];
  const steps = info.routes[segment.route].steps;
  return t("segment.title", {
    route: info.routes[segment.route].name,
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
