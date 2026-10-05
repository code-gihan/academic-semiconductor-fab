// The Setup view: a dataset, the run settings and a strategy in the core's configuration schema
// (times in ms), made of the dataset choice, the run settings, the strategy editor and the
// configuration's JSON; and the scenarios to compare, strategies saved under a name and run
// together under the same settings and random numbers. Dataset files are fetched once and decoded
// by the wasm module, for the editor (the dataset's info) and to check a configuration as a run
// would. Scenarios are kept in the browser and travel in share links.
import { Dataset, Simulation } from "./pkg/fab_wasm.js";
import { t } from "./i18n.js";
import { pulse } from "./motion.js";
import { shareLink } from "./share.js";
import { strategyEditor } from "./strategy.js";

const DAY = 86_400_000;
const HOUR = 3_600_000;
const $ = (id) => document.getElementById(id);
/** Configuration fields that are run settings, shared by compared scenarios; the others make a
 * strategy. */
const RUN_KEYS = ["horizon", "warm_up", "seed", "load", "replication"];
const BUNDLED = ["ds1", "ds2", "ds3", "ds4"];
const STORAGE_KEY = "smt2020.scenarios";
/** The demo: three strategies on DS1, short enough for a phone. */
const DEMO = {
  dataset: "ds1",
  settings: { horizon: 120 * DAY, warm_up: 30 * DAY, seed: 1, load: 1, replications: 2 },
  scenarios: [
    ["demo.base", { queue_time: "none", ranking: {} }],
    ["demo.qtcr", { queue_time: "qtcr", ranking: {} }],
    ["demo.batch", { queue_time: "qtcr", ranking: {}, batch_start_within: 2 * HOUR }],
  ],
};

/** A failure shown to the user: a text key and its values. */
export class Failure extends Error {
  constructor(key, params) {
    super(key);
    this.key = key;
    this.params = params;
  }
}

/**
 * The view; `status(render)` shows a status text, `run(batch)` starts a run of
 * {name, dataset: {key, bytes}, info, replications, scenarios: [{name, config, setup}]}. `ready()`
 * is called once the wasm module works, `relabel()` after a language change and
 * `load(state, render)` with the state of a share link and the text that says so.
 */
