// The details of one run (replication) of a finished run: its CQT violations by segment and week
// and one by one, a tool group day by day, the events of a window and the history of a violating
// lot. The first run records its violations and tool group days as it runs; the others, the
// event windows and the lot histories come from re-runs on the worker pool, which run the same
// as the run (deterministic; a re-run's digest must equal the run's).
import { chartGroup, heatmap, lineChart, timeline } from "./charts.js";
import { download } from "./files.js";
import { formatNumber, t } from "./i18n.js";
import { DAY, HOUR, at, hours, kindLabel, segmentLabel, segmentTitle, stepLabel } from "./labels.js";
import * as pool from "./pool.js";
import { infoButton } from "./tooltip.js";

/** Rows of a table page; segments in the heatmap; replications whose records are kept. */
const PAGE = 25;
const SEGMENTS = 20;
const KEEP = 3;
/** Event windows offered (days). */
const WINDOWS = [1, 2, 3, 7];
/** Orders of the violations. */
const SORTS = ["excess", "early", "late"];
/** Table columns of text (left-aligned); the others are numbers and times. */
const TEXT_COLUMNS = new Set(["lot", "kind", "part", "segment", "event", "step", "tool", "toolGroup"]);
/** Parts of a lot's visit to a step and their colours. */
const PARTS = [
  ["transport", "transport"],
  ["queue", "warning"],
  ["process", "process"],
];

const $ = (id) => document.getElementById(id);
/** The run shown (its `details` hold the recordings) and the choices made in the details. */
let run = null;
let view = null;

/** Records replication 0 runs with: what the details show without a replay. */
export const RECORDING = { violations: true, tool_groups: true };

/** Shows the details of `finished` (a finished run of the page) for CQT segment `segment`. */
export function showDetails(finished, segment) {
  if (run) pool.cancel(run.details);
  run = finished;
  const firstTool = [];
  run.info.tool_groups.reduce((first, group) => {
    firstTool.push(first);
    return first + group.tools;
  }, 0);
  run.details = { replays: new Map(), windows: new Map(), traces: new Map(), firstTool };
  const first = run.done[0];
  if (first.records) run.details.replays.set(0, { status: "ready", records: first.records });
  view = {
    replication: 0,
    segment,
    week: null,
    kind: "",
    part: "",
    sort: SORTS[0],
    page: 0,
    group: exitGroup(segment),
    window: null,
    draft: null,
    eventPage: 0,
    trace: null,
  };
  $("details").hidden = false;
  replay(0);
  render();
}

/** Filters the details by CQT segment `segment` (chosen in the overview). */
export function chooseSegment(segment) {
  if (!run) return;
  view = { ...view, segment, week: null, page: 0, group: exitGroup(segment), draft: null };
  render();
}

/** Draws the details again (a new language). */
export function renderDetails() {
  if (run) render();
}

// ---- recordings ----

/** Replication `index`'s configuration, with the QTS flow factors it measured so that a replay
 * runs in one pass. */
function replayConfig(index) {
  const done = run.done[index];
  return done.flowFactors && !done.config.flow_factors
    ? { ...done.config, flow_factors: done.flowFactors }
    : done.config;
}

/** Runs `job` ({config, recording, until?}) on the pool for `state`, which is queued, then
 * running (with its progress), then ready (`ready(message)` reads the worker's message) or
 * failed. */
function record(state, job, ready) {
  state.status = "queued";
  pool.submit({
    group: run.details,
    dataset: run.dataset,
    ...job,
    onStart: () => {
      state.status = "running";
      showProgress(state);
    },
    onProgress: ({ progress }) => {
      state.progress = progress;
      showProgress(state);
    },
    onDone: (message) => {
      ready(message);
      render();
    },
    onError: (error) => {
      state.status = "failed";
      state.error = error;
      render();
    },
  });
}

/** Records replication `index`'s violations and tool group days, unless done or under way. */
function replay(index) {
  const replays = run.details.replays;
  if (replays.has(index)) return;
  const state = {};
  replays.set(index, state);
  record(state, { config: replayConfig(index), recording: RECORDING }, (message) => {
    if (message.digest !== run.done[index].digest) {
      state.status = "failed";
      state.error = t("details.mismatch");
      return;
    }
    state.status = "ready";
    state.records = message.records;
    // The oldest records but the shown ones go beyond KEEP.
    const ready = [...replays].filter(([, each]) => each.status === "ready");
    for (const [old] of ready.slice(0, Math.max(0, ready.length - KEEP))) {
      if (old !== view.replication) replays.delete(old);
    }
  });
}

