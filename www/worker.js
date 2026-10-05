// One replication at a time on the wasm simulator. Messages in: {type: "load", bytes},
// {type: "run", config}. Messages out: "loaded", "progress", "done", "error".
import init, { Dataset, digest, run } from "./pkg/fab_wasm.js";

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
      const results = run(dataset, config, (progress) => {
        self.postMessage({ type: "progress", replication: config.replication, progress });
      });
      self.postMessage({
        type: "done",
        config,
        results,
        digest: digest(results),
        seconds: (performance.now() - started) / 1000,
        memoryBytes: wasm.memory.buffer.byteLength,
      });
    }
  } catch (error) {
    self.postMessage({ type: "error", message: error instanceof Error ? error.message : String(error) });
  }
};
