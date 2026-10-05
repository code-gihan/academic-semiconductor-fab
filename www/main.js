// SMT2020 simulator page: the Setup view describes scenarios, the worker pool runs their
// replications (at most navigator.hardwareConcurrency at once) and the wasm module summarizes
// them for the Analysis view, whose details show what the first scenario's replication 0 recorded
// and replays of the others, and compares them in the Compare view. Everything reacts to events
// (input, worker messages, hash changes); nothing polls.
import init, { csv, daily, summarize } from "./pkg/fab_wasm.js";
import { loadCharts } from "./charts.js";
import { renderComparison, showComparison } from "./compare.js";
import { RECORDING, chooseSegment, renderDetails, showDetails } from "./details.js";
import { download } from "./files.js";
import { LANGUAGES, formatDuration, initLanguage, language, setLanguage, t } from "./i18n.js";
import { reveal } from "./motion.js";
import * as pool from "./pool.js";
import { lanes, showProgress, startProgress } from "./progress.js";
import { defaultPeriod, onSegmentChosen, selectedSegment, showResults } from "./results.js";
import { failureText, setupView } from "./setup.js";
import { sharedState } from "./share.js";
import { go, initViews } from "./views.js";

const $ = (id) => document.getElementById(id);
let active = null;
let finished = null;
let statusText = () => "";
/** Python wheels published next to the page: file URLs. */
let wheels = [];

initLanguage();
$("language").replaceChildren(
  ...Object.entries(LANGUAGES).map(
    ([code, name]) => new Option(name, code, false, code === language()),
  ),
);
const setup = setupView({ status: setStatus, run: start });
initViews((view) => {
  // A tooltip of the view left behind would stay.
  $("tooltip").hidden = true;
  reveal($(`view-${view}`).querySelectorAll(".panel, .card, .step"));
});
setStatus(() => t("status.loadingWasm"));
loadWheels();
// The segment chosen in the overview filters the details.
onSegmentChosen(chooseSegment);

$("language").addEventListener("change", (event) => {
  setLanguage(event.target.value);
  setup.relabel();
  setStatus(statusText);
  showWheels();
  if (active) showProgress(active);
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
  try {
    setup.load(state, () => t("share.loaded", { count: state.scenarios.length }));
  } catch (error) {
    setStatus(() => failureText(error));
  }
}

/**
 * Runs `batch` ({name, dataset: {key, bytes}, info, replications, scenarios: [{name, config,
 * setup}]}) on the pool: every replication of every scenario, with the same seeds, so that the
 * scenarios compare pair by pair.
 */
function start(batch) {
  const replications = batch.replications;
  const count = batch.scenarios.length * replications;
  const run = {
    name: batch.name,
    dataset: batch.dataset,
    info: batch.info,
    count,
    replications,
    threads: Math.min(count, pool.capacity()),
    scenarios: batch.scenarios.map((scenario) => ({ ...scenario, done: [] })),
    lanes: lanes(batch.scenarios.map((scenario) => scenario.name), replications),
    finished: 0,
    /** Lane shown in the live fab map. */
    follow: null,
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
  go("run");
  run.scenarios.forEach((scenario, index) => {
    for (let replication = 0; replication < replications; replication++) {
      const lane = run.lanes[index * replications + replication];
      pool.submit({
        group: run,
        dataset: run.dataset,
        config: { ...scenario.config, replication },
        // The first scenario's replication 0 records what the details show first (recording
        // leaves results unchanged); the others are replayed when asked for.
        recording: index === 0 && replication === 0 ? RECORDING : undefined,
        live: true,
        onStart: () => {
          lane.state = "running";
          redraw(run);
        },
        onProgress: (message) => {
          lane.progress = message.progress;
          lane.toolGroups = message.toolGroups;
          // The fab map follows the first running lane with progress until it is done.
          if (run.follow === null || run.lanes[run.follow].state === "done") {
            run.follow = run.lanes.findIndex((each) => each.state === "running" && each.toolGroups);
          }
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
}

/** Shows the run's progress at the next frame, once however many messages came. */
function redraw(run) {
  run.frame ||= requestAnimationFrame(() => {
    run.frame = 0;
    if (run === active) showProgress(run);
  });
}

/** Follows a lane with progress in the fab map. */
function follow(run, index) {
  if (run !== active || !run.lanes[index].toolGroups) return;
  run.follow = index;
  showProgress(run);
}

function finish(run) {
  const seconds = (performance.now() - run.started) / 1000;
  cancelAnimationFrame(run.frame);
  active = null;
  setRunning(false);
  setStatus(() => t("status.done", { time: formatDuration(seconds) }));
  finished = {
    name: run.name,
    dataset: run.dataset,
    info: run.info,
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
  if (finished.scenarios.length > 1) {
    showComparison(finished, true);
    go("compare");
  } else {
    go("analysis");
  }
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

/** Ends the active run, if any, by stopping its jobs, and shows `render()`. */
function stop(render) {
  if (active) {
    pool.cancel(active);
    cancelAnimationFrame(active.frame);
    active = null;
  }
  setRunning(false);
  setStatus(render);
}

/** While running, the setup is locked and the Run tab marked. */
function setRunning(running) {
  setup.lock(running);
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