/** The events of the window `span` ({from, days, group}) in the shown replication. */
function events(span) {
  const key = `${view.replication}|${span.from}|${span.days}|${span.group}`;
  const windows = run.details.windows;
  if (!windows.has(key)) {
    const until = span.from + span.days * DAY;
    const state = { until };
    windows.set(key, state);
    const recording = {
      events: { from: span.from, until, tool_groups: [run.info.tool_groups[span.group].name] },
    };
    record(state, { config: replayConfig(view.replication), recording, until }, (message) => {
      state.status = "ready";
      state.events = message.records.events;
    });
  }
  return windows.get(key);
}

/** The history of the lot of violation `row` up to the violation, in the shown replication. */
function history(violations, row) {
  const lot = violations.lot[row];
  const key = `${view.replication}|${lot}|${violations.exit[row]}`;
  const traces = run.details.traces;
  if (!traces.has(key)) {
    const until = violations.exit[row] + 1;
    const state = { until };
    traces.set(key, state);
    const recording = { events: { from: violations.release[row], until, lots: [lot] } };
    record(state, { config: replayConfig(view.replication), recording, until }, (message) => {
      state.status = "ready";
      state.events = message.records.events;
    });
  }
  return traces.get(key);
}

/** Shows how far `state`'s recording is, where it shows. */
function showProgress(state) {
  if (state.line?.isConnected) state.line.textContent = progressText(state);
}

/** Where `state`'s replay is: queued, the day it reached of those it runs (to its `until` or the
 * end time, then the drain), or its failure. */
function progressText(state) {
  switch (state.status) {
    case "queued":
      return t("details.queued");
    case "running": {
      const progress = state.progress;
      if (!progress) return t("details.starting");
      const end = state.until ?? progress.horizon;
      if (progress.now > end) return t("progress.drain", { wip: formatNumber(progress.wip, 0) });
      return t("details.replaying", {
        day: formatNumber(Math.floor(progress.now / DAY), 0),
        days: formatNumber(Math.ceil(end / DAY), 0),
      });
    }
    case "failed":
      return t("status.error", { message: state.error });
    default:
      return "";
  }
}

/** A line showing `state`'s progress until it is ready. */
function progressLine(state) {
  const line = el("p", { class: "hint", "aria-live": "polite" }, progressText(state));
  state.line = line;
  return line;
}

// ---- drawing ----

function render() {
  const replays = run.details.replays;
  $("details-replication").replaceChildren(
    select(
      t("details.replication"),
      run.done.map((done, index) => [String(index), t("lane.name", { run: done.config.replication + 1 })]),
      String(view.replication),
      (value) => {
        view = { ...view, replication: Number(value), page: 0, window: null, draft: null, eventPage: 0, trace: null };
        replay(view.replication);
        render();
      },
    ),
  );
  for (const [index, state] of replays) {
    state.line = index === view.replication ? $("details-status") : null;
  }
  const state = replays.get(view.replication);
  $("details-status").textContent = progressText(state);
  if (state.status !== "ready") {
    $("details-body").replaceChildren();
    return;
  }
  const records = state.records;
  const rows = violationRows(records.violations);
  $("details-body").replaceChildren(
    heatmapCard(records.violations),
    violationsCard(records.violations, rows),
    ...(view.trace === null ? [] : [traceCard(records.violations)]),
    toolGroupCard(records.tool_groups),
    eventsCard(records.violations, rows),
  );
}

/** The exit step's tool group of segment `index`, or the first CQT segment's. */
function exitGroup(index) {
  const info = run.info;
  const segment = info.segments[index ?? 0];
  if (!segment) return 0;
  return info.routes[segment.route].steps[segment.exit].tool_group;
}

