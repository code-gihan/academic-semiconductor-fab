// SMT2020 simulator page: the form describes a run, Web Workers run its replications (one per
// worker, at most navigator.hardwareConcurrency at once) and the wasm module summarizes them.
// Everything reacts to events (input, worker messages); nothing polls.
import init, { csv, summarize } from "./pkg/fab_wasm.js";
import { LANGUAGES, formatDuration, initLanguage, language, setLanguage, t } from "./i18n.js";
import { pulse, reveal, toggle } from "./motion.js";
import { lanes, showProgress, startProgress } from "./progress.js";
import { defaultPeriod, showResults } from "./results.js";

const DAY = 86_400_000;
const HOUR = 3_600_000;

/** [P2] Table 3: stepper limits (front/total); other tool groups 1,000/1,000. */
const STOPPING = {
  high: { LithoTrack_FE_95: [90, 130], LithoTrack_FE_115: [90, 220] },
  medium: { LithoTrack_FE_95: [60, 95], LithoTrack_FE_115: [70, 150] },
  small: { LithoTrack_FE_95: [50, 85], LithoTrack_FE_115: [55, 125] },
};

/** [P1] §V: CAtE (production, engineering) window hours per dataset, CoT trigger limits. */
const CATE = {
  ds3: [[151.2, 16.8], [75.6, 8.4], [21.6, 2.4]],
  ds4: [[134.6, 33.4], [67.2, 16.8], [19.2, 4.8]],
};
const COT = [100, 50, 25, 10];

/** A failure shown to the user: a text key and its values. */
class Failure extends Error {
  constructor(key, params) {
    super(key);
    this.key = key;
    this.params = params;
  }
}

const $ = (id) => document.getElementById(id);
const form = $("setup");
const fields = form.elements;
const threads = navigator.hardwareConcurrency || 4;
let active = null;
let finished = null;
let statusText = () => "";
/** Python wheels published next to the page: file URLs. */
let wheels = [];

initLanguage();
const languages = Object.entries(LANGUAGES);
$("language").replaceChildren(
  ...languages.map(([code, name]) => new Option(name, code, false, code === language())),
);
describeForm();
setStatus(() => t("status.loadingWasm"));
loadWheels();
reveal(document.querySelectorAll(".sidebar .step, .sidebar .actions, #welcome"));

$("language").addEventListener("change", (event) => {
  setLanguage(event.target.value);
  describeForm();
  setStatus(statusText);
  showWheels();
  if (active) showProgress(active);
  if (finished) showResults(finished, $("period").value, false);
});
form.addEventListener("change", (event) => {
  if (event.target.name !== "dataset") return;
  selectDataset();
  pulse(event.target.closest(".choice"));
});
// The ? buttons open and close the explanation of their field.
form.addEventListener("click", (event) => {
  const button = event.target.closest(".info");
  if (!button) return;
  const open = button.getAttribute("aria-expanded") !== "true";
  button.setAttribute("aria-expanded", String(open));
  toggle($(button.getAttribute("aria-controls")), open);
});
form.addEventListener("submit", (event) => {
  event.preventDefault();
  start();
});
$("cancel").addEventListener("click", () => stop(() => t("status.cancelled")));
$("period").addEventListener("change", () => showResults(finished, $("period").value, true));
$("download-json").addEventListener("click", downloadJson);
$("download-csv").addEventListener("click", () =>
  download(`${finished.name}.csv`, csv(finished.summary), "text/csv"),
);
// The run button stays disabled until the wasm module (summaries) is ready.
try {
  await init();
  fields.run.disabled = false;
  setStatus(() => "");
} catch (error) {
  setStatus(() => t("status.wasmFailed", { message: error.message }));
}

/** Form texts that depend on the language and the device: engineering choices, threads. */
function describeForm() {
  selectDataset();
  $("replications-hint").textContent = t("settings.replications.hint", { threads });
}

/** Fields of the chosen dataset (also one the browser restored): its file input, if a file, and
 * its engineering strategies. */
function selectDataset() {
  $("file-field").hidden = fields.dataset.value !== "file";
  fillEngineering();
}

/** Engineering strategies of the chosen dataset, keeping the chosen one if it remains. */
function fillEngineering() {
  const select = fields.engineering;
  const chosen = select.value;
  const choices = engineeringChoices(fields.dataset.value);
  select.replaceChildren(...choices.map(([value, label]) => new Option(label, value)));
  select.value = choices.some(([value]) => value === chosen) ? chosen : "base";
  select.disabled = choices.length === 1;
}

/** [value, label] of the engineering strategies of `dataset`: none without engineering lots (DS1,
 * DS2), the dataset's CAtE windows for DS3 and DS4, both sets for a dataset file. */
function engineeringChoices(dataset) {
  if (dataset === "ds1" || dataset === "ds2") return [["base", t("strategy.engineering.none")]];
  const choices = [
    ["base", t("strategy.engineering.base")],
    ["engineering_first", t("strategy.engineering.ef")],
  ];
  const sets = CATE[dataset] ? [dataset] : Object.keys(CATE);
  const label = sets.length > 1 ? "strategy.engineering.cateSet" : "strategy.engineering.cate";
  for (const set of sets) {
    for (const [production, engineering] of CATE[set]) {
      const cycle = Math.round(production + engineering);
      const params = { cycle, production, engineering, dataset: set.toUpperCase() };
      choices.push([`cate:${production}:${engineering}`, t(label, params)]);
    }
  }
  for (const trigger of COT) {
    choices.push([`cot:${trigger}`, t("strategy.engineering.cot", { trigger })]);
  }
  return choices;
}

