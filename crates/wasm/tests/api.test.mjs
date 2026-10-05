// The JavaScript module on the page's DS1, after wasm-bindgen wrote www/pkg:
// node --test crates/wasm/tests/api.test.mjs
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import {
  Dataset,
  Simulation,
  csv,
  digest,
  initSync,
  summarize,
} from "../../../www/pkg/fab_wasm.js";

const root = new URL("../../../", import.meta.url);
initSync({ module: readFileSync(new URL("www/pkg/fab_wasm_bg.wasm", root)) });

const DAY = 86_400_000;
const HOUR = 3_600_000;
const STATES = ["down", "pm", "setup", "process", "load", "unload", "idle"];
const dataset = new Dataset(readFileSync(new URL("www/data/ds1.bin", root)));
// Ten days of releases, then the drain of the lots released.
const config = { horizon: 10 * DAY };
const straight = new Simulation(dataset, config);
const finished = straight.run();
const results = straight.results();

test("a run ends when every released lot is complete", () => {
  assert.equal(finished.finished, true);
  assert.equal(finished.released, finished.completed);
  assert.equal(finished.wip, 0);
  assert.equal(results.periods.at(-1).name, "Drain");
});

test("a paused run shows the fab and resumes to the same results", () => {
  const simulation = new Simulation(dataset, config);
  const progress = simulation.run(2 * DAY + 0.5 * HOUR);
  assert.equal(progress.now, 2 * DAY + HOUR / 2);
  assert.equal(progress.finished, false);
  const lots = simulation.lots();
  const tools = simulation.tools();
  const groups = simulation.toolGroups();
  assert.equal(lots.length, progress.wip);
  assert.deepEqual(lots.map((lot) => lot.id), lots.map((lot) => lot.id).sort((a, b) => a - b));
  assert.equal(groups.reduce((sum, group) => sum + group.tools, 0), tools.length);
  assert.equal(
    groups.reduce((sum, group) => sum + group.queue, 0),
    lots.filter((lot) => lot.state === "queued").length,
  );
  const processing = Object.fromEntries(
    lots.filter((lot) => lot.state === "processing").map((lot) => [lot.id, lot.tool]),
  );
  const onTools = tools.flatMap((tool) => tool.lots.map((lot) => [lot, tool.id]));
  assert.deepEqual(processing, Object.fromEntries(onTools));
  for (const group of groups) {
    assert.equal(STATES.reduce((sum, state) => sum + group[state], 0), group.tools);
  }
  assert.throws(() => simulation.results(), /has not finished/);

  const days = [];
  const paused = simulation.run(undefined, (progress) => {
    days.push(progress.now / DAY);
    return progress.now < 5 * DAY;
  });
  assert.equal(paused.now, 5 * DAY);
  assert.deepEqual(days, [3, 4, 5]);
  assert.deepEqual(simulation.run(), finished);
  assert.equal(digest(simulation.results()), digest(results));
});

test("an error thrown by the observer pauses the run", () => {
  const simulation = new Simulation(dataset, config);
  assert.throws(
    () =>
      simulation.run(undefined, (progress) => {
        throw new RangeError(String(progress.now));
      }),
    RangeError,
  );
  assert.equal(simulation.progress().now, DAY);
  simulation.run();
  assert.equal(digest(simulation.results()), digest(results));
});

test("reset starts over", () => {
  const simulation = new Simulation(dataset, config);
  simulation.run(DAY);
  simulation.reset();
  assert.deepEqual([simulation.progress().now, simulation.progress().released], [0, 0]);
  const stopping = { limits: { LithoTrack_FE_95: { front: 50, total: 85 } } };
  simulation.reset({ ...config, queue_time: "qtcr", stopping });
  assert.equal(simulation.config().queue_time, "qtcr");
  assert.equal(simulation.config().seed, 1);
  // A rejected configuration leaves the simulation as it was.
  assert.throws(() => simulation.reset({ horizon: 0 }), /horizon must be positive/);
  assert.equal(simulation.config().queue_time, "qtcr");
});

test("summaries and CSV", () => {
  const summaries = summarize([results, results]);
  const completed = summaries.find(
    (row) => row.period === "WarmUp" && row.scope === "fab" && row.measure === "completed",
  );
  assert.deepEqual([completed.n, completed.std], [2, 0]);
  assert.ok(csv(summaries).startsWith("period,scope,item,kind,measure,n,mean,std,ci95\n"));
});

test("bad input is rejected", () => {
  assert.throws(() => new Simulation(dataset, { horizon: DAY, sead: 2 }), /unknown field/);
  const stopping = { limits: { Nope: { front: 1, total: 1 } } };
  assert.throws(
    () => new Simulation(dataset, { horizon: DAY, queue_time: "qts", stopping }),
    /no tool group Nope/,
  );
  assert.throws(() => new Simulation(dataset, config).run(Number.NaN), /not a time/);
  assert.throws(() => new Dataset(new Uint8Array([1, 2, 3])), /not an SMT2020 dataset file/);
});
