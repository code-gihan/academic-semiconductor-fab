// SMT2020 simulator page: the Home view races [P2]'s rules, the Setup view describes scenarios,
// the worker pool runs their replications (at most navigator.hardwareConcurrency at once) and the
// wasm module summarizes them for the Analysis view, whose details show what the first
// scenario's replication 0 recorded and replays of the others, and compares them in the Compare
// view; the Layout view animates the setup's transport on a dataset with an AMHS layout, whose
// runs record the view's window so that its replay is there when they end.
// Everything reacts to events (input, worker messages, hash changes, resizes, animation frames);
// nothing polls.
import init, { csv, daily, summarize } from "./pkg/smt2020.js";
import { loadCharts } from "./charts.js";
import { renderComparison, showComparison } from "./compare.js";
import { RECORDING, chooseSegment, renderDetails, showDetails } from "./details.js";
import { download } from "./files.js";
import { homeView } from "./home.js";
import { LANGUAGES, failureText, formatDuration, initLanguage, language, setLanguage, t } from "./i18n.js";
import { layoutView } from "./layout.js";
import { reveal, slideTo } from "./motion.js";
import * as pool from "./pool.js";
import { lanes, relabelProgress, showProgress, startProgress } from "./progress.js";
import { defaultPeriod, onSegmentChosen, selectedSegment, showResults } from "./results.js";
import { setupView } from "./setup.js";
import { sharedState } from "./share.js";
import { bindTips, hideTip } from "./tooltip.js";
import { go, initViews } from "./views.js";

const $ = (id) => document.getElementById(id);
let active = null;
let finished = null;
let statusText = () => "";
/** Python wheels published next to the page: file URLs. */
let wheels = [];

initLanguage();
bindTips();
$("language").replaceChildren(
  ...Object.entries(LANGUAGES).map(
    ([code, name]) => new Option(name, code, false, code === language()),
  ),
);
// The Layout view hears of setup edits once both exist.
let layout = null;
const setup = setupView({ status: setStatus, run: start, changed: () => layout?.changed() });
layout = layoutView({ setup, finishedReplay, flowFactors: measuredFlowFactors });
const home = homeView({ start, stop: () => stop(() => t("status.cancelled")) });
/** The tab indicator has been placed once: later moves slide. */
let placed = false;
initViews((view) => {
  // A tooltip of the view left behind would stay.
  hideTip();
  placeIndicator();
  home.shown(view);
  layout.shown(view);
  reveal($(`view-${view}`).querySelectorAll(".panel, .card, .step, .hero > *"));
});
// The tabs change size with the window: the indicator follows at once.
new ResizeObserver(() => {
  placed = false;
  placeIndicator();
}).observe($("tabs"));
setStatus(() => t("status.loadingWasm"));
loadWheels();
// The segment chosen in the overview filters the details.
onSegmentChosen(chooseSegment);

$("language").addEventListener("change", (event) => {
  setLanguage(event.target.value);
  // The tabs' words changed width.
  placed = false;
  placeIndicator();
  setup.relabel();
  home.relabel();
  layout.relabel();
  setStatus(statusText);
  showWheels();
  relabelProgress();
  if (finished) {
    showScenarios();
    showResults(shownScenario().view, $("period").value, false);
    renderDetails();
    renderComparison();
  }
});
$("cancel").addEventListener("click", () => stop(() => t("status.cancelled")));
$("period").addEventListener("change", () =>
  showResults(shownScenario().view, $("period").value, true),
);
$("scenario").addEventListener("change", (event) => showScenario(Number(event.target.value)));
$("download-json").addEventListener("click", downloadJson);
$("download-csv").addEventListener("click", () => {
  const view = shownScenario().view;
  download(`${fileName(view)}.csv`, csv(view.summary), "text/csv");
});
// The setup works once the wasm module (dataset info, checks, summaries) is ready; a share link
// then brings its scenarios.
try {
  await init();
  setup.ready();
  home.ready();
  layout.ready();
  await openShareLink();
} catch (error) {
  setStatus(() => t("status.wasmFailed", { message: error.message }));
}

