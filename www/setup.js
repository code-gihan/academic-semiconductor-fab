// The Setup view: one scenario, that is a dataset, a configuration in the core's schema (times in
// ms) and a number of replications, made of the dataset choice, the run settings, the strategy
// editor and the configuration's JSON. Dataset files are fetched once and decoded by the wasm
// module, for the editor (the dataset's info) and to check a configuration as a run would.
import { Dataset, Simulation } from "./pkg/fab_wasm.js";
import { t } from "./i18n.js";
import { pulse } from "./motion.js";
import { strategyEditor } from "./strategy.js";

const DAY = 86_400_000;
const HOUR = 3_600_000;
const $ = (id) => document.getElementById(id);

/** A failure shown to the user: a text key and its values. */
export class Failure extends Error {
  constructor(key, params) {
    super(key);
    this.key = key;
    this.params = params;
  }
}

/** The view; `status(render)` shows a status text, `run(scenario)` starts a run. `ready()` is
 * called once the wasm module works; `relabel()` after a language change. */
export function setupView({ status, run }) {
  const form = $("setup");
  const fields = form.elements;
  /** Decoded datasets by key: {name, bytes, dataset, info}. */
  const loaded = new Map();
  let current = null;
  /** Bumped by every dataset choice, so a slower earlier load is dropped. */
  let choice = 0;
  const config = { horizon: 730 * DAY, seed: 1, load: 1, queue_time: "none", ranking: {} };
  const editor = strategyEditor($("strategy"), showJson);

  form.addEventListener("change", (event) => {
    if (event.target.name === "dataset") {
      pulse(event.target.closest(".choice"));
      chooseDataset();
    } else if (event.target.name === "file") {
      chooseDataset();
    } else {
      readSettings();
      showJson();
    }
  });
  form.addEventListener("submit", (event) => {
    event.preventDefault();
    start();
  });
  $("config-json").addEventListener("toggle", showJson);
  $("json-apply").addEventListener("click", applyJson);
  $("json-copy").addEventListener("click", () => {
    navigator.clipboard
      .writeText($("json-text").value)
      .then(() => jsonStatus(() => t("json.copied")))
      .catch((error) => jsonStatus(() => t("status.error", { message: error.message })));
  });

  /** The wasm module works: the run button and the chosen dataset. */
  function ready() {
    fields.run.disabled = false;
    describeReplications();
    chooseDataset();
  }

  function relabel() {
    describeReplications();
    editor.render();
  }

  function describeReplications() {
    const threads = navigator.hardwareConcurrency || 4;
    $("replications-hint").textContent = t("settings.replications.hint", { threads });
  }

  /** Shows the chosen dataset's strategy editor once its file is decoded. */
  async function chooseDataset() {
    const key = fields.dataset.value;
    const file = fields.file.files[0];
    $("file-field").hidden = key !== "file";
    const token = ++choice;
    if (key === "file" && !file) {
      current = null;
      $("strategy").replaceChildren(element("p", "hint", t("error.noFile")));
      return;
    }
    const id = key === "file" ? `file:${file.name}:${file.size}:${file.lastModified}` : key;
    const name = key === "file" ? file.name.replace(/\.bin$/i, "") : key;
    try {
      let entry = loaded.get(id);
      if (!entry) {
        status(() => t("status.loadingDataset", { dataset: datasetName(key, name) }));
        const bytes = await datasetBytes(key, file);
        const dataset = new Dataset(new Uint8Array(bytes));
        entry = { key, name, bytes, dataset, info: dataset.info() };
        loaded.set(id, entry);
      }
      if (token !== choice) return;
      current = entry;
      status(() => "");
      editor.show(entry.info, key, config);
      showJson();
    } catch (error) {
      if (token !== choice) return;
      current = null;
      status(() => failureText(error));
    }
  }

  /** The run settings into the configuration. */
  function readSettings() {
    config.horizon = Number(fields.horizon.value) * DAY;
    if (fields.warmUp.value === "") delete config.warm_up;
    else config.warm_up = Number(fields.warmUp.value) * DAY;
    config.seed = Number(fields.seed.value);
    config.load = Number(fields.load.value);
  }

  /** The configuration's run settings into their fields. */
  function writeSettings() {
    fields.horizon.value = String(config.horizon / DAY);
    fields.warmUp.value = config.warm_up == null ? "" : String(config.warm_up / DAY);
    fields.seed.value = String(config.seed ?? 1);
    fields.load.value = String(config.load ?? 1);
  }

  function showJson() {
    if ($("config-json").open) {
      $("json-text").value = JSON.stringify(config, null, 2);
      jsonStatus(() => "");
    }
  }

  function jsonStatus(render) {
    $("json-status").textContent = render();
  }

  /** The configuration of the JSON text, checked by the core, replaces the edited one. */
  function applyJson() {
    if (!current) return;
    let parsed;
    try {
      parsed = JSON.parse($("json-text").value);
      if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed)) {
        throw new Error(t("json.notObject"));
      }
      check(parsed);
    } catch (error) {
      jsonStatus(() => t("status.invalid", { message: error.message }));
      return;
    }
    for (const key of Object.keys(config)) delete config[key];
    Object.assign(config, parsed);
    config.ranking ??= {};
    delete config.replication;
    writeSettings();
    editor.show(current.info, current.key, config);
    showJson();
    jsonStatus(() => t("json.applied"));
  }

  /** Throws the core's objection to `candidate` on the current dataset, if any. */
  function check(candidate) {
    new Simulation(current.dataset, { ...candidate, replication: 0 }).free();
  }

  function start() {
    if (!current) {
      status(() => t("error.noFile"));
      return;
    }
    readSettings();
    try {
      check(config);
    } catch (error) {
      status(() => t("status.invalid", { message: error.message }));
      return;
    }
    const snapshot = structuredClone(config);
    run({
      name: current.name,
      bytes: current.bytes,
      config: snapshot,
      replications: Number(fields.replications.value),
      setup: describe(current.key, current.name, snapshot, Number(fields.replications.value)),
    });
  }

  /** Datasets during a run are fixed: the settings are locked. */
  function lock(running) {
    fields.run.disabled = running;
    for (const step of form.querySelectorAll("fieldset")) step.disabled = running;
    $("strategy").inert = running;
    $("config-json").inert = running;
  }

  return { ready, relabel, lock };
}

