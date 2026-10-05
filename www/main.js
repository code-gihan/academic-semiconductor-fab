// SMT2020 simulator page: the Setup view describes a scenario, Web Workers run its replications
// (one per worker, at most navigator.hardwareConcurrency at once) and the wasm module summarizes
// them for the Results view. Everything reacts to events (input, worker messages, hash changes);
// nothing polls.
import init, { csv, summarize } from "./pkg/fab_wasm.js";
import { LANGUAGES, formatDuration, initLanguage, language, setLanguage, t } from "./i18n.js";
import { reveal } from "./motion.js";
import { lanes, showProgress, startProgress } from "./progress.js";
import { defaultPeriod, showResults } from "./results.js";
import { setupView } from "./setup.js";
import { go, initViews } from "./views.js";

const $ = (id) => document.getElementById(id);
const threads = navigator.hardwareConcurrency || 4;
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

$("language").addEventListener("change", (event) => {
  setLanguage(event.target.value);
  setup.relabel();
  setStatus(statusText);
  showWheels();
  if (active) showProgress(active);
  if (finished) showResults(finished, $("period").value, false);
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

/** Runs the replications of `scenario` ({name, bytes, config, replications, setup}). */
function start(scenario) {
  const count = scenario.replications;
  const run = {
    name: scenario.name,
    count,
    threads: Math.min(count, threads),
    setup: scenario.setup,
    next: 0,
    done: [],
    workers: [],
    lanes: lanes(count),
    /** Replication shown in the live fab map. */
    follow: null,
    started: performance.now(),
    frame: 0,
  };
  active = run;
  setRunning(true);
  $("run-empty").hidden = true;
  startProgress(run, (replication) => follow(run, replication));
  showProgress(run);
  go("run");
  for (let index = 0; index < run.threads; index++) {
    const worker = new Worker(new URL("worker.js", import.meta.url), { type: "module" });
    worker.onmessage = ({ data: message }) => receive(run, worker, scenario.config, message);
    worker.onerror = (event) => {
      if (run !== active) return;
      stop(() => t("error.worker", { message: event.message || event.type }));
    };
    const copy = scenario.bytes.slice(0);
    worker.postMessage({ type: "load", bytes: copy }, [copy]);
    run.workers.push(worker);
  }
  setStatus(() => t("status.running"));
}

function receive(run, worker, base, message) {
  if (run !== active) return;
  switch (message.type) {
    case "loaded":
      assign(run, worker, base);
      break;
    case "progress": {
      const lane = run.lanes[message.replication];
      lane.progress = message.progress;
      lane.toolGroups = message.toolGroups;
      // The fab map follows the first running replication with progress until it is done.
      if (run.follow === null || run.lanes[run.follow].state === "done") {
        run.follow = run.lanes.findIndex((each) => each.state === "running" && each.toolGroups);
      }
      redraw(run);
      break;
    }
    case "done": {
      const lane = run.lanes[message.config.replication];
      lane.state = "done";
      lane.seconds = message.seconds;
      run.done.push(message);
      assign(run, worker, base);
      if (run.done.length === run.count) {
        finish(run);
      } else {
        redraw(run);
      }
      break;
    }
    case "error":
      stop(() => t("status.error", { message: message.message }));
      break;
  }
}

/** Gives the worker the next replication, or ends it. */
function assign(run, worker, base) {
  if (run.next < run.count) {
    const replication = run.next++;
    run.lanes[replication].state = "running";
    worker.postMessage({ type: "run", config: { ...base, replication } });
    redraw(run);
  } else {
    worker.terminate();
  }
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
  active = null;
  setRunning(false);
  setStatus(() => t("status.done", { time: formatDuration(seconds) }));
  run.done.sort((a, b) => a.config.replication - b.config.replication);
  const summary = summarize(run.done.map((replication) => replication.results));
  finished = { ...run, seconds, summary };
  $("results-empty").hidden = true;
  $("results-body").hidden = false;
  showResults(finished, defaultPeriod(finished), true);
  go("results");
}

/** Ends the active run, if any, by terminating its workers, and shows `render()`. */
function stop(render) {
  if (active) {
    for (const worker of active.workers) worker.terminate();
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

function download(name, text, type) {
  const url = URL.createObjectURL(new Blob([text], { type }));
  Object.assign(document.createElement("a"), { href: url, download: name }).click();
  // Some browsers read the file after click() returns: release it later, not at once.
  setTimeout(() => URL.revokeObjectURL(url), 60_000);
}