/** Takes the scenarios of a #share= link, then shows the Setup view with them. */
async function openShareLink() {
  let state;
  try {
    state = await sharedState(location.hash);
  } catch (error) {
    setStatus(() => t("share.invalid"));
    return;
  }
  if (!state) return;
  history.replaceState(null, "", `${location.pathname}${location.search}#setup`);
  go("setup");
  try {
    setup.load(state, () => t("share.loaded", { count: state.scenarios.length }));
  } catch (error) {
    setStatus(() => failureText(error));
  }
}

/** Slides the tab indicator under the current view's tab, at once the first time. */
function placeIndicator() {
  const tab = $("tabs").querySelector('a[aria-current="page"]');
  if (!tab) return;
  slideTo($("tab-indicator"), tab, placed);
  placed = true;
}

/**
 * Runs `batch` ({name, dataset: {key, bytes}, info, replications, scenarios: [{name, config,
 * passes, code?, setup}], view?}) on the pool: every replication of every scenario, with the same seeds,
 * so that the scenarios compare pair by pair. The Run view follows it, unless the batch names the
 * view that does (the Home view's race); its results go to the Analysis and Compare views.
 * Returns the run.
 */
function start(batch) {
  const replications = batch.replications;
  const count = batch.scenarios.length * replications;
  const run = {
    name: batch.name,
    view: batch.view ?? null,
    dataset: batch.dataset,
    info: batch.info,
    count,
    replications,
    threads: Math.min(count, pool.capacity()),
    scenarios: batch.scenarios.map((scenario) => ({ ...scenario, done: [] })),
    // On a dataset with a layout the Layout view's window, which the runs record for it.
    window: batch.info.layout ? layout.window() : null,
    lanes: lanes(batch.scenarios.map((scenario) => scenario.name), replications),
    finished: 0,
    /** Lane shown on the clocks of the Run view, and whether a click chose it. */
    follow: null,
    chosen: false,
    started: performance.now(),
    frame: 0,
  };
  active = run;
  setRunning(true);
  // The Analysis view's charts are needed when the run ends.
  loadCharts();
  $("run-empty").hidden = true;
  startProgress(run, (index) => follow(run, index));
  showProgress(run);
  if (!run.view) go("run");
  // Runs of two passes (QTS measuring its flow factors) take about twice as long: they go to the
  // workers first, so that the run ends sooner when there are fewer workers than replications.
  const order = run.scenarios
    .map((_, index) => index)
    .sort((a, b) => run.scenarios[b].passes - run.scenarios[a].passes);
  order.forEach((index) => {
    const scenario = run.scenarios[index];
    for (let replication = 0; replication < replications; replication++) {
      const lane = run.lanes[index * replications + replication];
      pool.submit({
        group: run,
        dataset: run.dataset,
        config: { ...scenario.config, replication },
        code: scenario.code,
        recording: replication === 0 ? recordingOf(index, run.window) : undefined,
        live: true,
        onStart: () => {
          lane.state = "running";
          redraw(run);
        },
        onProgress: (message) => {
          lane.progress = message.progress;
          lane.segments = message.segments;
          refollow(run);
          redraw(run);
        },
        onDone: (message) => {
          lane.state = "done";
          lane.seconds = message.seconds;
          scenario.done.push(message);
          run.finished += 1;
          if (run.finished === run.count) {
            finish(run);
          } else {
            redraw(run);
          }
        },
        onError: (message) => {
          if (run === active) stop(() => t("status.error", { message }));
        },
      });
    }
  });
  setStatus(() => t("status.running"));
  return run;
}

/** Shows the run's progress at the next frame, once however many messages came. */
function redraw(run) {
  run.frame ||= requestAnimationFrame(() => {
    run.frame = 0;
    if (run !== active) return;
    showProgress(run);
    if (run.view === "home") home.update(run);
  });
}