/** Violations by segment and week (by day up to 120 days); a cell filters the list. */
function heatmapCard(violations) {
  const info = run.info;
  const end = run.done[view.replication].results.end;
  const bin = end <= 120 * DAY ? DAY : 7 * DAY;
  const bins = Math.max(1, Math.ceil(end / bin));
  const counts = new Map();
  violations.segment.forEach((segment, row) => {
    if (!counts.has(segment)) counts.set(segment, new Array(bins).fill(0));
    counts.get(segment)[Math.min(bins - 1, Math.floor(violations.exit[row] / bin))] += 1;
  });
  const total = (values) => values.reduce((sum, value) => sum + value, 0);
  const rows = [...counts].sort((a, b) => total(b[1]) - total(a[1])).slice(0, SEGMENTS);
  const tip = () => t(bin === DAY ? "details.heatmapDays" : "details.heatmapWeeks", { count: SEGMENTS });
  if (rows.length === 0) return card("details.heatmap", tip, el("p", { class: "hint" }, t("details.none")));
  const chart = heatmap({
    label: t("details.heatmap"),
    rows: rows.map(([segment]) => segmentLabel(info, segment)),
    columns: Array.from({ length: bins }, (_, column) =>
      t("time.day", { day: formatNumber((column * bin) / DAY, 0) }),
    ),
    values: rows.map(([, values]) => values),
    title: (row, column) =>
      [
        segmentTitle(info, rows[row][0]),
        t("details.cell", {
          from: formatNumber((column * bin) / DAY, 0),
          to: formatNumber(((column + 1) * bin) / DAY, 0),
          count: formatNumber(rows[row][1][column], 0),
        }),
      ].join("\n"),
    onSelect: (row, column) => {
      const segment = rows[row][0];
      view = { ...view, segment, week: [column * bin, (column + 1) * bin], page: 0, group: exitGroup(segment), draft: null };
      render();
      $("details-violations").scrollIntoView({ block: "start" });
    },
  });
  return card("details.heatmap", tip, chart);
}

/** The rows of the violations that pass the filters, in the chosen order. */
function violationRows(violations) {
  const info = run.info;
  const rows = [];
  for (let row = 0; row < violations.lot.length; row++) {
    if (view.segment !== null && violations.segment[row] !== view.segment) continue;
    if (view.week && (violations.exit[row] < view.week[0] || violations.exit[row] >= view.week[1])) continue;
    if (view.kind && violations.kind[row] !== view.kind) continue;
    if (view.part !== "" && violations.part[row] !== Number(view.part)) continue;
    rows.push(row);
  }
  const excess = (row) =>
    violations.exit[row] - violations.entered[row] - info.segments[violations.segment[row]].limit;
  if (view.sort === "excess") rows.sort((a, b) => excess(b) - excess(a));
  if (view.sort === "late") rows.reverse();
  return rows;
}