/** The scenario as [label key, text()] pairs; texts follow the language. */
function describe(key, name, config, replications) {
  const engineering = config.engineering ?? "base";
  const ranked = Object.keys(config.ranking ?? {}).length;
  const limited = Object.keys(config.stopping?.limits ?? {}).length;
  const engineeringText = () => {
    if (typeof engineering === "string") return t(`engineering.${engineering}`);
    if (engineering.cate) {
      const { production, engineering: e } = engineering.cate;
      return t("strategy.engineering.cate", {
        cycle: round((production + e) / HOUR),
        production: round(production / HOUR),
        engineering: round(e / HOUR),
      });
    }
    return t("strategy.engineering.cot", { trigger: engineering.cot.trigger });
  };
  return [
    ["dataset.heading", () => datasetName(key, name)],
    ["strategy.queueTime.label", () => t(`strategy.queueTime.${config.queue_time ?? "none"}`)],
    ["ranking.heading", () => (ranked ? t("ranking.custom", { count: ranked }) : t("ranking.dataset"))],
    [
      "batch.heading",
      () =>
        config.batch_start_within == null
          ? t("common.off")
          : t("batch.summary", { hours: round(config.batch_start_within / HOUR) }),
    ],
    [
      "strategy.stopping.label",
      () => (config.stopping ? t("stopping.summary", { count: limited }) : t("common.off")),
    ],
    ["strategy.engineering.label", engineeringText],
    ["strategy.superHot.short", () => t(config.reserve_super_hot ? "common.on" : "common.off")],
    ["settings.horizon.label", () => String(config.horizon / DAY)],
    [
      "settings.warmUp.label",
      () => (config.warm_up == null ? t("settings.warmUp.dataset") : String(config.warm_up / DAY)),
    ],
    ["settings.replications.label", () => String(replications)],
    ["settings.seed.label", () => String(config.seed ?? 1)],
    ["settings.load.label", () => String(config.load ?? 1)],
  ];
}

function round(value) {
  return String(Math.round(value * 10) / 10);
}

export function datasetName(key, name) {
  return key === "file" ? name : t(`dataset.${key}.name`);
}

/** Bytes of the chosen dataset: a file served next to the page or a local one. */
async function datasetBytes(key, file) {
  if (key === "file") return file.arrayBuffer();
  const path = `data/${key}.bin`;
  const response = await fetch(path);
  if (!response.ok) throw new Failure("error.fetch", { file: path, status: response.status });
  return response.arrayBuffer();
}

export function failureText(error) {
  return error instanceof Failure
    ? t(error.key, error.params)
    : t("status.error", { message: error.message });
}

function element(tag, className, text) {
  const node = document.createElement(tag);
  node.className = className;
  node.textContent = text;
  return node;
}
