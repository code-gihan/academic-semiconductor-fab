// One simulation at a time on the wasm simulator, for the pool (pool.js). Messages in:
// {id, dataset, bytes?, config, code?, recording?, until?, live}; the bytes come with the first
// job of a dataset, which the worker then keeps, and the strategy code is JavaScript source,
// which runs here only. Messages out, with the job's id: "progress" (with the CQT segments if
// live), then "done" or "error".
import init, { Dataset, Simulation, digest } from "./pkg/fab_wasm.js";

/** Wall time between progress messages: the run pauses at the next simulated day after it. */
const SHOW_EVERY_MS = 250;
const MINUTE = 60_000;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;
/** The functions strategy code may define. */
const HOOKS = ["priority", "admit", "startBatch"];

const ready = init();
/** Decoded datasets by key. */
const datasets = new Map();

self.onmessage = async ({ data: job }) => {
  const wasm = await ready;
  try {
    if (job.bytes) {
      datasets.get(job.dataset)?.free();
      datasets.set(job.dataset, new Dataset(new Uint8Array(job.bytes)));
    }
    const started = performance.now();
    const code = job.code ? compile(job.code) : undefined;
    const simulation = new Simulation(datasets.get(job.dataset), job.config, job.recording, code);
    try {
      // Paused to report, which leaves the results unchanged.
      let shown = started;
      let progress;
      do {
        progress = simulation.run(job.until, () => performance.now() - shown < SHOW_EVERY_MS);
        shown = performance.now();
        self.postMessage({
          type: "progress",
          id: job.id,
          progress,
          segments: job.live ? simulation.segments() : undefined,
        });
      } while (!progress.finished && (job.until == null || progress.now < job.until));
      const results = progress.finished ? simulation.results() : null;
      self.postMessage({
        type: "done",
        id: job.id,
        config: job.config,
        progress,
        results,
        digest: results && digest(results),
        records: job.recording ? simulation.records() : null,
        flowFactors: simulation.flowFactors(),
        seconds: (performance.now() - started) / 1000,
        memoryBytes: wasm.memory.buffer.byteLength,
      });
    } finally {
      simulation.free();
    }
  } catch (error) {
    const text = error instanceof Error ? error.message : String(error);
    self.postMessage({ type: "error", id: job.id, message: text });
  }
};

/** The hooks `source` defines, its top level run once with MINUTE, HOUR and DAY given. An error
 * of a hook names its line in the source. */
function compile(source) {
  const hooks = `return {${HOOKS.map((name) => `${name}: typeof ${name} === "function" ? ${name} : undefined`).join(", ")}};`;
  let defined;
  try {
    // The source starts on the body's first line: its lines are the body's, after the wrapper's 2.
    defined = new Function("MINUTE", "HOUR", "DAY", `"use strict";${source}\n;${hooks}\n//# sourceURL=strategy.js`)(MINUTE, HOUR, DAY);
  } catch (error) {
    throw new Error(`strategy code: ${error.name}: ${error.message}`);
  }
  return Object.fromEntries(HOOKS.filter((name) => defined[name]).map((name) => [name, located(defined[name])]));
}

/** `hook`, whose errors say where in the source they arose. */
function located(hook) {
  return function (a, b, c) {
    try {
      return hook.call(this, a, b, c);
    } catch (error) {
      const text = error instanceof Error ? `${error.name}: ${error.message}` : String(error);
      const line = /strategy\.js:(\d+):\d+/.exec(error?.stack ?? "")?.[1];
      throw line ? `${text} (line ${Number(line) - 2})` : text;
    }
  };
}
