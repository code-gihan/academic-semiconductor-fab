// Datasets of the page: a bundled file served next to it (data/ds1–4.bin) or a local .bin file,
// fetched or read once and decoded by the wasm module, with its info (tool groups, routes, CQT
// segments) for the editor, the labels and the checks a run would make.
import { Dataset, Simulation } from "./pkg/fab_wasm.js";
import { Failure, t } from "./i18n.js";

export const BUNDLED = ["ds1", "ds2", "ds3", "ds4"];
/** Datasets by id, as promises: loads asked for together share one; a failed one is dropped. */
const loaded = new Map();

/**
 * The dataset `key` (bundled, or "file" for the local `file`): {id, key, name, bytes, dataset,
 * info}, decoded on first use.
 */
export function loadDataset(key, file) {
  const id = key === "file" ? `file:${file.name}:${file.size}:${file.lastModified}` : key;
  let entry = loaded.get(id);
  if (!entry) {
    entry = decode(id, key, file);
    loaded.set(id, entry);
    entry.catch(() => loaded.delete(id));
  }
  return entry;
}

async function decode(id, key, file) {
  const name = key === "file" ? file.name.replace(/\.bin$/i, "") : key;
  const bytes = await datasetBytes(key, file);
  const dataset = new Dataset(new Uint8Array(bytes));
  return { id, key, name, bytes, dataset, info: dataset.info() };
}

/**
 * The passes a run of `config` on the dataset `entry` takes (QTS without flow factors measures
 * them first); throws the core's objection to `config`, as the run would.
 */
export function passes(entry, config) {
  const simulation = new Simulation(entry.dataset, { ...config, replication: 0 });
  try {
    return simulation.progress().passes;
  } finally {
    simulation.free();
  }
}

/** A bundled dataset's name in the page's language, or a file's name. */
export function datasetName(key, name) {
  return key === "file" ? name : t(`dataset.${key}.name`);
}

/** Bytes of the dataset: a file served next to the page or a local one. */
async function datasetBytes(key, file) {
  if (key === "file") return file.arrayBuffer();
  const path = `data/${key}.bin`;
  const response = await fetch(path);
  if (!response.ok) throw new Failure("error.fetch", { file: path, status: response.status });
  return response.arrayBuffer();
}