/** The violations that pass the filters, a page at a time. */
function violationsCard(violations, rows) {
  const info = run.info;
  const counts = new Map();
  for (const segment of violations.segment) counts.set(segment, (counts.get(segment) ?? 0) + 1);
  const segments = [...counts].sort((a, b) => b[1] - a[1]);
  const filters = el(
    "div",
    { class: "row" },
    select(
      t("details.segment"),
      [
        ["", t("details.allSegments")],
        ...segments.map(([segment, count]) => [
          String(segment),
          `${segmentLabel(info, segment)} (${formatNumber(count, 0)})`,
        ]),
      ],
      view.segment === null ? "" : String(view.segment),
      (value) => {
        const segment = value === "" ? null : Number(value);
        view = { ...view, segment, week: null, page: 0, draft: null };
        if (segment !== null) view.group = exitGroup(segment);
        render();
      },
    ),
    select(
      t("details.kind"),
      [["", t("details.allKinds")], ...[...new Set(violations.kind)].sort().map((kind) => [kind, kindLabel(kind)])],
      view.kind,
      (value) => {
        view = { ...view, kind: value, page: 0 };
        render();
      },
    ),
    select(
      t("details.part"),
      [
        ["", t("details.allParts")],
        ...[...new Set(violations.part)].sort((a, b) => a - b).map((part) => [String(part), info.parts[part].name]),
      ],
      view.part,
      (value) => {
        view = { ...view, part: value, page: 0 };
        render();
      },
    ),
    select(
      t("details.sort"),
      SORTS.map((sort) => [sort, t(`details.sort.${sort}`)]),
      view.sort,
      (value) => {
        view = { ...view, sort: value, page: 0 };
        render();
      },
    ),
  );
  if (view.week) {
    filters.append(
      button(
        t("details.clearDays", {
          from: formatNumber(view.week[0] / DAY, 0),
          to: formatNumber(view.week[1] / DAY, 0),
        }),
        () => {
          view = { ...view, week: null, page: 0, draft: null };
          render();
        },
      ),
    );
  }
  filters.append(button(t("details.download"), () => downloadViolations(violations, rows)));

  const pages = Math.max(1, Math.ceil(rows.length / PAGE));
  view.page = Math.min(view.page, pages - 1);
  const body = el("tbody");
  for (const row of rows.slice(view.page * PAGE, (view.page + 1) * PAGE)) {
    const segment = info.segments[violations.segment[row]];
    const wait = violations.exit[row] - violations.entered[row];
    const lot = button(String(violations.lot[row]), () => {
      view = { ...view, trace: row };
      render();
      $("details-trace").scrollIntoView({ block: "start" });
    });
    lot.className = "link own";
    lot.title = t("details.traceHint");
    body.append(
      el(
        "tr",
        { class: row === view.trace ? "chosen" : "" },
        el("th", { scope: "row" }, lot),
        el("td", { class: "text" }, kindLabel(violations.kind[row])),
        el("td", { class: "text" }, info.parts[violations.part[row]].name),
        el("td", { class: "text" }, segmentLabel(info, violations.segment[row])),
        el("td", {}, at(violations.entered[row])),
        el("td", {}, hours(wait)),
        el("td", {}, hours(wait - segment.limit)),
        el("td", {}, hours(violations.exit[row] - violations.arrived[row])),
      ),
    );
  }
  const result = card(
    "details.violations",
    () => t("details.violationsHint"),
    filters,
    el(
      "div",
      { class: "scroll" },
      table(["lot", "kind", "part", "segment", "entered", "wait", "excess", "exitQueue"], body),
    ),
    pager(view.page, pages, rows.length, (page) => {
      view = { ...view, page };
      render();
    }),
  );
  result.id = "details-violations";
  return result;
}

/** A tool group day by day: its mean queue and its tools' time shares, pointed at together. */
function toolGroupCard(days) {
  const group = view.group;
  const queue = [];
  const shares = { util: [], down: [], pm: [] };
  for (let row = 0; row < days.day.length; row++) {
    if (days.tool_group[row] !== group) continue;
    const busy = days.setup[row] + days.process[row] + days.load[row] + days.unload[row];
    const total = busy + days.down[row] + days.pm[row] + days.idle[row];
    if (total === 0) continue;
    const day = days.day[row];
    queue.push([day, days.queue[row]]);
    shares.util.push([day, (100 * busy) / total]);
    shares.down.push([day, (100 * days.down[row]) / total]);
    shares.pm.push([day, (100 * days.pm[row]) / total]);
  }
  const x = (day) => t("time.day", { day: formatNumber(day, 0) });
  // Both charts point at and zoom to the same days.
  const linked = chartGroup();
  const charts =
    queue.length < 2
      ? [el("p", { class: "hint" }, t("details.none"))]
      : [
          el("p", { class: "label" }, t("details.queue")),
          lineChart({
            series: [{ label: t("details.queue"), kind: "warning", points: queue }],
            x,
            y: (value) => formatNumber(value, 1),
            label: t("details.queue"),
            group: linked,
          }),
          el("p", { class: "label" }, t("details.shares")),
          lineChart({
            series: [
              { label: t("col.util"), kind: "process", points: shares.util },
              { label: t("col.down"), kind: "down", points: shares.down },
              { label: t("col.pm"), kind: "pm", points: shares.pm },
            ],
            x,
            y: (value) => formatNumber(value, 0),
            label: t("details.shares"),
            group: linked,
          }),
        ];
  return card(
    "details.toolGroup",
    () => t("details.toolGroupHint"),
    el(
      "div",
      { class: "row" },
      groupChoice(group, (chosen) => {
        view = { ...view, group: chosen };
        render();
      }),
      el("span", { class: "hint" }, t("details.tools", { tools: run.info.tool_groups[group].tools })),
    ),
    ...charts,
  );
}

