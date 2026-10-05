// One simulation at a time on the wasm simulator, for the pool (pool.js). Messages in:
// {id, dataset, bytes?, config, recording?, until?, live}; the bytes come with the first job of a
// dataset, which the worker then keeps. Messages out, with the job's id: "progress" (with the
// tool groups if live), then "done" or "error".
import init, { Dataset, Simulation, digest } from "./pkg/fab_wasm.js";

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
    const simulation = new Simulation(datasets.get(job.dataset), job.config, job.recording);
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
          toolGroups: job.live ? simulation.toolGroups() : undefined,
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
