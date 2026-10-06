// The Home view: what the page does at a glance, a critical queue time as a picture
// (explainer.js) and the race: [P2]'s three dispatching rules on dataset 2 (presets.js), run side
// by side under the same random numbers. While it runs, each rule shows its progress and its CQT
// violations so far, and their lines grow in a chart; then the papers' main measures of a rule
// against BASE, with the core's paired differences and their verdicts. The race is a run of the
// page like any other: main.js runs it on the worker pool, and its results also fill the Analysis
// and Compare views.
import { compare } from "./pkg/fab_wasm.js";
import { liveLines } from "./charts.js";
import { loadDataset, passes } from "./datasets.js";
import { explain } from "./explainer.js";
import { failureText, formatDuration, formatNumber, t } from "./i18n.js";
import { MAIN, lookup, tile, valueText, verdict } from "./kpis.js";
import { glide, reveal, still } from "./motion.js";
import { grow, laneFraction, shareSoFar } from "./progress.js";
import { RACE } from "./presets.js";
import { defaultPeriod } from "./results.js";
import { describe } from "./setup.js";

const DAY = 86_400_000;
const $ = (id) => document.getElementById(id);
/** Colour tokens of the rules, in race order: BASE in grey, then the CQT rules. */
const KINDS = ["rule-base", "rule-1", "rule-2"];
const CQT = MAIN[0];

/**
 * The view; `start(batch)` runs a batch (main.js) and returns its run, `stop()` ends the run.
 * `ready()` is called once the wasm module works, `relabel()` after a language change,
 * `lock(running)` while any run goes, `update(run)` at a frame with new progress, `finish(run,
 * finished)` when a run is done, `stopped(run)` when it was cancelled or failed and `shown(view)`
 * when a view shows.
 */