/** The events of a window at a tool group: a timeline of its tools and the list. */
function eventsCard(violations, rows) {
  // The window being chosen: the one shown or, at first, the day of the first violation listed at
  // its exit tool group.
  view.draft ??= view.window ? { ...view.window } : {
    from: rows.length > 0 ? Math.floor(violations.exit[rows[0]] / DAY) * DAY : 0,
    days: WINDOWS[0],
    group: view.group,
  };
  const draft = view.draft;
  const from = el("input", { type: "number", min: "0", step: "any", class: "short" });
  from.value = String(draft.from / DAY);
  from.addEventListener("change", () => {
    draft.from = Math.max(0, Math.round(Number(from.value) * DAY));
  });
  const content = [
    el(
      "div",
      { class: "row" },
      el("label", { class: "field" }, el("span", {}, t("details.from")), from),
      el(
        "label",
        { class: "field" },
        el("span", {}, t("details.length")),
        select(
          t("details.length"),
          WINDOWS.map((count) => [String(count), t("details.days", { days: count })]),
          String(draft.days),
          (value) => {
            draft.days = Number(value);
          },
        ),
      ),
      el(
        "label",
        { class: "field" },
        el("span", {}, t("details.toolGroup")),
        groupChoice(draft.group, (chosen) => {
          draft.group = chosen;
        }),
      ),
      button(
        t("details.showEvents"),
        () => {
          view = { ...view, window: { ...draft }, eventPage: 0 };
          render();
        },
        "primary",
      ),
    ),
  ];
  if (view.window) {
    const state = events(view.window);
    if (state.status === "ready") {
      content.push(...eventViews(state.events, view.window));
    } else {
      content.push(progressLine(state));
    }
  }
  const result = card("details.events", () => t("details.eventsHint"), ...content);
  result.id = "details-events";
  return result;
}

/** The events of the window `span`: the tools of its group on a timeline, with the arrivals,
 * and the list. */
