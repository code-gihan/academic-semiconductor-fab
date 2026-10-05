// SMT2020 simulator page: the Setup view describes a scenario, the worker pool runs its
// replications (at most navigator.hardwareConcurrency at once) and the wasm module summarizes
// them for the Analysis view, whose details show what replication 0 recorded and replays of the
// others. Everything reacts to events (input, worker messages, hash changes); nothing polls.
import init, { csv, daily, summarize } from "./pkg/fab_wasm.js";
import { loadCharts } from "./charts.js";
import { RECORDING, chooseSegment, renderDetails, showDetails } from "./details.js";
import { download } from "./files.js";
import { LANGUAGES, formatDuration, initLanguage, language, setLanguage, t } from "./i18n.js";
import { reveal } from "./motion.js";
import * as pool from "./pool.js";
import { lanes, showProgress, startProgress } from "./progress.js";
import { defaultPeriod, onSegmentChosen, selectedSegment, showResults } from "./results.js";
import { setupView } from "./setup.js";
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
    showResults(finished, $("period").value, false);
    renderDetails();
  }
});
$("cancel").addEventListener("click", () => stop(() => t("status.cancelled")));
$("period").addEventListener("change", () => showResults(finished, $("period").value, true));
$("download-json").addEventListener("click", downloadJson);
$("download-csv").addEventListener("click", () =>
  download(`${finished.name}.csv`, csv(finished.summary), "text/csv"),
);
// The setup works once the wasm module (dataset info, checks, summaries) is ready.
try {
  await init();
  setup.ready();
} catch (error) {
  setStatus(() => t("status.wasmFailed", { message: error.message }));
}

/** Runs the replications of `scenario` ({name, dataset: {key, bytes}, info, config, replications,
 * setup}) on the pool. */
function start(scenario) {
  const count = scenario.replications;
  const run = {
    name: scenario.name,
    dataset: scenario.dataset,
    info: scenario.info,
    count,
    threads: Math.min(count, pool.capacity()),
    setup: scenario.setup,
    done: [],
    lanes: lanes(count),
    /** Replication shown in the live fab map. */
    follow: null,
    started: performance.now(),
    frame: 0,
  };
  active = run;
  setRunning(true);
  // The Analysis view's charts are needed when the run ends.
  loadCharts();
  $("run-empty").hidden = true;
  startProgress(run, (replication) => follow(run, replication));
  showProgress(run);
  go("run");
  for (let replication = 0; replication < count; replication++) {
    const lane = run.lanes[replication];
    pool.submit({
      group: run,
      dataset: run.dataset,
      config: { ...scenario.config, replication },
      // Replication 0 records what the details show first (recording leaves results unchanged).
      recording: replication === 0 ? RECORDING : undefined,
      live: true,
      onStart: () => {
        lane.state = "running";
        redraw(run);
      },
      onProgress: (message) => {
        lane.progress = message.progress;
        lane.toolGroups = message.toolGroups;
        // The fab map follows the first running replication with progress until it is done.
        if (run.follow === null || run.lanes[run.follow].state === "done") {
          run.follow = run.lanes.findIndex((each) => each.state === "running" && each.toolGroups);
        }
        redraw(run);
      },
      onDone: (message) => {
        lane.state = "done";
        lane.seconds = message.seconds;
        run.done.push(message);
        if (run.done.length === run.count) {
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
  setStatus(() => t("status.running"));
}

/** Shows the run's progress at the next frame, once however many messages came. */
function redraw(run) {
  run.frame ||= requestAnimationFrame(() => {
    run.frame = 0;
    if (run === active) showProgress(run);
  });
}

/** Follows a replication with progress in the fab map. */
function follow(run, replication) {
  if (run !== active || !run.lanes[replication].toolGroups) return;
  run.follow = replication;
  showProgress(run);
}

function finish(run) {
  const seconds = (performance.now() - run.started) / 1000;
  cancelAnimationFrame(run.frame);
  active = null;
  setRunning(false);
  setStatus(() => t("status.done", { time: formatDuration(seconds) }));
  run.done.sort((a, b) => a.config.replication - b.config.replication);
  const results = run.done.map((replication) => replication.results);
  finished = { ...run, seconds, summary: summarize(results), daily: daily(results) };
  $("analysis-empty").hidden = true;
  $("analysis-body").hidden = false;
  showResults(finished, defaultPeriod(finished), true);
  showDetails(finished, selectedSegment());
  go("analysis");
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

/** The results in the JSON form of the command line's --json output. */
function downloadJson() {
  const output = {
    data: finished.name,
    threads: finished.threads,
    replications: finished.done.map(({ config, digest, seconds, memoryBytes, results }) => ({
      config,
      digest,
      seconds,
      memory_bytes: memoryBytes,
      results,
    })),
    summary: finished.summary,
  };
  download(`${finished.name}.json`, JSON.stringify(output), "application/json");
}
