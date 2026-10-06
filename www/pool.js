// A pool of Web Workers (worker.js) for runs and replays: jobs queue up and go to the next free
// worker, at most navigator.hardwareConcurrency of them, each keeping the datasets it decoded.
// A job's callbacks follow its worker's messages; nothing polls.

const size = navigator.hardwareConcurrency || 4;
/** Workers: {worker, job (running or null), datasets (keys decoded)}. */
const workers = [];
const queue = [];
let nextId = 0;

/**
 * Queues `job`: {group, dataset: {key, bytes}, config, code?, recording?, until?, live?,
 * onStart?, onProgress?, onDone, onError}. `group` names the jobs `cancel` drops together; `onStart` is
 * called when a worker takes the job.
 */
export function submit(job) {
  queue.push({ ...job, id: nextId++ });
  dispatch();
}

/** Drops the queued jobs of `group` and stops its running ones. */
export function cancel(group) {
  for (let index = queue.length - 1; index >= 0; index--) {
    if (queue[index].group === group) queue.splice(index, 1);
  }
  for (const slot of [...workers]) {
    if (slot.job?.group === group) {
      // A message already on its way finds no job.
      slot.job = null;
      slot.worker.terminate();
      workers.splice(workers.indexOf(slot), 1);
    }
  }
  dispatch();
}

/** The number of workers that may run at once. */
export function capacity() {
  return size;
}

/** Gives queued jobs to free workers, starting workers up to the pool size. */
function dispatch() {
  while (queue.length > 0) {
    let slot = workers.find((each) => !each.job);
    if (!slot) {
      if (workers.length >= size) return;
      slot = start();
    }
    const job = queue.shift();
    slot.job = job;
    const known = slot.datasets.has(job.dataset.key);
    slot.datasets.add(job.dataset.key);
    const bytes = known ? undefined : job.dataset.bytes.slice(0);
    slot.worker.postMessage(
      {
        id: job.id,
        dataset: job.dataset.key,
        bytes,
        config: job.config,
        code: job.code,
        recording: job.recording,
        until: job.until,
        live: Boolean(job.live),
      },
      bytes ? [bytes] : [],
    );
    job.onStart?.();
  }
}

function start() {
  const worker = new Worker(new URL("worker.js", import.meta.url), { type: "module" });
  const slot = { worker, job: null, datasets: new Set() };
  worker.onmessage = ({ data: message }) => {
    const job = slot.job;
    if (!job || job.id !== message.id) return;
    if (message.type === "progress") {
      job.onProgress?.(message);
      return;
    }
    slot.job = null;
    if (message.type === "done") {
      job.onDone(message);
    } else {
      job.onError(message.message);
    }
    dispatch();
  };
  worker.onerror = (event) => {
    const job = slot.job;
    worker.terminate();
    workers.splice(workers.indexOf(slot), 1);
    job?.onError(event.message || event.type);
    dispatch();
  };
  workers.push(slot);
  return slot;
}