function eventViews(events, span) {
  const info = run.info;
  const first = run.details.firstTool[span.group];
  const tool = (index) => t("details.tool", { tool: index - first + 1 });
  const until = span.from + span.days * DAY;
  const lanes = Array.from({ length: info.tool_groups[span.group].tools }, (_, index) => ({
    label: tool(first + index),
    bars: [],
    marks: [],
  }));
  const arrivals = { label: t("details.arrivals"), bars: [], marks: [] };
  // A job, outage or PM lasts from its start event (or the window's start) to its end event (or
  // the window's end) on the same tool; a job is named by its lot.
  const names = { process: (row) => lotText(events, row), down: () => t("col.down"), pm: () => t("col.pm") };
  const open = new Map();
  const close = (key, kind, row) => {
    const opened = open.get(key);
    open.delete(key);
    const start = opened === undefined ? span.from : events.time[opened];
    return { start, end: events.time[row], kind, name: names[kind](opened ?? row) };
  };
  for (let row = 0; row < events.time.length; row++) {
    const key = (kind) => `${kind}|${events.tool[row]}${kind === "process" ? `|${events.lot[row]}` : ""}`;
    const lane = events.tool[row] === null ? null : lanes[events.tool[row] - first];
    switch (events.kind[row]) {
      case "arrive":
        arrivals.marks.push({
          time: events.time[row],
          kind: "load",
          title: `${at(events.time[row])} · ${lotText(events, row)}`,
        });
        break;
      case "start":
        open.set(key("process"), row);
        break;
      case "down":
        open.set(key("down"), row);
        break;
      case "pm_start":
        open.set(key("pm"), row);
        break;
      case "end":
        lane?.bars.push(close(key("process"), "process", row));
        break;
      case "up":
        lane?.bars.push(close(key("down"), "down", row));
        break;
      case "pm_end":
        lane?.bars.push(close(key("pm"), "pm", row));
        break;
    }
  }
  // Still open when the window ends.
  for (const [key, row] of open) {
    const kind = key.split("|")[0];
    lanes[events.tool[row] - first]?.bars.push({
      start: events.time[row],
      end: until,
      kind,
      name: names[kind](row),
      open: true,
    });
  }
  // The lots of a batch start and end together: one bar names them all.
  for (const lane of lanes) {
    const bars = new Map();
    for (const bar of lane.bars) {
      const key = `${bar.start}|${bar.end}|${bar.kind}`;
      if (bars.has(key)) bars.get(key).names.push(bar.name);
      else bars.set(key, { ...bar, names: [bar.name] });
    }
    lane.bars = [...bars.values()].map((bar) => ({
      start: bar.start,
      end: bar.end,
      kind: bar.kind,
      title: [`${at(bar.start)}–${bar.open ? "" : at(bar.end)}`, ...bar.names].join("\n"),
    }));
  }
  const pages = Math.max(1, Math.ceil(events.time.length / PAGE));
  view.eventPage = Math.min(view.eventPage, pages - 1);
  const list = el("tbody");
  const start = view.eventPage * PAGE;
  for (let row = start; row < Math.min(events.time.length, start + PAGE); row++) {
    const part = events.part[row];
    list.append(
      el(
        "tr",
        {},
        el("th", { scope: "row" }, at(events.time[row])),
        el("td", { class: "text" }, t(`event.${events.kind[row]}`)),
        el("td", {}, events.lot[row] === null ? "–" : String(events.lot[row])),
        el("td", { class: "text" }, part === null ? "–" : info.parts[part].name),
        el("td", { class: "text" }, stepName(events, row)),
        el("td", { class: "text" }, events.tool[row] === null ? "–" : tool(events.tool[row])),
      ),
    );
  }
  return [
    timeline({
      lanes: [arrivals, ...lanes],
      from: span.from,
      until,
      time: at,
      kinds: Object.fromEntries(["load", "process", "down", "pm"].map((kind) => [kind, t(`details.legend.${kind}`)])),
      label: t("details.events"),
    }),
    el(
      "div",
      { class: "row" },
      el("span", { class: "hint" }, t("details.eventCount", { count: formatNumber(events.time.length, 0) })),
      button(t("details.download"), () => downloadEvents(events, span)),
    ),
    el(
      "div",
      { class: "scroll" },
      table(["time", "event", "lot", "part", "step", "tool"], list),
    ),
    pager(view.eventPage, pages, events.time.length, (page) => {
      view = { ...view, eventPage: page };
      render();
    }),
  ];
}

/** A violating lot's history: its steps from release to the violation, the segment's steps
 * close up and in a table. */