export function homeView({ start, stop }) {
  const button = $("race-start");
  /** The latest race: {run, names, horizon, replications, warmUp, points, chart, result}. */
  let race = null;
  let ready = false;
  let locked = false;
  let visible = false;
  let statusText = () => "";

  button.addEventListener("click", begin);
  $("race-stop").addEventListener("click", () => stop());
  document.addEventListener("visibilitychange", play);
  renderLanes();
  showMeta();
  showButton();

  function play() {
    explain($("explainer"), visible && !document.hidden);
  }

  /** The race on dataset 2: its file, the rules checked by the core, then the run. */
  async function begin() {
    if (!ready || locked) return;
    button.disabled = true;
    setStatus(() => t("race.loading"));
    let entry;
    let scenarios;
    const { horizon, warm_up: warmUp, seed, load, replications } = RACE.settings;
    try {
      entry = await loadDataset(RACE.dataset);
      const settings = { horizon, seed, load, ...(warmUp == null ? {} : { warm_up: warmUp }) };
      scenarios = RACE.rules.map((rule) => {
        const config = { ...structuredClone(rule.strategy), ...settings };
        return {
          name: t(`race.rule.${rule.key}`),
          config,
          passes: passes(entry, config),
          setup: describe(entry.key, entry.name, config, replications),
        };
      });
    } catch (error) {
      setStatus(() => failureText(error));
      button.disabled = locked;
      return;
    }
    // Another run may have started while the file loaded.
    if (locked) {
      setStatus(() => "");
      return;
    }
    race = {
      names: scenarios.map((scenario) => scenario.name),
      horizon,
      replications,
      // The dataset's warm-up when the settings give none: up to its first reported year.
      warmUp: warmUp ?? entry.info.periods[1]?.start ?? 0,
      points: RACE.rules.map(() => []),
      result: null,
    };
    drawChart();
    $("race-chart-card").hidden = false;
    $("race-result").hidden = true;
    race.run = start({
      name: entry.name,
      dataset: { key: entry.id, bytes: entry.bytes },
      info: entry.info,
      replications,
      scenarios,
      view: "home",
    });
    showControls(true);
    renderLanes();
    reveal([$("race-chart-card")]);
    setStatus(() => t("race.running"));
    // The race starts below the hero.
    $("race").scrollIntoView({ behavior: still() ? "auto" : "smooth", block: "start" });
  }

  /** The race's chart, in the page's language, with the lines so far. */
  function drawChart() {
    const days = (value) => value / DAY;
    race.chart = liveLines({
      series: race.names.map((name, index) => ({ label: name, kind: KINDS[index] })),
      max: days(race.horizon),
      shade: { to: days(race.warmUp), label: t("chart.warmUp") },
      x: (day) => t("time.day", { day: formatNumber(day, 0) }),
      y: (share) => `${formatNumber(share, 1)}%`,
      tick: (share) => `${formatNumber(share, 0)}%`,
      label: t("race.chart"),
    });
    race.chart.set(race.points);
    $("race-chart").replaceChildren(race.chart.box);
  }

  /** The lanes of rule `index` in the run: one per replication. */
  function lanesOf(run, index) {
    return run.lanes.slice(index * race.replications, (index + 1) * race.replications);
  }

  function update(run) {
    if (!race || run !== race.run) return;
    const items = $("race-lanes").children;
    RACE.rules.forEach((_, index) => {
      const lanes = lanesOf(run, index);
      showLane(items[index], lanes);
      grow(race.points[index], shareSoFar(lanes));
    });
    race.chart.set(race.points);
    const done = run.lanes.filter((lane) => lane.state === "done").length;
    setStatus(() => t("race.progress", { done, count: run.lanes.length }));
  }

  /** A rule's progress: its share of the work done, where its runs are, its violations so far. */
  function showLane(item, lanes) {
    const fraction = lanes.reduce((sum, lane) => sum + laneFraction(lane), 0) / lanes.length;
    item.dataset.state = lanes.every((lane) => lane.state === "done")
      ? "done"
      : lanes.some((lane) => lane.state === "running")
        ? "running"
        : "waiting";
    const track = item.querySelector(".race-track");
    track.firstElementChild.style.transform = `scaleX(${fraction})`;
    track.setAttribute("aria-valuenow", String(Math.round(100 * fraction)));
    item.querySelector(".race-phase").textContent = phase(lanes);
    // Until its runs report, the paper's value stays.
    const share = shareSoFar(lanes);
    if (!share) return;
    item.dataset.value = "live";
    glide(item.querySelector(".race-number"), share.value, (value) => formatNumber(value, 1));
    item.querySelector(".race-caption").textContent = t("race.soFar");
  }

  function phase(lanes) {
    if (lanes.every((lane) => lane.state === "done")) {
      return t("race.done", { time: formatDuration(Math.max(...lanes.map((lane) => lane.seconds))) });
    }
    const started = lanes.filter((lane) => lane.progress);
    if (started.length === 0) return t("race.waiting");
    if (started.some((lane) => lane.progress.pass < lane.progress.passes - 1)) return t("race.preRun");
    const day = mean(started.map((lane) => Math.min(lane.progress.now, race.horizon) / DAY));
    if (day >= race.horizon / DAY) return t("race.finishing");
    return t("race.day", {
      day: formatNumber(Math.floor(day), 0),
      days: formatNumber(race.horizon / DAY, 0),
    });
  }

  /** The race is done: the main measures of its last reported period, the rule with the fewest
   * CQT violations shown against BASE. */
  function finish(run, finished) {
    if (!race || run !== race.run) return;
    const scenarios = finished.scenarios;
    const period = defaultPeriod(scenarios[0].view);
    const results = (scenario) => scenario.done.map((replication) => replication.results);
    const summaries = scenarios.map((scenario) => lookup(scenario.summary, period));
    const differences = scenarios.map((scenario, index) =>
      index === 0 ? null : lookup(compare(results(scenarios[0]), results(scenario)), period),
    );
    const share = (index) => summaries[index](CQT)?.mean ?? Infinity;
    let best = 1;
    for (let index = 2; index < scenarios.length; index++) {
      if (share(index) < share(best)) best = index;
    }
    const seconds = (scenario) => Math.max(...scenario.done.map((replication) => replication.seconds));
    race.run = null;
    race.result = { seconds: scenarios.map(seconds), summaries, differences, shown: best };
    showControls(false);
    showFinal();
    renderResult(true);
    showButton();
    setStatus(() => t("race.finished", { time: formatDuration(finished.seconds) }));
  }

  /** The rules' lanes after the race: done, with their CQT violations of the reported period. */
  function showFinal() {
    const items = $("race-lanes").children;
    race.result.summaries.forEach((summary, index) => {
      const item = items[index];
      item.dataset.state = "done";
      const track = item.querySelector(".race-track");
      track.firstElementChild.style.transform = "scaleX(1)";
      track.setAttribute("aria-valuenow", "100");
      item.querySelector(".race-phase").textContent = t("race.done", {
        time: formatDuration(race.result.seconds[index]),
      });
      const cqt = summary(CQT);
      item.dataset.value = cqt ? "final" : "none";
      if (cqt) glide(item.querySelector(".race-number"), cqt.mean, (value) => formatNumber(value, 1));
      item.querySelector(".race-caption").textContent = t("race.final", { runs: race.replications });
    });
  }

  /** The result: a headline, the rule choice and the main measures against BASE. */
  function renderResult(animated) {
    const { names } = race;
    const { summaries, differences, shown } = race.result;
    const base = summaries[0](CQT)?.mean;
    const value = summaries[shown](CQT)?.mean;
    const difference = differences[shown](CQT);
    const judged = difference ? verdict(difference, CQT.better) : "untested";
    const share = formatNumber((100 * Math.abs(base - value)) / base, 0);
    $("race-headline").textContent =
      judged === "better"
        ? t("race.headline.fewer", { rule: names[shown], share })
        : judged === "worse"
          ? t("race.headline.more", { rule: names[shown], share })
          : t("race.headline.same", { rule: names[shown] });
    $("race-subline").textContent = t("race.headline.values", {
      base: valueText(CQT, base),
      value: valueText(CQT, value),
      runs: race.replications,
    });
    $("race-rules").replaceChildren(
      ...names.slice(1).map((name, offset) => {
        const index = offset + 1;
        const choice = document.createElement("button");
        choice.type = "button";
        choice.textContent = name;
        choice.style.setProperty("--rule", `var(--${KINDS[index]})`);
        choice.setAttribute("aria-pressed", String(index === shown));
        choice.addEventListener("click", () => {
          if (race.result.shown === index) return;
          race.result.shown = index;
          renderResult(true);
        });
        return choice;
      }),
    );
    const tiles = MAIN.flatMap((spec, index) => {
      const summary = summaries[shown](spec);
      const baseline = summaries[0](spec);
      if (!summary || !baseline) return [];
      const note = t("race.versus", { name: names[0], value: valueText(spec, baseline.mean) });
      const difference = differences[shown](spec);
      return [tile(spec, summary, { difference, note, animated, delay: 150 + index * 90 })];
    });
    $("race-kpis").replaceChildren(...tiles);
    $("race-paper").textContent = t("race.paper", {
      values: RACE.rules.map((rule, index) => `${names[index]} ${valueText(CQT, rule.paper)}`).join(" · "),
    });
    $("race-result").hidden = false;
    if (animated) reveal([$("race-headline"), $("race-subline"), ...tiles]);
  }

  /** The rules' lanes: who they are, the paper's result, and their state in the latest race. */
  function renderLanes() {
    $("race-lanes").replaceChildren(
      ...RACE.rules.map((rule, index) => {
        const name = t(`race.rule.${rule.key}`);
        const swatch = el("span", "race-swatch");
        swatch.setAttribute("aria-hidden", "true");
        const description = t(`race.rule.${rule.key}.desc`);
        const label = el("div", "race-names", el("strong", "race-name", name), el("span", "race-desc", description));
        const track = el("div", "race-track", el("span", "race-fill"));
        track.setAttribute("role", "progressbar");
        track.setAttribute("aria-valuemin", "0");
        track.setAttribute("aria-valuemax", "100");
        track.setAttribute("aria-label", name);
        const number = el("span", "race-number", formatNumber(rule.paper, CQT.decimals));
        const value = el("div", "race-value", number, el("span", "race-unit", "%"));
        const caption = el("span", "race-caption", t("race.paper.caption"));
        const cited = el("span", "race-cited", t("race.paperValue", { value: valueText(CQT, rule.paper) }));
        const item = el(
          "li",
          "race-lane",
          el("div", "race-who", swatch, label),
          el("div", "race-run", track, el("span", "race-phase", t("race.ready"))),
          el("div", "race-score", value, caption, cited),
        );
        item.style.setProperty("--rule", `var(--${KINDS[index]})`);
        item.dataset.state = "idle";
        item.dataset.value = "paper";
        return item;
      }),
    );
    if (race?.run) update(race.run);
    else if (race?.result) showFinal();
  }

  function showMeta() {
    $("race-setup").textContent = t("race.setup", { runs: RACE.settings.replications });
    $("race-meta").textContent = t("race.meta", {
      runs: RACE.rules.length * RACE.settings.replications,
      threads: navigator.hardwareConcurrency || 4,
    });
  }

  /** The race's status; told again only when its words change (it is announced). */
  function setStatus(render) {
    statusText = render;
    const text = render();
    if ($("race-status").textContent !== text) $("race-status").textContent = text;
  }

  /** While the race runs: a link to its queue-time clocks (the Run view) and the stop. */
  function showControls(running) {
    $("race-clocks").hidden = !running;
    $("race-stop").hidden = !running;
  }

  /** The race button: a first race, or another. */
  function showButton() {
    button.querySelector("span").textContent = t(race?.result ? "race.again" : "race.start");
  }

  function relabel() {
    renderLanes();
    showMeta();
    showButton();
    if (race) drawChart();
    if (race?.result && !race.run) renderResult(false);
    setStatus(statusText);
  }

  return {
    ready() {
      ready = true;
      button.disabled = locked;
    },
    relabel,
    lock(running) {
      locked = running;
      button.disabled = !ready || running;
    },
    update,
    finish,
    stopped(run, render) {
      if (!race || run !== race.run) return;
      race.run = null;
      showControls(false);
      // The lanes keep where they were, still.
      for (const item of $("race-lanes").children) {
        if (item.dataset.state !== "done") item.dataset.state = "stopped";
      }
      setStatus(render);
    },
    shown(view) {
      visible = view === "home";
      play();
    },
  };
}

function mean(values) {
  return values.reduce((sum, value) => sum + value, 0) / values.length;
}

/** An element with a class and children (strings become text). */
function el(tag, className, ...children) {
  const node = document.createElement(tag);
  node.className = className;
  node.append(...children);
  return node;
}
