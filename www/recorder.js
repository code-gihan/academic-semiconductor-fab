// Records a replay for the Layout view: runs the setup's simulation, recording the AMHS of a
// window (recording.replay), up to the window's end, then hands the replay over in its compact
// form. Message in: {run, key, bytes?, config, code?, window: {from, until}} (the dataset's bytes
// with the first run of it). Messages out, with the run number: {type: "progress", progress} once
// per simulated day, then {type: "replay", replay} (a Uint8Array, transferred) or {type: "error",
// message}. The page stops a recording by ending the worker.
import { compile } from "./hooks.js";
import init, { Dataset, Simulation } from "./pkg/fab_wasm.js";

const ready = init();
/** The decoded dataset and its key. */
let dataset = null;

self.onmessage = async ({ data: job }) => {
  await ready;
  let simulation = null;
  try {
    if (job.bytes) {
      dataset?.value.free();
      dataset = { key: job.key, value: new Dataset(new Uint8Array(job.bytes)) };
    }
    const code = job.code ? compile(job.code) : undefined;
    simulation = new Simulation(dataset.value, job.config, { replay: job.window }, code);
    simulation.run(job.window.until, (progress) => {
      self.postMessage({ type: "progress", run: job.run, progress });
    });
    const replay = simulation.replay();
    self.postMessage({ type: "replay", run: job.run, replay }, [replay.buffer]);
  } catch (error) {
    const text = error instanceof Error ? error.message : String(error);
    self.postMessage({ type: "error", run: job.run, message: text });
  } finally {
    simulation?.free();
  }
};
