// The JavaScript module on the page's DS1, after wasm-bindgen wrote www/pkg:
// node --test crates/wasm/tests/api.test.mjs
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import {
  Dataset,
  Simulation,
  compare,
  comparisonCsv,
  csv,
  daily,
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

test("the dataset info names and indexes the dataset", () => {
  const info = dataset.info();
  assert.deepEqual([info.tool_groups.length, info.segments.length], [106, 66]);
  for (const segment of info.segments) {
    const steps = info.routes[segment.route].steps;
    assert.ok(segment.entry < segment.exit && segment.exit < steps.length);
    assert.ok(segment.tool_groups.includes(steps[segment.exit].tool_group));
  }
  assert.deepEqual(
    info.tool_groups.filter((group) => group.stepper).map((group) => group.name).sort(),
    ["LithoTrack_FE_115", "LithoTrack_FE_95"],
  );
  assert.deepEqual(info.periods.slice(0, 2).map((period) => period.name), ["WarmUp", "Period_1"]);
});

test("rankings, batch starts and a warm-up configure a run", () => {
  const info = dataset.info();
  const segment = info.segments[0];
  const exit = info.routes[segment.route].steps[segment.exit].tool_group;
  const ranking = { [info.tool_groups[exit].name]: [{ qt_within: HOUR }, "priority", "fifo"] };
  const strategy = { ...config, warm_up: 2 * DAY, batch_start_within: HOUR, ranking };
  const simulation = new Simulation(dataset, strategy);
  assert.deepEqual(simulation.config().ranking, ranking);
  simulation.run();
  const periods = simulation.results().periods.map((period) => period.name);
  assert.deepEqual(periods, ["WarmUp", "Period_1", "Drain"]);
});

test("recording leaves the results unchanged and accounts for them", () => {
  const info = dataset.info();
  const recording = {
    violations: true,
    tool_groups: true,
    events: { from: DAY, until: 2 * DAY, tool_groups: [info.tool_groups[0].name] },
  };
  const simulation = new Simulation(dataset, config, recording);
  assert.deepEqual(simulation.recording(), { ...recording, events: { ...recording.events, lots: [] } });
  const progress = simulation.run();
  const recorded = simulation.results();
  assert.equal(digest(recorded), digest(results));
  const violated = recorded.periods.reduce(
    (sum, period) => sum + period.cqt_litho.violated + period.cqt_rest.violated,
    0,
  );
  assert.equal(progress.cqt_violated, violated);
  const { violations, tool_groups: days, events } = simulation.records();
  assert.equal(violations.lot.length, violated);
  for (const table of [violations, days, events]) {
    const lengths = new Set(Object.values(table).map((column) => column.length));
    assert.equal(lengths.size, 1);
  }
  assert.equal(days.day.length, recorded.days.length * info.tool_groups.length);
  assert.ok(events.time.every((time) => time >= DAY && time < 2 * DAY));
  assert.ok(events.tool_group.every((group) => group === 0));
  assert.ok(events.part.every((part, row) => (part === null) === (events.lot[row] === null)));
  assert.equal(
    recorded.days.reduce((sum, day) => sum + day.started, 0),
    recorded.released,
  );
  assert.equal(recorded.periods[0].cqt_segments.length, info.segments.length);
  assert.equal(simulation.flowFactors(), null);
  const byDay = daily([recorded, results]);
  const wip = byDay.find((row) => row.day === 1 && row.scope === "fab" && row.measure === "wip");
  assert.deepEqual([wip.n, wip.std], [2, 0]);
});

test("summaries and CSV", () => {
  const summaries = summarize([results, results]);
  const completed = summaries.find(
    (row) => row.period === "WarmUp" && row.scope === "fab" && row.measure === "completed",
  );
  assert.deepEqual([completed.n, completed.std], [2, 0]);
  assert.ok(csv(summaries).startsWith("period,scope,item,kind,measure,n,mean,std,ci95\n"));
});

test("comparisons pair replications", () => {
  const run = (replication, queueTime) => {
    const simulation = new Simulation(dataset, { horizon: 3 * DAY, queue_time: queueTime, replication });
    simulation.run();
    return simulation.results();
  };
  const baseline = [run(0, "none"), run(1, "none")];
  const other = [run(1, "qtcr"), run(0, "qtcr")];
  const rows = compare(baseline, other);
  const vl = rows.find((row) => row.period === "WarmUp" && row.scope === "cqt" && row.item === "total" && row.measure === "vl_pct");
  assert.equal(vl.n, 2);
  assert.ok(Math.abs(vl.other - vl.baseline - vl.difference) < 1e-9);
  // A configuration against itself: no difference.
  assert.ok(compare(baseline, baseline).every((row) => row.difference === 0));
  assert.ok(comparisonCsv(rows).startsWith("period,scope,item,kind,measure,n,baseline,other,difference,std,ci95\n"));
  assert.throws(() => compare(baseline, other.slice(1)), /do not pair/);
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
  const ranking = (criteria) => new Simulation(dataset, { horizon: DAY, ranking: { Nope: criteria } });
  assert.throws(() => ranking(["fifoo"]), /unknown variant/);
  assert.throws(() => ranking(["fifo"]), /no tool group Nope/);
  assert.throws(() => new Simulation(dataset, { horizon: DAY, warm_up: DAY }), /warm-up/);
  assert.throws(() => new Simulation(dataset, config, { violation: true }), /unknown field/);
  const window = { events: { from: DAY, until: DAY } };
  assert.throws(() => new Simulation(dataset, config, window), /must end after it starts/);
});