function traceCard(violations) {
  const info = run.info;
  const row = view.trace;
  const segment = info.segments[violations.segment[row]];
  const lot = violations.lot[row];
  const entered = violations.entered[row];
  const exit = violations.exit[row];
  const content = [
    el(
      "p",
      { class: "hint" },
      t("details.traceOf", {
        lot,
        kind: kindLabel(violations.kind[row]),
        part: info.parts[violations.part[row]].name,
        segment: segmentTitle(info, violations.segment[row]),
        wait: hours(exit - entered),
      }),
    ),
  ];
  const state = history(violations, row);
  const visits = state.status === "ready" ? visitsOf(state.events) : [];
  // The segment's visits: the entrance step (which ends when the segment starts) and on.
  const close = visits.filter((visit) => (visit.ended ?? Infinity) >= entered);
  if (state.status !== "ready") {
    content.push(progressLine(state));
  } else if (close.length === 0) {
    content.push(el("p", { class: "hint" }, t("details.none")));
  } else {
    const bars = (visit) => {
      const name = stepLabel(info, segment.route, visit.step);
      const times = {
        transport: [visit.left, visit.arrived],
        queue: [visit.arrived, visit.started],
        process: [visit.started, visit.ended ?? visit.started],
      };
      return PARTS.map(([part, kind]) => {
        const [start, end] = times[part];
        return {
          start,
          end,
          kind,
          title: `${name}\n${t(`part.${part}`)}: ${hours(end - start)} (${at(start)}–${at(end)})`,
        };
      });
    };
    const kinds = Object.fromEntries(PARTS.map(([part, kind]) => [kind, t(`part.${part}`)]));
    const lines = [
      { time: entered, kind: "muted", label: t("details.entered") },
      { time: entered + segment.limit, kind: "pm", label: t("details.deadline") },
      { time: exit, kind: "text", label: t("details.exit") },
    ];
    // The step after the entrance where the lot queued longest: its tool group's events then.
    const queued = (visit) => visit.started - visit.arrived;
    const longest = close
      .slice(1)
      .reduce((most, visit) => (queued(visit) > queued(most) ? visit : most), close.at(-1));
    const steps = el("tbody");
    for (const visit of close) {
      const ended = visit.ended ?? visit.started;
      steps.append(
        el(
          "tr",
          {},
          el("th", { scope: "row" }, info.routes[segment.route].steps[visit.step].name),
          el("td", { class: "text" }, info.tool_groups[visit.group].name),
          el("td", {}, visit.step === segment.entry ? "–" : hours(visit.arrived - visit.left)),
          el("td", {}, visit.step === segment.entry ? "–" : hours(visit.started - visit.arrived)),
          el("td", {}, visit.step === segment.exit ? "–" : hours(ended - visit.started)),
        ),
      );
    }
    content.push(
      el("p", { class: "label" }, t("details.history", { steps: visits.length })),
      timeline({
        lanes: [{ label: t("details.lot", { lot }), bars: visits.flatMap(bars) }],
        from: violations.release[row],
        until: exit + 1,
        time: at,
        kinds,
        lines,
        label: t("details.history", { steps: visits.length }),
      }),
      el("p", { class: "label" }, t("details.segmentSteps")),
      timeline({
        lanes: close.map((visit) => ({ label: stepLabel(info, segment.route, visit.step), bars: bars(visit) })),
        from: Math.min(...close.map((visit) => visit.left)),
        until: Math.max(exit, entered + segment.limit) + HOUR,
        time: at,
        kinds,
        lines,
        label: t("details.segmentSteps"),
      }),
      el(
        "div",
        { class: "scroll" },
        table(["step", "toolGroup", "transport", "queue", "process"], steps),
      ),
      el(
        "div",
        { class: "row" },
        button(t("details.queueEvents", { group: info.tool_groups[longest.group].name }), () => {
          const from = Math.floor(longest.arrived / DAY) * DAY;
          const days = WINDOWS.find((count) => from + count * DAY > longest.started) ?? WINDOWS.at(-1);
          const span = { from, days, group: longest.group };
          view = { ...view, window: span, draft: { ...span }, eventPage: 0 };
          render();
          $("details-events").scrollIntoView({ block: "start" });
        }),
      ),
    );
  }
  const result = card("details.trace", null, ...content);
  result.id = "details-trace";
  return result;
}

/** A lot's visits to steps from its events: the step and tool group, when it left the step before
 * (or was released), arrived, started and ended (null if not yet). */
function visitsOf(events) {
  const visits = [];
  let left = null;
  let arrived = null;
  for (let row = 0; row < events.time.length; row++) {
    const time = events.time[row];
    switch (events.kind[row]) {
      case "release":
        left = time;
        break;
      case "arrive":
        arrived = time;
        break;
      case "start":
        visits.push({
          step: events.step[row],
          group: events.tool_group[row],
          left: left ?? arrived ?? time,
          arrived: arrived ?? time,
          started: time,
          ended: null,
        });
        break;
      case "end":
        if (visits.length > 0) visits.at(-1).ended = time;
        left = time;
        break;
    }
  }
  return visits;
}

/** The name of event `row`'s step in its part's route, or "–". */
function stepName(events, row) {
  const part = events.part[row];
  const step = events.step[row];
  if (part === null || step === null) return "–";
  return run.info.routes[run.info.parts[part].route].steps[step].name;
}

/** Event `row`'s lot, its part and step. */
function lotText(events, row) {
  const part = events.part[row];
  const lot = t("details.lot", { lot: events.lot[row] });
  return part === null ? lot : `${lot} · ${run.info.parts[part].name} · ${stepName(events, row)}`;
}

