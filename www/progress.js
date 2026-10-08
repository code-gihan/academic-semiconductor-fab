// The run as it goes: overall progress, a lane per run, and queue time live: each strategy's CQT
// violations so far, and the queue-time clocks of the followed run, its lots in CQT segments
// placed by how much of the limit they have used. Rendered from the run's state, at most once per
// frame, and again in a new language.
import { liveClocks, liveLines } from "./charts.js";
import { coach } from "./coach.js";
import { formatDuration, formatNumber, t } from "./i18n.js";
import { DAY, hours, kindLabel, routeName } from "./labels.js";
import { reveal } from "./motion.js";

const $ = (id) => document.getElementById(id);
/** CQT completions a strategy needs before its share so far shows (its first hours are noise). */
const FIRST_SHARE = 1000;
/** Constraints on clocks, and how far a clock runs past the limit (times the limit). */
const CLOCKS = 10;
const PAST = 2;
/** Colour tokens of the strategies' lines: of several, the first (the comparisons' default
 * baseline) in grey. */
const RULES = ["rule-1", "rule-2", "rule-3", "rule-4", "rule-5"];
/** A lot's state on its clock (its colour token) and the words for it. */
const STATES = { "on-track": "clocks.onTrack", "at-risk": "clocks.atRisk", "over-limit": "clocks.overLimit" };

/** The latest run's live charts: {run, constraints, points (a line per scenario), lines,
 * clocks}. */
let live = null;

/** A lane of the run per scenario (named by `names`) and replication: waiting, then running with
 * its latest progress and CQT segments, then done. */
export function lanes(names, replications) {
  return names.flatMap((name) =>
    Array.from({ length: replications }, (_, replication) => ({
      state: "waiting",
      scenario: names.length > 1 ? name : null,
      replication,
    })),
  );
}

/** Empties the panel for `run`; a click on a lane follows it on the clocks. */
export function startProgress(run, follow) {
  $("lanes").replaceChildren(
    ...run.lanes.map((_, index) => {
      const button = element("button", "lane");
      button.type = "button";
      button.addEventListener("click", () => follow(index));
      const bar = element("span", "lane-bar");
      bar.append(element("span", "lane-fill"));
      button.append(element("span", "lane-name"), element("span", "lane-state"), bar);
      const item = document.createElement("li");
      item.append(button);
      return item;
    }),
  );
  live = { run, constraints: constraints(run.info), points: run.scenarios.map(() => []) };
  drawLive();
  $("live").hidden = true;
  reveal($("lanes").children);
}

/** The latest run's panel in the page's language. */
export function relabelProgress() {
  if (!live) return;
  drawLive();
  showProgress(live.run);
}

/** Share of the run's work done. */
export function fraction(run) {
  return run.lanes.reduce((sum, lane) => sum + laneFraction(lane), 0) / run.lanes.length;
}

export function showProgress(run) {
  const done = fraction(run);
  $("progress-fill").style.transform = `scaleX(${done})`;
  const finished = run.lanes.filter((lane) => lane.state === "done").length;
  // An ended run keeps its time.
  const elapsed = run.seconds ?? (performance.now() - run.started) / 1000;
  const parts = [
    t("progress.done", { done: finished, count: run.lanes.length }),
    t("progress.elapsed", { time: formatDuration(elapsed) }),
  ];
  // Estimated from the pace so far, once there is some, until the horizon (the drain is not).
  if (run.seconds == null && done >= 0.02 && done < 1) {
    parts.push(t("progress.remaining", { time: formatDuration((elapsed * (1 - done)) / done) }));
  }
  $("progress-summary").textContent = parts.join(" · ");
  run.lanes.forEach((lane, index) => {
    const button = $("lanes").children[index].firstElementChild;
    button.classList.toggle("followed", index === run.follow);
    button.classList.toggle("done", lane.state === "done");
    button.children[0].textContent = laneName(lane);
    button.children[1].textContent = laneText(lane);
    button.children[2].firstElementChild.style.transform = `scaleX(${laneFraction(lane)})`;
  });
  if (live?.run === run) showLive(run);
}

/** A lane's run, counted from 1, after its scenario if the run has several. */
function laneName(lane) {
  const name = t("lane.name", { run: lane.replication + 1 });
  return lane.scenario ? `${lane.scenario} · ${name}` : name;
}

/** Share of a lane's work done: its passes, each up to the horizon. */
export function laneFraction(lane) {
  if (lane.state === "done") return 1;
  const progress = lane.progress;
  if (!progress) return 0;
  return (progress.pass + Math.min(progress.now / progress.horizon, 1)) / progress.passes;
}

/** CQT completions over the limit so far, over `lanes` in their final pass: {day (their mean
 * simulated day up to the horizon), value (%)}, once there are enough. */
