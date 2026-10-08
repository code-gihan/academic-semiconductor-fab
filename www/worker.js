// One simulation at a time on the wasm simulator, for the pool (pool.js). Messages in:
// {id, dataset, bytes?, config, code?, recording?, until?, live}; the bytes come with the first
// job of a dataset, which the worker then keeps, and the strategy code is JavaScript source,
// which runs here only. Messages out, with the job's id: "progress" (with the CQT segments if
// live), then "done" (with what the recording asks for: the records of its tables, the replay of
// its window) or "error".
import { compile } from "./hooks.js";
import init, { Dataset, Simulation, digest } from "./pkg/smt2020.js";

/** Wall time between progress messages: the run pauses at the next simulated day after it. */
const SHOW_EVERY_MS = 250;

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
      // The recording asks for tables (the records) and the replay of a window, each or both.
      const { replay: window, ...tables } = job.recording ?? {};
      const replay = window ? simulation.replay() : null;
      self.postMessage(
        {
          type: "done",
          id: job.id,
          config: job.config,
          progress,
          results,
          digest: results && digest(results),
          records: Object.values(tables).some(Boolean) ? simulation.records() : null,
          replay,
          flowFactors: simulation.flowFactors(),
          seconds: (performance.now() - started) / 1000,
          memoryBytes: wasm.memory.buffer.byteLength,
        },
        replay ? [replay.buffer] : [],
      );
    } finally {
      simulation.free();
    }
  } catch (error) {
    const text = error instanceof Error ? error.message : String(error);
    self.postMessage({ type: "error", id: job.id, message: text });
  }
};
