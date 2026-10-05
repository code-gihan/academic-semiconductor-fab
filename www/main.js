// SMT2020 simulator page: replications run in Web Workers, one replication per worker and at most
// navigator.hardwareConcurrency at once; the wasm module then summarizes them. Everything reacts
// to events (form input, worker messages); nothing polls.
import init, { csv, summarize } from "./pkg/fab_wasm.js";

const DAY = 86_400_000;
const HOUR = 3_600_000;

/** Dataset files served next to the page (`smt2020 convert` output). */
const DATASETS = {
  ds1: { label: "DS1 HV/LM", file: "data/ds1.bin" },
  ds2: { label: "DS2 LV/HM", file: "data/ds2.bin" },
  ds3: { label: "DS3 HV/LM + 엔지니어링", file: "data/ds3.bin" },
  ds4: { label: "DS4 LV/HM + 엔지니어링", file: "data/ds4.bin" },
};

/** [P2] Table 3: stepper limits (front/total); other tool groups 1,000/1,000. */
const STOPPING = {
  high: { LithoTrack_FE_95: [90, 130], LithoTrack_FE_115: [90, 220] },
  medium: { LithoTrack_FE_95: [60, 95], LithoTrack_FE_115: [70, 150] },
  small: { LithoTrack_FE_95: [50, 85], LithoTrack_FE_115: [55, 125] },
};

/** [P1] §V: CAtE (production, engineering) interval hours per dataset, CoT trigger limits. */
const CATE = {
  ds3: [[151.2, 16.8], [75.6, 8.4], [21.6, 2.4]],
  ds4: [[134.6, 33.4], [67.2, 16.8], [19.2, 4.8]],
};
const COT = [100, 50, 25, 10];

const KINDS = ["PRL", "PHL", "SHL", "ERL", "EHL"];
const LOT_COLUMNS = [
  ["completed", "완료 lot", 0],
  ["ct_mean_d", "ACT (d)", 2],
  ["ct_std_d", "CT 표준편차 (d)", 2],
  ["on_time_pct", "ONTIME (%)", 1],
  ["ff_mean", "FF 평균", 3],
];
const TABLES = [
  { title: "lot 유형", scope: "kind", columns: LOT_COLUMNS },
  {
    title: "흐름 계수(FF) 분위수",
    scope: "kind",
    columns: ["ff_p0", "ff_p5", "ff_p25", "ff_p50", "ff_p75", "ff_p95", "ff_p100"].map((measure) => [
      measure,
      `${measure.slice(4)}%`,
      3,
    ]),
  },
  { title: "제품 × lot 유형", scope: "lot", columns: LOT_COLUMNS },
  {
    title: "CQT 구간",
    scope: "cqt",
    columns: [
      ["completed", "완료", 0],
      ["vl_pct", "%VL", 2],
      ["vl1h_pct", "%VL1h", 2],
      ["vl2h_pct", "%VL2h", 2],
      ["vl4h_pct", "%VL4h", 2],
      ["avl_h", "AVL (h)", 3],
      ["aont_h", "AONT (h)", 3],
    ],
  },
  {
    title: "영역",
    scope: "area",
    columns: [
      ["availability_pct", "가용도 (%)", 2],
      ["sdt_share_pct", "SDT 비중 (%)", 1],
      ["util_pct", "가동률 (%)", 2],
      ["util_max_pct", "최대 TG 가동률 (%)", 2],
    ],
  },
  {
    title: "툴그룹 (가동률 순)",
    scope: "tool_group",
    sortBy: "util_pct",
    columns: [
      ["util_pct", "가동률", 2],
      ["availability_pct", "가용도", 2],
      ["down_pct", "DOWN", 2],
      ["pm_pct", "PM", 2],
      ["setup_pct", "SETUP", 2],
      ["process_pct", "PROC", 2],
      ["load_pct", "LOAD", 2],
      ["unload_pct", "UNLOAD", 2],
      ["idle_pct", "IDLE", 2],
    ],
  },
];

const $ = (id) => document.getElementById(id);
const form = $("setup");
const fields = form.elements;
const status = $("status");
const bar = $("progress");
let active = null;
let finished = null;

fillEngineering();
fields.dataset.addEventListener("change", () => {
  $("file-field").hidden = fields.dataset.value !== "file";
  fillEngineering();
});
form.addEventListener("submit", (event) => {
  event.preventDefault();
  start().catch((error) => stop(`오류: ${error.message}`));
});
$("cancel").addEventListener("click", () => stop("취소했습니다."));
$("period").addEventListener("change", () => render(finished, $("period").value));
$("download-json").addEventListener("click", downloadJson);
$("download-csv").addEventListener("click", () =>
  download(`${finished.name}.csv`, csv(finished.summary), "text/csv"),
);
// The run button stays disabled until the wasm module (summaries) is ready.
try {
  await init();
  fields.run.disabled = false;
} catch (error) {
  status.textContent = `wasm 모듈을 불러오지 못했습니다: ${error.message}`;
}