/** A select of the tool groups, those of CQT segments first; `onChoose(group)` on a change. */
function groupChoice(group, onChoose) {
  const info = run.info;
  const inSegments = new Set(info.segments.flatMap((segment) => segment.tool_groups));
  const byName = (a, b) => info.tool_groups[a].name.localeCompare(info.tool_groups[b].name);
  const indices = info.tool_groups.map((_, index) => index);
  const choice = el("select", { "aria-label": t("details.toolGroup") });
  for (const [label, members] of [
    [t("details.cqtGroups"), indices.filter((index) => inSegments.has(index))],
    [t("details.otherGroups"), indices.filter((index) => !inSegments.has(index))],
  ]) {
    if (members.length === 0) continue;
    const options = el("optgroup", { label });
    for (const index of members.sort(byName)) {
      options.append(new Option(info.tool_groups[index].name, String(index), false, index === group));
    }
    choice.append(options);
  }
  choice.addEventListener("change", () => onChoose(Number(choice.value)));
  return choice;
}

/** A table of `body` under the heads of `columns` (details.col.*), text columns to the left. */
function table(columns, body) {
  const head = el(
    "tr",
    {},
    ...columns.map((column) =>
      el("th", { scope: "col", class: TEXT_COLUMNS.has(column) ? "text" : "" }, t(`details.col.${column}`)),
    ),
  );
  return el("table", {}, el("thead", {}, head), body);
}

/** Previous and next buttons around "page n of m". */
function pager(page, pages, count, go) {
  const previous = button(t("details.previous"), () => go(page - 1));
  previous.disabled = page === 0;
  const next = button(t("details.next"), () => go(page + 1));
  next.disabled = page >= pages - 1;
  return el(
    "div",
    { class: "row pager" },
    previous,
    el("span", { class: "hint" }, t("details.page", { page: page + 1, pages, count: formatNumber(count, 0) })),
    next,
  );
}

/** The violations listed, as CSV (times in ms, durations in hours). */
function downloadViolations(violations, rows) {
  const info = run.info;
  const lines = ["lot,kind,part,route,entry_step,exit_step,limit_h,release_ms,entered_ms,arrived_ms,exit_ms,wait_h,excess_h"];
  for (const row of rows) {
    const segment = info.segments[violations.segment[row]];
    const steps = info.routes[segment.route].steps;
    const wait = violations.exit[row] - violations.entered[row];
    lines.push(
      [
        violations.lot[row],
        violations.kind[row],
        info.parts[violations.part[row]].name,
        info.routes[segment.route].name,
        steps[segment.entry].name,
        steps[segment.exit].name,
        segment.limit / HOUR,
        violations.release[row],
        violations.entered[row],
        violations.arrived[row],
        violations.exit[row],
        wait / HOUR,
        (wait - segment.limit) / HOUR,
      ].join(","),
    );
  }
  download(`${run.name}-r${view.replication}-violations.csv`, `${lines.join("\n")}\n`, "text/csv");
}

/** The events of the window `span`, as CSV (tool numbers within the group). */
function downloadEvents(events, span) {
  const info = run.info;
  const first = run.details.firstTool[span.group];
  const lines = ["time_ms,event,lot,part,step,tool"];
  for (let row = 0; row < events.time.length; row++) {
    const part = events.part[row];
    lines.push(
      [
        events.time[row],
        events.kind[row],
        events.lot[row] ?? "",
        part === null ? "" : info.parts[part].name,
        part === null || events.step[row] === null ? "" : stepName(events, row),
        events.tool[row] === null ? "" : events.tool[row] - first + 1,
      ].join(","),
    );
  }
  const name = `${run.name}-r${view.replication}-${run.info.tool_groups[span.group].name}-day${span.from / DAY}`;
  download(`${name}-events.csv`, `${lines.join("\n")}\n`, "text/csv");
}

/** A select of `options` [[value, text]] with `value` chosen; `onChange(value)` on a change. */
function select(label, options, value, onChange) {
  const choice = el("select", { "aria-label": label });
  for (const [option, text] of options) choice.append(new Option(text, option, false, option === value));
  choice.addEventListener("change", () => onChange(choice.value));
  return choice;
}

function button(text, onClick, className) {
  const result = el("button", { type: "button" }, text);
  if (className) result.className = className;
  result.addEventListener("click", onClick);
  return result;
}

/** A card of the details: its title with the (i) of `tip()`, if any, then `content`. */
function card(title, tip, ...content) {
  const heading = el("h3", {}, t(title));
  if (tip) heading.append(infoButton(tip));
  return el("section", { class: "card" }, heading, ...content);
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