export function shareSoFar(lanes) {
  const final = lanes.filter((lane) => lane.progress && lane.progress.pass === lane.progress.passes - 1);
  const completed = final.reduce((sum, lane) => sum + lane.progress.cqt_completed, 0);
  if (completed < FIRST_SHARE) return null;
  const violated = final.reduce((sum, lane) => sum + lane.progress.cqt_violated, 0);
  const day = final.reduce((sum, lane) => sum + Math.min(lane.progress.now, lane.progress.horizon), 0) / final.length / DAY;
  return { day, value: (100 * violated) / completed };
}

/** Grows `line` by `share` (shareSoFar), at most a point per simulated day. */
export function grow(line, share) {
  if (share && (line.length === 0 || share.day >= line.at(-1)[0] + 1)) {
    line.push([share.day, share.value]);
  }
}

function laneText(lane) {
  if (lane.state === "waiting") return t("lane.waiting");
  if (lane.state === "done") return t("lane.done", { time: formatDuration(lane.seconds) });
  return lane.progress ? phase(lane.progress) : t("lane.starting");
}

/** A run's simulated day of the horizon's, or the drain after it. */
export function dayText(progress) {
  return progress.now > progress.horizon
    ? t("progress.drain", { wip: formatNumber(progress.wip, 0) })
    : t("progress.day", {
        day: formatNumber(Math.floor(progress.now / DAY), 0),
        days: formatNumber(progress.horizon / DAY, 0),
      });
}

/** Where a replication is: its simulated day or the drain, the QTS pass and its CQT violations
 * so far. */
function phase(progress) {
  const parts = [dayText(progress)];
  if (progress.passes > 1) {
    parts.push(t(progress.pass === 0 ? "progress.preRun" : "progress.mainRun"));
  }
  if (progress.cqt_completed > 0) {
    parts.push(t("progress.cqt", { share: formatNumber(violationShare(progress), 1) }));
  }
  return parts.join(" · ");
}

/** CQT segment completions over the limit so far (%). */
function violationShare(progress) {
  return (100 * progress.cqt_violated) / progress.cqt_completed;
}

/** The live charts of the latest run, with the lines so far, and the counts of the clocks. */
function drawLive() {
  const { run } = live;
  const config = run.scenarios[0].config;
  const days = (ms) => ms / DAY;
  // The dataset's warm-up when the settings give none: up to its first reported period.
  const warmUp = config.warm_up ?? run.info.periods[1]?.start ?? 0;
  const several = run.scenarios.length > 1;
  live.lines = liveLines({
    series: run.scenarios.map((scenario, index) => ({
      label: several ? scenario.name : t("live.allRuns", { runs: run.replications }),
      kind: several && index === 0 ? "rule-base" : RULES[(index - Number(several)) % RULES.length],
    })),
    max: days(config.horizon),
    shade: { to: days(warmUp), label: t("chart.warmUp") },
    x: (day) => t("time.day", { day: formatNumber(day, 0) }),
    y: (share) => `${formatNumber(share, 1)}%`,
    tick: (share) => `${formatNumber(share, 0)}%`,
    label: t("live.lines"),
    height: 220,
  });
  live.lines.set(live.points);
  $("live-lines").replaceChildren(live.lines.box);
  const info = run.info;
  const group = (index) => info.tool_groups[index].name;
  live.clocks = liveClocks({
    count: CLOCKS,
    max: PAST,
    x: (share) => (share === 0 ? "0" : share === 1 ? t("clocks.limit") : t("clocks.times", { times: share })),
    column: t("clocks.soFarShort"),
    rowTitle: ({ constraint, lots, completed, violated, text }) => {
      const { "on-track": onTrack, "at-risk": atRisk, "over-limit": over } = counts(lots);
      return [
        t("clocks.rowTitle", {
          from: group(constraint.from),
          to: group(constraint.to),
          limit: hours(constraint.limit),
          products: constraint.products.join(", "),
        }),
        t("clocks.rowNow", { lots: lots.length, onTrack, atRisk, over }),
        t("clocks.rowSoFar", {
          violated: formatNumber(violated, 0),
          completed: formatNumber(completed, 0),
          share: text,
        }),
      ].join("\n");
    },
    lotTitle: ({ constraint }, lot) => {
      const route = info.segments[lot.segment].route;
      const step = info.routes[route].steps[lot.step];
      const time =
        lot.token === "over-limit"
          ? t("clocks.over", { over: hours(lot.used - constraint.limit) })
          : t(lot.slack < 0 ? "clocks.short" : "clocks.slack", {
              used: hours(lot.used),
              limit: hours(constraint.limit),
              slack: hours(Math.abs(lot.slack)),
            });
      return [
        t("clocks.lot", { id: lot.id, kind: kindLabel(lot.kind), product: routeName(info, route) }),
        t("clocks.where", { step: step.name, group: group(step.tool_group), state: t(`clocks.state.${lot.state}`) }),
        time,
      ].join("\n");
    },
    label: t("clocks.heading"),
  });
  $("clocks").replaceChildren(live.clocks.box);
  $("clock-stats").replaceChildren(
    ...[...Object.entries(STATES), [null, "clocks.soFar"]].map(([token, key]) => {
      const label = element("span", "hint");
      if (token) {
        const swatch = element("span", "swatch");
        swatch.style.setProperty("--swatch", `var(--${token})`);
        label.append(swatch);
      }
      label.append(t(key));
      const stat = element("div", "stat");
      stat.append(label, element("strong", "", "–"));
      return stat;
    }),
  );
}