/** Follows a lane with progress on the clocks, chosen by a click. */
function follow(run, index) {
  if (run !== active || !run.lanes[index].segments) return;
  run.follow = index;
  run.chosen = true;
  showProgress(run);
}

/** The clocks keep a chosen lane until it is done, and otherwise follow a running lane in its
 * final pass (a QTS pre-run runs other rules), or any running lane until there is one. */
function refollow(run) {
  const current = run.lanes[run.follow];
  const final = (lane) =>
    lane.state === "running" && lane.segments && lane.progress.pass === lane.progress.passes - 1;
  if (current && current.state !== "done" && (run.chosen || final(current))) return;
  run.chosen = false;
  const index = run.lanes.findIndex(final);
  run.follow = index >= 0 ? index : run.lanes.findIndex((lane) => lane.state === "running" && lane.segments);
}

function finish(run) {
  const seconds = (performance.now() - run.started) / 1000;
  run.seconds = seconds;
  cancelAnimationFrame(run.frame);
  showProgress(run);
  active = null;
  setRunning(false);
  setStatus(() => t("status.done", { time: formatDuration(seconds) }));
  finished = {
    name: run.name,
    dataset: run.dataset,
    info: run.info,
    window: run.window,
    seconds,
    shown: 0,
    scenarios: run.scenarios.map((scenario) => {
      scenario.done.sort((a, b) => a.config.replication - b.config.replication);
      const results = scenario.done.map((replication) => replication.results);
      const summary = summarize(results);
      // What the Analysis view shows of the scenario (its details keep their recordings here).
      const view = {
        name: run.scenarios.length > 1 ? scenario.name : run.name,
        dataset: run.dataset,
        info: run.info,
        count: run.replications,
        threads: run.threads,
        seconds,
        setup: scenario.setup,
        code: scenario.code,
        done: scenario.done,
        summary,
        daily: daily(results),
      };
      return { name: scenario.name, setup: scenario.setup, done: scenario.done, summary, view };
    }),
  };
  $("analysis-empty").hidden = true;
  $("analysis-body").hidden = false;
  showScenarios();
  showScenario(0, true);
  if (finished.scenarios.length > 1) showComparison(finished, true);
  if (run.view === "home") home.finish(run, finished);
  else go(finished.scenarios.length > 1 ? "compare" : "analysis");
}

/** The scenario choice of the Analysis view, when the run had several. */
function showScenarios() {
  const choice = $("scenario");
  choice.hidden = finished.scenarios.length < 2;
  choice.replaceChildren(
    ...finished.scenarios.map(
      (scenario, index) => new Option(scenario.name, String(index), false, index === finished.shown),
    ),
  );
}

/** The Analysis view of scenario `index`. */
function showScenario(index, animated = false) {
  finished.shown = index;
  const view = finished.scenarios[index].view;
  showResults(view, defaultPeriod(view), animated);
  showDetails(view, selectedSegment());
}

function shownScenario() {
  return finished.scenarios[finished.shown];
}

/** What replication 0 of scenario `index` records (results unchanged): the first scenario's, what
 * the details show first (the others are replayed when asked for); every scenario's, the replay
 * of `window` (none without a layout) for the Layout view. */
function recordingOf(index, window) {
  if (index > 0 && !window) return undefined;
  return { ...(index === 0 ? RECORDING : {}), ...(window ? { replay: window } : {}) };
}

/** The finished run's replications that ran `config` (replication included) on the dataset
 * `key`, each with its scenario's strategy code. */
function finishedRuns(key, config) {
  if (finished?.dataset.key !== key) return [];
  const wanted = canonical(config);
  return finished.scenarios.flatMap((scenario) =>
    scenario.done
      .filter((done) => canonical(done.config) === wanted)
      .map((done) => ({ done, code: scenario.view.code ?? null })),
  );
}

