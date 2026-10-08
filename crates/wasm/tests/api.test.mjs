// The JavaScript module on the page's DS1, after wasm-bindgen wrote www/pkg:
// node --test crates/wasm/tests/api.test.mjs
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import {
  Dataset,
  ReplayPlayer,
  Simulation,
  compare,
  comparisonCsv,
  csv,
  daily,
  digest,
  initSync,
  summarize,
} from "../../../www/pkg/smt2020.js";

const root = new URL("../../../", import.meta.url);
initSync({ module: readFileSync(new URL("www/pkg/smt2020_bg.wasm", root)) });

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

test("segments show the lots on their clocks and their completions so far", () => {
  const info = dataset.info();
  const simulation = new Simulation(dataset, config);
  const progress = simulation.run(2 * DAY + 0.5 * HOUR);
  const segments = simulation.segments();
  assert.equal(segments.length, info.segments.length);
  // Every lot in a segment until its exit step starts, with its deadline.
  const waiting = simulation
    .lots()
    .filter((lot) => lot.cqt_exit !== null && !(lot.step === lot.cqt_exit && lot.state === "processing"));
  const byId = new Map(waiting.map((lot) => [lot.id, lot]));
  const inSegments = segments.flatMap((segment, index) =>
    segment.lots.map((lot) => ({ ...lot, segment: info.segments[index] })),
  );
  assert.ok(inSegments.length > 0);
  assert.equal(inSegments.length, waiting.length);
  for (const lot of inSegments) {
    const status = byId.get(lot.id);
    assert.deepEqual([lot.kind, lot.step, lot.state], [status.kind, status.step, status.state]);
    assert.equal(status.cqt_exit, lot.segment.exit);
    assert.equal(lot.entered + lot.segment.limit, status.cqt_deadline);
    assert.ok(lot.slack <= status.cqt_deadline - progress.now);
  }
  const total = (count) => segments.reduce((sum, segment) => sum + count(segment.cqt), 0);
  assert.equal(total((cqt) => cqt.completed), progress.cqt_completed);
  assert.equal(total((cqt) => cqt.violated), progress.cqt_violated);
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
  assert.deepEqual(simulation.recording(), {
    ...recording,
    events: { ...recording.events, lots: [] },
    replay: null,
  });
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

/** The state of a simulation after `days`: progress, lots, tools and CQT segments. */
function stateAfter(simulation, days) {
  simulation.run(days * DAY);
  return JSON.stringify([simulation.progress(), simulation.lots(), simulation.tools(), simulation.segments()]);
}

test("strategy code runs the papers' rules as the built-in rules do", () => {
  const info = dataset.info();
  const horizon = { horizon: 30 * DAY };
  // The same priority everywhere ranks nothing; the views carry the lot.
  let seen = null;
  const constant = {
    priority(lot, now) {
      seen ??= { kind: lot.kind, toolGroup: lot.toolGroup, stepName: lot.stepName, now, cqt: lot.cqt && lot.cqt.limit };
      return 0;
    },
  };
  assert.equal(
    stateAfter(new Simulation(dataset, { ...horizon, queue_time: "code" }, null, constant), 5),
    stateAfter(new Simulation(dataset, horizon), 5),
  );
  assert.equal(typeof seen.kind, "string");
  assert.equal(typeof seen.toolGroup, "string");
  assert.equal(typeof seen.now, "number");
  // The end of the lot's segment, as code, ranks as the built-in criterion.
  const exits = [...new Set(info.segments.map((segment) => info.tool_groups[info.routes[segment.route].steps[segment.exit].tool_group].name))];
  const ranking = (criterion) => ({ ...horizon, ranking: Object.fromEntries(exits.map((name) => [name, [criterion, "fifo"]])) });
  const deadline = { priority: (lot) => (lot.cqt ? lot.cqt.deadline : Infinity) };
  assert.equal(
    stateAfter(new Simulation(dataset, ranking("code"), null, deadline), 5),
    stateAfter(new Simulation(dataset, ranking("qt_deadline")), 5),
  );
  // Stopping at the steppers, as admission code.
  const steppers = ["LithoTrack_FE_95", "LithoTrack_FE_115"];
  const stopping = { limits: Object.fromEntries(steppers.map((name) => [name, { front: 5, total: 10 }])) };
  let held = 0;
  const admission = {
    admit(lot, segment) {
      const reached = segment.groups.some((group) => {
        const [front, total] = steppers.includes(group.toolGroup) ? [5, 10] : [1000, 1000];
        return group.front >= front || group.total >= total;
      });
      held += reached ? 1 : 0;
      return !reached;
    },
  };
  assert.equal(
    stateAfter(new Simulation(dataset, horizon, null, admission), 10),
    stateAfter(new Simulation(dataset, { ...horizon, stopping }), 10),
  );
  assert.ok(held > 0);
  // Batches below their minimum start at an hour of slack, as code.
  let started = 0;
  const batches = {
    startBatch(batch, now) {
      if (batch.slack === null) return false;
      if (batch.slack <= HOUR) {
        started += 1;
        return true;
      }
      return Math.ceil(now + batch.slack - HOUR);
    },
  };
  assert.equal(
    stateAfter(new Simulation(dataset, horizon, null, batches), 10),
    stateAfter(new Simulation(dataset, { ...horizon, batch_start_within: HOUR }), 10),
  );
  assert.ok(started > 0);
});

test("strategy code errors stop the run", () => {
  const ranked = { horizon: 30 * DAY, queue_time: "code" };
  const thrower = new Simulation(dataset, ranked, null, {
    priority(lot) {
      return lot.cqt.slack;
    },
  });
  assert.throws(() => thrower.run(), /^Error: strategy code: priority: TypeError: .*null/);
  assert.throws(() => thrower.run(), /strategy code: priority/);
  const text = new Simulation(dataset, ranked, null, { priority: () => "soon" });
  assert.throws(() => text.run(), /strategy code: priority: returned string, not a number/);
  assert.throws(() => new Simulation(dataset, ranked).run(DAY), /ranks by code, but no strategy code was given/);
  assert.throws(() => new Simulation(dataset, ranked, null, { admit: () => true }), /has no priority/);
  assert.throws(() => new Simulation(dataset, ranked, null, {}), /defines no hook/);
  assert.throws(() => new Simulation(dataset, ranked, null, { priority: 1 }), /code.priority is not a function/);
  // Methods of a class instance see it as `this`.
  class Strategy {
    constructor() {
      this.calls = 0;
    }
    priority() {
      this.calls += 1;
      return 0;
    }
  }
  const strategy = new Strategy();
  new Simulation(dataset, ranked, null, strategy).run(DAY);
  assert.ok(strategy.calls > 0);
});

test("the SMAT2022 layout and AMHS show where vehicles and FOUPs are", () => {
  const smat = new Dataset(readFileSync(new URL("www/data/smat2022.bin", root)));
  const layout = smat.layout();
  assert.equal(layout.rails.length, 3424);
  assert.equal(layout.ports.length, 22120);
  assert.equal(dataset.layout(), null);
  const simulation = new Simulation(smat, { horizon: 730 * DAY, amhs: { vehicles: 300 } });
  simulation.run(HOUR);
  const amhs = simulation.amhs();
  assert.equal(amhs.vehicles.length, 300);
  assert.equal(amhs.tools.length, smat.info().tool_groups.reduce((sum, group) => sum + group.tools, 0));
  const lots = simulation.lots();
  const carried = amhs.vehicles.flatMap((vehicle) => (vehicle.lot == null ? [] : [vehicle.lot]));
  assert.deepEqual(
    carried.sort((a, b) => a - b),
    lots.filter((lot) => lot.vehicle != null).map((lot) => lot.id),
  );
  const atPorts = new Set(lots.filter((lot) => lot.port != null).map((lot) => lot.id));
  assert.ok(amhs.foups.every((foup) => atPorts.has(foup.lot)));
  assert.equal(new Simulation(dataset, config).amhs(), null);
  assert.throws(() => new Simulation(dataset, { ...config, amhs: {} }), /no AMHS layout/);
  smat.free();
});

test("a recorded replay plays every vehicle, FOUP and tool as the run had them", () => {
  const smat = new Dataset(readFileSync(new URL("www/data/smat2022.bin", root)));
  const config = { horizon: 730 * DAY, amhs: { vehicles: 200 } };
  const [from, until] = [HOUR / 2, HOUR];
  const recorded = new Simulation(smat, config, { replay: { from, until } });
  assert.equal(recorded.replay(), null);
  recorded.run(until);
  const bytes = recorded.replay();
  assert.ok(bytes instanceof Uint8Array);
  const player = new ReplayPlayer(smat, bytes);
  assert.deepEqual(player.window(), { from, until });
  const plain = new Simulation(smat, config);
  const seen = [];
  for (let at = from; at < until; at += 61_373) {
    plain.run(at);
    seen.push([at, plain.amhs()]);
  }
  // Played forward, then sought backward.
  for (const [at, amhs] of [...seen, ...[...seen].reverse()]) {
    const frame = player.frame(at);
    const { vehicles, foups } = frame;
    assert.equal(frame.time, at);
    assert.equal(vehicles.x.length, amhs.vehicles.length);
    amhs.vehicles.forEach((vehicle, id) => {
      const off = Math.hypot(vehicle.x - vehicles.x[id], vehicle.y - vehicles.y[id]);
      const rear = Math.hypot(vehicle.tail_x - vehicles.tail_x[id], vehicle.tail_y - vehicles.tail_y[id]);
      assert.ok(off < 1.1e-3 && rear < 1.1e-3, `vehicle ${id} at ${at}: ${off}, ${rear} mm off`);
      // The rear lies along the rails, the body (784 mm) no longer than itself.
      const body = Math.hypot(vehicles.x[id] - vehicles.tail_x[id], vehicles.y[id] - vehicles.tail_y[id]);
      assert.ok(body > 500 && body <= 784 + 1e-6, `vehicle ${id} at ${at}: body ${body} mm`);
      assert.equal(vehicles.activity[id], vehicle.activity);
    });
    const atPorts = [...foups.lot].flatMap((lot, row) =>
      foups.place[row] === "port" ? [[lot, foups.index[row]]] : [],
    );
    assert.deepEqual(new Map(atPorts), new Map(amhs.foups.map((foup) => [foup.lot, foup.port])));
    assert.deepEqual(frame.tools, amhs.tools);
  }
  assert.throws(() => new ReplayPlayer(smat, bytes.slice(0, bytes.length >> 1)), /corrupt replay/);
  assert.throws(() => new ReplayPlayer(dataset, bytes), /no AMHS layout/);
  for (const each of [player, recorded, plain, smat]) each.free();
});