/** The run configuration of the form, in the schema of every interface (times in ms). */
function config() {
  const [rule, first, second] = fields.engineering.value.split(":");
  const engineering =
    rule === "cate"
      ? { cate: { production: Number(first) * HOUR, engineering: Number(second) * HOUR } }
      : rule === "cot"
        ? { cot: { trigger: Number(first) } }
        : rule;
  const preset = STOPPING[fields.stopping.value];
  const stopping = preset && {
    limits: Object.fromEntries(
      Object.entries(preset).map(([group, [front, total]]) => [group, { front, total }]),
    ),
  };
  return {
    horizon: Number(fields.horizon.value) * DAY,
    seed: Number(fields.seed.value),
    replication: 0,
    load: Number(fields.load.value),
    reserve_super_hot: fields.reserveSuperHot.checked,
    queue_time: fields.queueTime.value,
    stopping,
    engineering,
  };
}

/** The form's choices as [label key, text] pairs; texts follow the language. */
function setupOf(dataset, name) {
  const rule = fields.queueTime.value;
  const preset = fields.stopping.value;
  const choice = fields.engineering.value;
  const reserve = fields.reserveSuperHot.checked;
  const entered = (field) => {
    const text = fields[field].value;
    return () => text;
  };
  return [
    ["dataset.heading", () => datasetName(dataset, name)],
    ["strategy.queueTime.label", () => t(`strategy.queueTime.${rule}`)],
    ["strategy.stopping.label", () => t(`strategy.stopping.${preset}`)],
    [
      "strategy.engineering.label",
      () => engineeringChoices(dataset).find(([value]) => value === choice)[1],
    ],
    ["strategy.superHot.short", () => t(reserve ? "common.on" : "common.off")],
    ["settings.horizon.label", entered("horizon")],
    ["settings.replications.label", entered("replications")],
    ["settings.seed.label", entered("seed")],
    ["settings.load.label", entered("load")],
  ];
}

function datasetName(dataset, name) {
  return dataset === "file" ? name : t(`dataset.${dataset}.name`);
}

/** Bytes of the chosen dataset: a file served next to the page or a local one. */
async function datasetBytes(dataset, file) {
  if (dataset === "file") return file.arrayBuffer();
  const path = `data/${dataset}.bin`;
  const response = await fetch(path);
  if (!response.ok) throw new Failure("error.fetch", { file: path, status: response.status });
  return response.arrayBuffer();
}

async function start() {
  const dataset = fields.dataset.value;
  const file = fields.file.files[0];
  if (dataset === "file" && !file) {
    setStatus(() => t("error.noFile"));
    return;
  }
  const name = dataset === "file" ? file.name.replace(/\.bin$/i, "") : dataset;
  const count = Number(fields.replications.value);
  const base = config();
  const run = {
    name,
    count,
    threads: Math.min(count, threads),
    setup: setupOf(dataset, name),
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
  $("welcome").hidden = true;
  $("results").hidden = true;
  $("run-panel").hidden = false;
  startProgress(run, (replication) => follow(run, replication));
  showProgress(run);
  reveal([$("run-panel")]);
  setStatus(() => t("status.loadingDataset", { dataset: datasetName(dataset, name) }));
  try {
    const bytes = await datasetBytes(dataset, file);
    if (run !== active) return;
    for (let index = 0; index < run.threads; index++) {
      const worker = new Worker(new URL("worker.js", import.meta.url), { type: "module" });
      worker.onmessage = ({ data: message }) => receive(run, worker, base, message);
      worker.onerror = (event) => {
        if (run !== active) return;
        stop(() => t("error.worker", { message: event.message || event.type }));
      };
      const copy = bytes.slice(0);
      worker.postMessage({ type: "load", bytes: copy }, [copy]);
      run.workers.push(worker);
    }
    setStatus(() => t("status.running"));
  } catch (error) {
    if (run !== active) return;
    stop(() =>
      error instanceof Failure
        ? t(error.key, error.params)
        : t("status.error", { message: error.message }),
    );
  }
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
  $("run-panel").hidden = true;
  $("results").hidden = false;
  showResults(finished, defaultPeriod(finished), true);
  $("results").scrollIntoView({ behavior: "smooth", block: "start" });
}

/** Ends the active run, if any, by terminating its workers, and shows `render()` and the results
 * of the last finished run. */
function stop(render) {
  if (active) {
    for (const worker of active.workers) worker.terminate();
    cancelAnimationFrame(active.frame);
    active = null;
  }
  setRunning(false);
  setStatus(render);
  $("run-panel").hidden = true;
  $("results").hidden = !finished;
  $("welcome").hidden = Boolean(finished);
}

/** While running, the settings are locked. */
function setRunning(running) {
  fields.run.disabled = running;
  $("cancel").disabled = !running;
  for (const step of form.querySelectorAll("fieldset")) step.disabled = running;
}

/** Shows the status text `render()` returns; it is rendered again when the language changes. */
function setStatus(render) {
  statusText = render;
  $("status").textContent = render();
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