/** The QTS flow factors the finished run measured with `config` on the dataset `key`, if it ran
 * that: a run of it given them needs no pre-run (which runs without strategy code) and runs the
 * same. */
function measuredFlowFactors(key, config) {
  return finishedRuns(key, config).find(({ done }) => done.flowFactors)?.done.flowFactors ?? null;
}

/** The replay of `window` the finished run recorded with `config` and strategy `code` on the
 * dataset `key`, if it did. */
function finishedReplay(key, config, code, window) {
  if (canonical(finished?.window) !== canonical(window)) return null;
  const same = finishedRuns(key, config).find((run) => run.code === (code ?? null) && run.done.replay);
  return same?.done.replay ?? null;
}

/** JSON of `value` with every object's keys in order: equal for equal configurations. */
function canonical(value) {
  return JSON.stringify(value, (_, each) =>
    each && typeof each === "object" && !Array.isArray(each)
      ? Object.fromEntries(Object.entries(each).sort(([a], [b]) => (a < b ? -1 : 1)))
      : each,
  );
}

/** Ends the active run, if any, by stopping its jobs, and shows `render()`. */
function stop(render) {
  const run = active;
  if (run) {
    pool.cancel(run);
    cancelAnimationFrame(run.frame);
    run.seconds = (performance.now() - run.started) / 1000;
    active = null;
  }
  setRunning(false);
  setStatus(render);
  if (run) home.stopped(run, render);
}

/** While running, the setup and the race are locked and the Run tab marked. */
function setRunning(running) {
  setup.lock(running);
  home.lock(running);
  $("cancel").disabled = !running;
  $("run-badge").hidden = !running;
}

/** Shows the status text `render()` returns in the Setup and Run views; it is rendered again
 * when the language changes. */
function setStatus(render) {
  statusText = render;
  $("status").textContent = render();
  $("run-status").textContent = render();
}

/** Reads the wheel index the deployment writes (python/index.html, pip's --find-links page);
 * a copy of the page without it offers none. */
async function loadWheels() {
  try {
    const response = await fetch("python/");
    if (response.ok) {
      const index = new DOMParser().parseFromString(await response.text(), "text/html");
      wheels = [...index.querySelectorAll('a[href$=".whl"]')].map(
        (link) => new URL(link.getAttribute("href"), response.url).href,
      );
    }
  } catch {
    // No index reachable: no wheels.
  }
  showWheels();
}

/** The pip command and the wheels by platform. */
function showWheels() {
  const index = new URL("python/", location.href).href;
  $("python-install").textContent = `pip install smt2020 --no-index --find-links ${index}`;
  $("wheels-note").textContent = t(wheels.length > 0 ? "python.downloads" : "python.none");
  $("wheels").replaceChildren(
    ...wheels.map((url) => {
      const file = url.slice(url.lastIndexOf("/") + 1);
      const platform = file.includes("win_amd64")
        ? "python.windows"
        : file.includes("macosx")
          ? "python.macos"
          : "python.linux";
      const link = Object.assign(document.createElement("a"), { href: url });
      link.textContent = t(platform);
      const name = Object.assign(document.createElement("span"), { className: "hint" });
      name.textContent = ` ${file}`;
      const item = document.createElement("li");
      item.append(link, name);
      return item;
    }),
  );
}

/** A file name for a scenario's downloads: the dataset, and the scenario if there are several. */
function fileName(view) {
  return finished.scenarios.length > 1 ? `${finished.name}-${view.name}` : finished.name;
}

/** The shown scenario's results in the JSON form of the command line's --json output. */
function downloadJson() {
  const view = shownScenario().view;
  const output = {
    data: finished.name,
    threads: view.threads,
    replications: view.done.map(({ config, digest, seconds, memoryBytes, results }) => ({
      config,
      digest,
      seconds,
      memory_bytes: memoryBytes,
      results,
    })),
    summary: view.summary,
  };
  download(`${fileName(view)}.json`, JSON.stringify(output), "application/json");
}