/** Engineering presets of the chosen dataset: none without engineering lots (DS1, DS2), the
 * dataset's CAtE settings for DS3 and DS4, both sets for a local file. */
function fillEngineering() {
  const options = [["base", "BASE"]];
  const dataset = fields.dataset.value;
  if (dataset === "ds1" || dataset === "ds2") {
    fields.engineering.replaceChildren(new Option("BASE (엔지니어링 lot 없음)", "base"));
    return;
  }
  options.push(["engineering_first", "EF (엔지니어링 우선)"]);
  const sets = CATE[dataset] ? [dataset] : Object.keys(CATE);
  for (const set of sets) {
    for (const [production, engineering] of CATE[set]) {
      const cycle = Math.round(production + engineering);
      const label = `CAtE ${cycle} h (${production}/${engineering})${sets.length > 1 ? ` ${set.toUpperCase()}` : ""}`;
      options.push([`cate:${production}:${engineering}`, label]);
    }
  }
  for (const trigger of COT) {
    options.push([`cot:${trigger}`, `CoT ${trigger}`]);
  }
  fields.engineering.replaceChildren(
    ...options.map(([value, label]) => new Option(label, value)),
  );
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

/** Bytes and name of the chosen dataset: a served file or a local one. */
async function dataset() {
  if (fields.dataset.value === "file") {
    const file = fields.file.files[0];
    if (!file) throw new Error("데이터셋 파일을 선택하세요.");
    return { name: file.name.replace(/\.bin$/, ""), bytes: await file.arrayBuffer() };
  }
  const { file } = DATASETS[fields.dataset.value];
  const response = await fetch(file);
  if (!response.ok) {
    throw new Error(
      `${file}이(가) 없습니다(${response.status}). smt2020 convert로 만든 파일을 '로컬 파일'로 선택하세요.`,
    );
  }
  return { name: fields.dataset.value, bytes: await response.arrayBuffer() };
}

async function start() {
  stop();
  $("results").hidden = true;
  const base = config();
  const count = Number(fields.replications.value);
  status.textContent = "데이터셋을 불러오는 중…";
  setRunning(true);
  const { name, bytes } = await dataset();
  const threads = Math.min(count, navigator.hardwareConcurrency || 4);
  const run = {
    name,
    count,
    threads,
    next: 0,
    done: [],
    workers: [],
    fractions: new Map(),
    latest: null,
    started: performance.now(),
    frame: 0,
  };
  active = run;
  for (let index = 0; index < threads; index++) {
    const worker = new Worker(new URL("worker.js", import.meta.url), { type: "module" });
    worker.onmessage = ({ data: message }) => receive(run, worker, base, message);
    worker.onerror = (event) => stop(`오류: ${event.message ?? "워커를 시작하지 못했습니다."}`);
    const copy = bytes.slice(0);
    worker.postMessage({ type: "load", bytes: copy }, [copy]);
    run.workers.push(worker);
  }
  status.textContent = `${DATASETS[name]?.label ?? name}: 복제 ${count}회, 워커 ${threads}개 시작`;
}

function receive(run, worker, base, message) {
  if (run !== active) return;
  switch (message.type) {
    case "loaded":
      assign(run, worker, base);
      break;
    case "progress": {
      const { pass, passes, now, horizon } = message.progress;
      run.fractions.set(message.replication, (pass + Math.min(now / horizon, 1)) / passes);
      run.latest = message.progress;
      run.frame ||= requestAnimationFrame(() => showProgress(run));
      break;
    }
    case "done":
      run.fractions.delete(message.config.replication);
      run.done.push(message);
      if (run.done.length === run.count) {
        finish(run);
      } else {
        assign(run, worker, base);
      }
      break;
    case "error":
      stop(`오류: ${message.message}`);
      break;
  }
}

/** Gives the worker the next replication, or ends it. */
function assign(run, worker, base) {
  if (run.next < run.count) {
    worker.postMessage({ type: "run", config: { ...base, replication: run.next++ } });
  } else {
    worker.terminate();
  }
}

/** Progress bar and the latest observation, at most once per frame. */
function showProgress(run) {
  run.frame = 0;
  if (run !== active) return;
  const last = run.latest;
  const running = [...run.fractions.values()].reduce((sum, fraction) => sum + fraction, 0);
  bar.value = (run.done.length + running) / run.count;
  const day = Math.floor(last.now / DAY);
  const phase =
    last.now > last.horizon ? `Drain(잔여 WIP ${last.wip} lot)` : `${day}/${last.horizon / DAY}일`;
  const pass = last.passes > 1 ? ` · QTS ${last.pass === 0 ? "FF 사전 실행" : "본 실행"}` : "";
  status.textContent = `복제 ${run.done.length}/${run.count} 완료 · 실행 중 ${run.fractions.size}개 · ${phase}${pass}`;
}

function finish(run) {
  const seconds = (performance.now() - run.started) / 1000;
  run.done.sort((a, b) => a.config.replication - b.config.replication);
  const summary = summarize(run.done.map((replication) => replication.results));
  const events = run.done.reduce((sum, replication) => sum + replication.results.events, 0);
  const busy = run.done.reduce((sum, replication) => sum + replication.seconds, 0);
  const memory = Math.max(...run.done.map((replication) => replication.memoryBytes));
  active = null;
  setRunning(false);
  bar.value = 1;
  status.textContent = "완료";
  finished = { ...run, summary };
  $("performance").textContent =
    `복제 ${run.count}회 · 워커 ${run.threads}개 · 전체 ${seconds.toFixed(1)} s · ` +
    `복제당 ${(busy / run.count).toFixed(1)} s · ${(events / busy / 1e6).toFixed(2)} M 사건/s · ` +
    `wasm 메모리 최대 ${(memory / 1e6).toFixed(0)} MB`;
  $("digests").replaceChildren(
    ...run.done.map((replication) => {
      const item = document.createElement("li");
      item.textContent = `복제 ${replication.config.replication}: ${replication.digest} (${replication.seconds.toFixed(1)} s)`;
      return item;
    }),
  );
  const periods = run.done[0].results.periods.map((period) => period.name);
  const period = periods.at(-2) ?? periods[0];
  $("period").replaceChildren(...periods.map((name) => new Option(name, name, false, name === period)));
  render(finished, period);
  $("results").hidden = false;
}

/** Ends the active run, if any: terminates its workers. */
function stop(message) {
  if (active) {
    for (const worker of active.workers) worker.terminate();
    cancelAnimationFrame(active.frame);
    active = null;
  }
  setRunning(false);
  if (message) status.textContent = message;
}

function setRunning(running) {
  fields.run.disabled = running;
  $("cancel").disabled = !running;
  if (running) {
    bar.hidden = false;
    bar.value = 0;
  }
}

function render(run, period) {
  const rows = run.summary.filter((row) => row.period === period);
  const at = new Map(rows.map((row) => [`${row.scope}|${row.item}|${row.kind}|${row.measure}`, row]));
  const value = (scope, item, kind, measure) => at.get(`${scope}|${item}|${kind}|${measure}`);
  const fab = (measure, decimals) => cell(value("fab", "", null, measure), decimals);
  $("fab").textContent = `평균 WIP ${fab("wip", 0)} lot · 투입 ${fab("started", 0)} · 완료 ${fab("completed", 0)} lot`;
  $("tables").replaceChildren(...TABLES.map((table) => tableOf(table, rows, value)).filter(Boolean));
}

function tableOf(spec, rows, value) {
  const keys = new Map();
  for (const row of rows.filter((row) => row.scope === spec.scope)) {
    keys.set(`${row.item}|${row.kind}`, [row.item, row.kind]);
  }
  let items = [...keys.values()].filter(([item, kind]) =>
    spec.columns.some(([measure]) => value(spec.scope, item, kind, measure)),
  );
  if (items.length === 0) return null;
  if (spec.sortBy) {
    const key = ([item, kind]) => value(spec.scope, item, kind, spec.sortBy)?.mean ?? -Infinity;
    items = items.sort((a, b) => key(b) - key(a));
  } else if (spec.scope === "kind") {
    items = items.sort(([, a], [, b]) => KINDS.indexOf(a) - KINDS.indexOf(b));
  }
  const table = document.createElement("table");
  const head = table.createTHead().insertRow();
  for (const label of ["", ...spec.columns.map(([, label]) => label)]) {
    head.append(Object.assign(document.createElement("th"), { textContent: label }));
  }
  const body = table.createTBody();
  for (const [item, kind] of items) {
    const row = body.insertRow();
    row.insertCell().textContent = [item, kind].filter(Boolean).join(" ");
    for (const [measure, , decimals] of spec.columns) {
      row.insertCell().textContent = cell(value(spec.scope, item, kind, measure), decimals);
    }
  }
  const section = document.createElement("section");
  section.append(Object.assign(document.createElement("h3"), { textContent: spec.title }));
  const scroll = document.createElement("div");
  scroll.className = "scroll";
  scroll.append(table);
  section.append(scroll);
  return section;
}

/** Mean, ± the 95% confidence interval half-width of several replications. */
function cell(summary, decimals) {
  if (!summary) return "–";
  const mean = summary.mean.toFixed(decimals);
  return summary.ci95 == null ? mean : `${mean} ± ${summary.ci95.toFixed(decimals)}`;
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
  const link = Object.assign(document.createElement("a"), { href: url, download: name });
  link.click();
  URL.revokeObjectURL(url);
}