export function setupView({ status, run }) {
  const form = $("setup");
  const fields = form.elements;
  /** Decoded datasets by id: {id, key, name, bytes, dataset, info}. */
  const loaded = new Map();
  let current = null;
  /** Bumped by every dataset choice, so a slower earlier load is dropped. */
  let choice = 0;
  const config = { horizon: 730 * DAY, seed: 1, load: 1, queue_time: "none", ranking: {} };
  const editor = strategyEditor($("strategy"), showJson);
  /** Strategies to compare: {name, strategy}. */
  let scenarios = restore();

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
  $("scenario-add").addEventListener("click", addScenario);
  $("scenarios-run").addEventListener("click", compareAll);
  $("scenarios-demo").addEventListener("click", () => {
    const scenarios = DEMO.scenarios.map(([name, strategy]) => ({ name: t(name), strategy }));
    load({ dataset: DEMO.dataset, settings: DEMO.settings, scenarios }, () => t("scenarios.demoLoaded"));
  });
  $("scenarios-share").addEventListener("click", share);

  /** The wasm module works: the run button and the chosen dataset. */
  function ready() {
    fields.run.disabled = false;
    describeReplications();
    renderScenarios();
    chooseDataset();
  }

  function relabel() {
    describeReplications();
    editor.render();
    renderScenarios();
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
        entry = { id, key, name, bytes, dataset, info: dataset.info() };
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

  /** The run of the edited strategy. */
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
    const name = datasetName(current.key, current.name);
    run(batch([{ name, config: snapshot }]));
  }

  /** A run of `scenarios` ({name, config}) on the current dataset. */
  function batch(list) {
    const replications = Number(fields.replications.value);
    return {
      name: current.name,
      dataset: { key: current.id, bytes: current.bytes },
      info: current.info,
      replications,
      scenarios: list.map(({ name, config: each }) => ({
        name,
        config: each,
        setup: describe(current.key, current.name, each, replications),
      })),
    };
  }

  // ---- scenarios ----

  /** Saves the edited strategy as a scenario. */
  function addScenario() {
    if (!current) return;
    readSettings();
    try {
      check(config);
    } catch (error) {
      scenarioStatus(() => t("status.invalid", { message: error.message }));
      return;
    }
    const input = $("scenario-name");
    const name = input.value.trim() || t("scenarios.defaultName", { number: scenarios.length + 1 });
    scenarios.push({ name, strategy: strategyOf(config) });
    input.value = "";
    keep();
    renderScenarios();
    scenarioStatus(() => t("scenarios.added", { name }));
  }

  /** Runs every scenario under the current settings, for the comparison. */
  function compareAll() {
    if (!current) return;
    if (scenarios.length < 2) {
      scenarioStatus(() => t("scenarios.tooFew"));
      return;
    }
    readSettings();
    const settings = Object.fromEntries(RUN_KEYS.filter((key) => key in config).map((key) => [key, config[key]]));
    const list = [];
    for (const scenario of scenarios) {
      const each = { ...structuredClone(scenario.strategy), ...settings };
      try {
        check(each);
      } catch (error) {
        scenarioStatus(() => t("scenarios.invalid", { name: scenario.name, message: error.message }));
        return;
      }
      list.push({ name: scenario.name, config: each });
    }
    scenarioStatus(() => "");
    run(batch(list));
  }

  function renderScenarios() {
    $("scenarios").replaceChildren(
      ...scenarios.map((scenario, index) => {
        const edit = element("button", "link own", t("scenarios.edit"));
        edit.type = "button";
        edit.addEventListener("click", () => {
          useStrategy(scenario.strategy);
          if (current) editor.show(current.info, current.key, config);
          showJson();
          scenarioStatus(() => t("scenarios.editing", { name: scenario.name }));
        });
        const remove = element("button", "icon", "✕");
        remove.type = "button";
        remove.setAttribute("aria-label", t("scenarios.remove", { name: scenario.name }));
        remove.addEventListener("click", () => {
          scenarios.splice(index, 1);
          keep();
          renderScenarios();
        });
        const title = element("strong", "", scenario.name);
        if (index === 0) title.append(element("span", "tag", t("scenarios.baseline")));
        const text = element("span", "scenario-text", "");
        text.append(title, element("span", "hint", strategySummary(scenario.strategy)));
        const item = document.createElement("li");
        item.append(text, edit, remove);
        return item;
      }),
    );
    $("scenarios-empty").hidden = scenarios.length > 0;
  }

  /** The strategy `strategy` in the editor, the run settings kept. */
  function useStrategy(strategy) {
    for (const key of Object.keys(config)) {
      if (!RUN_KEYS.includes(key)) delete config[key];
    }
    Object.assign(config, structuredClone(strategy));
    config.ranking ??= {};
  }

  /** Copies a link to the page with this dataset, these settings and the scenarios (or the
   * edited strategy). */
  async function share() {
    if (!current) return;
    if (!BUNDLED.includes(current.key)) {
      scenarioStatus(() => t("share.fileDataset"));
      return;
    }
    readSettings();
    const list = scenarios.length > 0 ? scenarios : [{ name: t("scenarios.edited"), strategy: strategyOf(config) }];
    const settings = {
      horizon: config.horizon,
      warm_up: config.warm_up ?? null,
      seed: config.seed,
      load: config.load,
      replications: Number(fields.replications.value),
    };
    let link;
    try {
      link = await shareLink({ dataset: current.key, settings, scenarios: list });
    } catch (error) {
      scenarioStatus(() => t("share.failed", { message: error.message }));
      return;
    }
    const field = $("share-link");
    field.value = link;
    field.hidden = false;
    field.select();
    // The link shows either way; the clipboard may be refused.
    try {
      await navigator.clipboard.writeText(link);
      scenarioStatus(() => t("share.copied", { count: list.length }));
    } catch {
      scenarioStatus(() => t("share.made", { count: list.length }));
    }
  }

  /** Takes the dataset, settings and scenarios of a share link or the demo, and says so with
   * `render()`. */
  function load(state, render) {
    if (!BUNDLED.includes(state?.dataset) || !Array.isArray(state.scenarios)) {
      throw new Failure("share.invalid");
    }
    fields.dataset.value = state.dataset;
    const settings = state.settings ?? {};
    config.horizon = Number(settings.horizon) || config.horizon;
    if (settings.warm_up == null) delete config.warm_up;
    else config.warm_up = Number(settings.warm_up);
    config.seed = Number(settings.seed ?? 1);
    config.load = Number(settings.load ?? 1);
    fields.replications.value = String(Number(settings.replications) || 1);
    writeSettings();
    scenarios = state.scenarios
      .filter((scenario) => scenario && typeof scenario.strategy === "object")
      .map((scenario) => ({ name: String(scenario.name ?? ""), strategy: scenario.strategy }));
    keep();
    renderScenarios();
    if (scenarios.length > 0) useStrategy(scenarios[0].strategy);
    chooseDataset();
    scenarioStatus(render);
  }

  function scenarioStatus(render) {
    $("scenarios-status").textContent = render();
  }

  function keep() {
    try {
      localStorage.setItem(STORAGE_KEY, JSON.stringify(scenarios));
    } catch {
      // Not kept; the list still works on this page.
    }
  }

  /** Datasets during a run are fixed: the settings are locked. */
  function lock(running) {
    fields.run.disabled = running;
    for (const step of form.querySelectorAll("fieldset")) step.disabled = running;
    $("strategy").inert = running;
    $("config-json").inert = running;
    $("scenarios-panel").inert = running;
  }

  return { ready, relabel, lock, load };
}

