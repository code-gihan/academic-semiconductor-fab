// One replication at a time on the wasm simulator. Messages in: {type: "load", bytes},
// {type: "run", config}. Messages out: "loaded", "progress" (with the tool groups), "done",
// "error".
import init, { Dataset, Simulation, digest } from "./pkg/fab_wasm.js";

/** Wall time between progress messages: the run pauses at the next simulated day after it. */
const SHOW_EVERY_MS = 250;

const ready = init();
let dataset = null;

self.onmessage = async ({ data: message }) => {
  const wasm = await ready;
  try {
    if (message.type === "load") {
      dataset?.free();
      dataset = new Dataset(new Uint8Array(message.bytes));
      self.postMessage({ type: "loaded" });
    } else if (message.type === "run") {
      const { config } = message;
      const started = performance.now();
      const simulation = new Simulation(dataset, config);
      try {
        // Paused to show the fab, which leaves the results unchanged.
        let shown = started;
        let progress;
        do {
          progress = simulation.run(undefined, () => performance.now() - shown < SHOW_EVERY_MS);
          shown = performance.now();
          self.postMessage({
            type: "progress",
            replication: config.replication,
            progress,
            toolGroups: simulation.toolGroups(),
          });
        } while (!progress.finished);
        const results = simulation.results();
        self.postMessage({
          type: "done",
          config,
          results,
          digest: digest(results),
          seconds: (performance.now() - started) / 1000,
          memoryBytes: wasm.memory.buffer.byteLength,
        });
      } finally {
        simulation.free();
      }
    }
  } catch (error) {
    const text = error instanceof Error ? error.message : String(error);
    self.postMessage({ type: "error", message: text });
  }
};