/** Each strategy's line so far, and the followed run's clocks: the constraints with the most
 * violations so far (then the most lots now), the others counted below. */
function showLive(run) {
  run.scenarios.forEach((_, index) => {
    grow(live.points[index], shareSoFar(run.lanes.slice(index * run.replications, (index + 1) * run.replications)));
  });
  live.lines.set(live.points);
  const lane = run.lanes[run.follow];
  if (!lane?.segments) return;
  $("live").hidden = false;
  $("live-caption").textContent = t("live.caption", { lane: laneName(lane), phase: laneText(lane) });
  const { info } = run;
  const now = lane.progress.now;
  const rows = live.constraints.map((constraint) => {
    let completed = 0;
    let violated = 0;
    const lots = constraint.segments.flatMap((index) => {
      const segment = lane.segments[index];
      completed += segment.cqt.completed;
      violated += segment.cqt.violated;
      return segment.lots.map((lot) => {
        const used = now - lot.entered;
        // Over once the wait passes the limit; at risk while the work left exceeds the time left.
        const token = used > constraint.limit ? "over-limit" : lot.slack < 0 ? "at-risk" : "on-track";
        return { ...lot, segment: index, used, at: used / constraint.limit, token };
      });
    });
    return { constraint, lots, completed, violated };
  });
  const all = counts(rows.flatMap((row) => row.lots));
  const stats = $("clock-stats").children;
  Object.keys(STATES).forEach((token, index) => {
    stats[index].lastElementChild.textContent = formatNumber(all[token], 0);
  });
  stats[3].lastElementChild.textContent =
    lane.progress.cqt_completed > 0 ? `${formatNumber(violationShare(lane.progress), 1)}%` : "–";
  const ranked = rows
    .filter((row) => row.lots.length > 0 || row.violated > 0)
    .sort((a, b) => b.violated - a.violated || b.lots.length - a.lots.length);
  const group = (index) => info.tool_groups[index].name;
  live.clocks.set(
    ranked.slice(0, CLOCKS).map((row) => ({
      ...row,
      label: t("clocks.row", {
        limit: hours(row.constraint.limit),
        from: group(row.constraint.from),
        to: group(row.constraint.to),
      }),
      text: row.completed > 0 ? `${formatNumber((100 * row.violated) / row.completed, 1)}%` : "–",
    })),
  );
  const rest = ranked.slice(CLOCKS).filter((row) => row.lots.length > 0);
  const others = rest.flatMap((row) => row.lots);
  $("clocks-more").textContent =
    rest.length > 0
      ? t("clocks.more", {
          constraints: formatNumber(rest.length, 0),
          lots: formatNumber(others.length, 0),
          over: formatNumber(counts(others)["over-limit"], 0),
        })
      : "";
  coach($("clocks"), "clocks");
}

/** The dataset's CQT segments by constraint: the same limit from the end of a step at one tool
 * group to the start of a step at another, for any product; in order of their first segment. */
function constraints(info) {
  const byKey = new Map();
  info.segments.forEach((segment, index) => {
    const steps = info.routes[segment.route].steps;
    const [from, to] = [steps[segment.entry].tool_group, steps[segment.exit].tool_group];
    const key = `${from}-${to}-${segment.limit}`;
    if (!byKey.has(key)) byKey.set(key, { from, to, limit: segment.limit, segments: [], products: [] });
    const constraint = byKey.get(key);
    constraint.segments.push(index);
    const product = routeName(info, segment.route);
    if (!constraint.products.includes(product)) constraint.products.push(product);
  });
  return [...byKey.values()];
}

/** Lots by their state on the clock. */
function counts(lots) {
  const count = Object.fromEntries(Object.keys(STATES).map((token) => [token, 0]));
  for (const lot of lots) count[lot.token] += 1;
  return count;
}

function element(tag, className, text) {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text != null) node.textContent = text;
  return node;
}