/** The scenarios kept in this browser. */
function restore() {
  try {
    const kept = JSON.parse(localStorage.getItem(STORAGE_KEY) ?? "[]");
    return Array.isArray(kept) ? kept.filter((scenario) => scenario?.strategy) : [];
  } catch {
    return [];
  }
}

/** The strategy part of a configuration: all but the run settings. */
function strategyOf(config) {
  const strategy = structuredClone(config);
  for (const key of RUN_KEYS) delete strategy[key];
  return strategy;
}

/** A strategy in a line. */
function strategySummary(strategy) {
  const parts = [t(`strategy.queueTime.${strategy.queue_time ?? "none"}`)];
  const ranked = Object.keys(strategy.ranking ?? {}).length;
  if (ranked) parts.push(t("ranking.custom", { count: ranked }));
  if (strategy.batch_start_within != null) {
    parts.push(t("batch.summary", { hours: round(strategy.batch_start_within / HOUR) }));
  }
  if (strategy.stopping) {
    parts.push(t("stopping.summary", { count: Object.keys(strategy.stopping.limits ?? {}).length }));
  }
  if ((strategy.engineering ?? "base") !== "base") parts.push(engineeringText(strategy.engineering));
  if (strategy.reserve_super_hot) parts.push(t("strategy.superHot.short"));
  return parts.join(" · ");
}

/** The scenario as [label key, text()] pairs; texts follow the language. */
function describe(key, name, config, replications) {
  const ranked = Object.keys(config.ranking ?? {}).length;
  const limited = Object.keys(config.stopping?.limits ?? {}).length;
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
    ["strategy.engineering.label", () => engineeringText(config.engineering ?? "base")],
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

/** The engineering rule: a preset's name, or the CAtE cycle or CoT trigger given. */
function engineeringText(engineering) {
  if (typeof engineering === "string") return t(`engineering.${engineering}`);
  if (engineering.cate) {
    const { production, engineering: hours } = engineering.cate;
    return t("strategy.engineering.cate", {
      cycle: round((production + hours) / HOUR),
      production: round(production / HOUR),
      engineering: round(hours / HOUR),
    });
  }
  return t("strategy.engineering.cot", { trigger: engineering.cot.trigger });
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
