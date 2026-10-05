// The run as it goes: overall progress, a lane per replication, and the live fab of the followed
// replication (its tool groups by area). Rendered from the run's state, at most once per frame
// and again in a new language.
import { withTooltip } from "./charts.js";
import { formatDuration, formatNumber, t } from "./i18n.js";
import { reveal } from "./motion.js";

const DAY = 86_400_000;
const $ = (id) => document.getElementById(id);

/** A lane of the run per replication: waiting, then running with its latest progress and tool
 * groups, then done. */
export function lanes(count) {
  return Array.from({ length: count }, () => ({ state: "waiting" }));
}

/** Empties the panel for `run`; a click on a lane follows that replication in the fab map. */
export function startProgress(run, follow) {
  $("lanes").replaceChildren(
    ...run.lanes.map((_, replication) => {
      const button = element("button", "lane");
      button.type = "button";
      button.addEventListener("click", () => follow(replication));
      const bar = element("span", "lane-bar");
      bar.append(element("span", "lane-fill"));
      button.append(element("span", "lane-name"), element("span", "lane-state"), bar);
      const item = document.createElement("li");
      item.append(button);
      return item;
    }),
  );
  $("fab-map").replaceChildren();
  $("live").hidden = true;
  reveal($("lanes").children);
}

/** Share of the run's work done. */
export function fraction(run) {
  return run.lanes.reduce((sum, lane) => sum + laneFraction(lane), 0) / run.lanes.length;
}

export function showProgress(run) {
  const done = fraction(run);
  $("progress-fill").style.transform = `scaleX(${done})`;
  const finished = run.lanes.filter((lane) => lane.state === "done").length;
  const elapsed = (performance.now() - run.started) / 1000;
  const parts = [
    t("progress.done", { done: finished, count: run.lanes.length }),
    t("progress.elapsed", { time: formatDuration(elapsed) }),
  ];
  // Estimated from the pace so far, once there is some, until the horizon (the drain is not).
  if (done >= 0.02 && done < 1) {
    parts.push(t("progress.remaining", { time: formatDuration((elapsed * (1 - done)) / done) }));
  }
  $("progress-summary").textContent = parts.join(" · ");
  run.lanes.forEach((lane, replication) => {
    const button = $("lanes").children[replication].firstElementChild;
    button.classList.toggle("followed", replication === run.follow);
    button.classList.toggle("done", lane.state === "done");
    button.children[0].textContent = t("lane.name", { replication });
    button.children[1].textContent = laneText(lane);
    button.children[2].firstElementChild.style.transform = `scaleX(${laneFraction(lane)})`;
  });
  showLive(run);
}

function laneFraction(lane) {
  if (lane.state === "done") return 1;
  const progress = lane.progress;
  if (!progress) return 0;
  return (progress.pass + Math.min(progress.now / progress.horizon, 1)) / progress.passes;
}

function laneText(lane) {
  if (lane.state === "waiting") return t("lane.waiting");
  if (lane.state === "done") return t("lane.done", { time: formatDuration(lane.seconds) });
  return lane.progress ? phase(lane.progress) : t("lane.starting");
}

/** Where a replication is: its simulated day or the drain, the QTS pass and its CQT violations
 * so far. */
function phase(progress) {
  const parts = [
    progress.now > progress.horizon
      ? t("progress.drain", { wip: formatNumber(progress.wip, 0) })
      : t("progress.day", {
          day: formatNumber(Math.floor(progress.now / DAY), 0),
          days: formatNumber(progress.horizon / DAY, 0),
        }),
  ];
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

/** The followed replication's fab: a tile per tool group, filled by its busy tools, with a bar
 * for tools down or in PM and its queue. */
function showLive(run) {
  const lane = run.lanes[run.follow];
  const groups = lane?.toolGroups;
  if (!groups) return;
  $("live").hidden = false;
  $("live-caption").textContent = t("live.caption", {
    replication: run.follow,
    phase: laneText(lane),
  });
  const progress = lane.progress;
  const stats = [
    ["live.released", formatNumber(progress.released, 0)],
    ["live.completed", formatNumber(progress.completed, 0)],
    ["live.wip", formatNumber(progress.wip, 0)],
    [
      "live.cqt",
      progress.cqt_completed > 0 ? formatNumber(violationShare(progress), 1) : "–",
    ],
  ];
  $("live-stats").replaceChildren(
    ...stats.map(([label, value]) => {
      const stat = element("div", "stat");
      stat.append(element("span", "hint", t(label)), element("strong", "", value));
      return stat;
    }),
  );
  const map = $("fab-map");
  if (map.childElementCount === 0) buildMap(map, groups, run);
  const tiles = map.querySelectorAll(".tile");
  groups.forEach((group, index) => {
    const busy = group.setup + group.process + group.load + group.unload;
    const tile = tiles[index];
    tile.style.setProperty("--busy", group.tools > 0 ? busy / group.tools : 0);
    tile.style.setProperty("--out", group.tools > 0 ? (group.down + group.pm) / group.tools : 0);
    tile.lastElementChild.textContent = group.queue > 0 ? formatNumber(group.queue, 0) : "";
  });
}

/** Tiles in dataset order, grouped by area; their tooltips read the followed lane's latest
 * state. */
function buildMap(map, groups, run) {
  const areas = new Map();
  groups.forEach((group, index) => {
    if (!areas.has(group.area)) areas.set(group.area, []);
    areas.get(group.area).push(index);
  });
  for (const [area, indices] of areas) {
    const tiles = element("div", "tiles");
    for (const index of indices) {
      const tile = element("span", "tile");
      tile.append(
        element("span", "tile-busy"),
        element("span", "tile-out"),
        element("span", "tile-queue"),
      );
      withTooltip(tile, () => groupText(run.lanes[run.follow].toolGroups[index]));
      tiles.append(tile);
    }
    const block = element("div", "area");
    block.append(element("span", "area-name", area), tiles);
    map.append(block);
  }
  reveal(map.querySelectorAll(".tile"), 4);
}

function groupText(group) {
  return [
    `${group.name} (${group.area})`,
    t("tip.tools", { tools: group.tools, queue: group.queue }),
    t("tip.busy", {
      setup: group.setup,
      process: group.process,
      load: group.load,
      unload: group.unload,
    }),
    t("tip.idle", { idle: group.idle, down: group.down, pm: group.pm }),
  ].join("\n");
}

function element(tag, className, text) {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text != null) node.textContent = text;
  return node;
}
