// The Layout view: a replay of the transport on the fab floor of a dataset with an AMHS layout
// (SMAT2022). A worker of its own (recorder.js) runs the setup's simulation up to the end of the
// chosen window, recording every vehicle move, FOUP move and tool state in it; the view then
// plays that record (the core's ReplayPlayer) at the chosen speed, or at the instant the time
// bar is dragged to. The rails, bays and footprints are drawn once into a background until the
// view moves; each frame adds the tools' states, the FOUPs and the vehicles. Everything reacts
// to events (animation frames while playing, worker messages, input, resizes, the colour
// scheme); nothing polls.
import { formatNumber, t } from "./i18n.js";
import { ReplayPlayer } from "./pkg/smt2020.js";
import { dayText } from "./progress.js";

const DAY = 86_400_000;
/** Simulated ms per wall ms offered, and the one chosen first. */
const SPEEDS = [1, 10, 60, 600];
const FIRST_SPEED = 60;
/** Wall time one frame may cover at most (ms): a slow frame is not made up for. */
const MAX_STEP = 100;
/** Drawn sizes (mm): a vehicle's width (its length is the dataset's), a FOUP, a bay's margin. */
const VEHICLE_WIDTH = 600;
const FOUP = 450;
const BAY_MARGIN = 1500;
/** Below this many px per vehicle length vehicles are dots; track buffers show from this scale
 * (px per mm) on, bay names from that. */
const DOT_BELOW = 5;
const BUFFERS_FROM = 0.02;
const NAMES_FROM = 0.006;
/** Lot kinds drawn as hot. */
const HOT = new Set(["PHL", "SHL", "EHL"]);
/** Vehicle activities and tool states by their colour token. */
const ACTIVITY_COLOUR = {
  idle: "idle",
  to_pickup: "accent",
  loading: "setup",
  to_dropoff: "transport",
  unloading: "setup",
};
const STATE_COLOUR = {
  process: "process",
  setup: "setup",
  load: "setup",
  unload: "setup",
  down: "down",
  pm: "pm",
};
const LEGEND = [
  ["vehicle", "idle", "layout.legend.idle"],
  ["vehicle", "accent", "layout.legend.toPickup"],
  ["vehicle", "setup", "layout.legend.hoist"],
  ["vehicle", "transport", "layout.legend.loaded"],
  ["foup", "text-2", "layout.legend.foup"],
  ["foup", "warning", "layout.legend.hot"],
  ["tool", "process", "layout.legend.process"],
  ["tool", "setup", "layout.legend.setup"],
  ["tool", "down", "layout.legend.down"],
  ["tool", "pm", "layout.legend.pm"],
];
/** Colour tokens read from the page's CSS. */
const TOKENS = ["card", "surface", "line", "line-strong", "text", "text-2", "muted", "accent", "process", "setup", "down", "pm", "idle", "transport", "warning"];
/** Size of the floor's labels (px). */
const LABEL = 11;

const $ = (id) => document.getElementById(id);

/**
 * The view; `setup` is the Setup view, whose `snapshot()` is the setup to simulate. A finished
 * run of the page may have done part of the work: `finishedReplay(key, config, code, window)` is
 * the replay it recorded of `window` with `config` and strategy `code` on the dataset `key`, and
 * `flowFactors(key, config)` the QTS flow factors it measured with `config`, which spare a
 * recording its pre-run (null if none). `ready()` is called once the wasm module works,
 * `shown(view)` whenever a view is shown, `changed()` after an edit of the setup, `relabel()`
 * after a language change, and `window()` is the window the view's inputs choose.
 */
export function layoutView({ setup, finishedReplay, flowFactors }) {
  const canvas = $("layout-canvas");
  const context = canvas.getContext("2d");
  const background = document.createElement("canvas");
  const speed = $("layout-speed");
  const seek = $("layout-seek");
  let worker = recorder();
  /** The dataset key whose bytes the worker has. */
  let sent = null;
  /** Bumped by every recording: messages of an earlier one are dropped. */
  let run = 0;
  /** The recording under way: its setup, first day, window and length. */
  let recording = null;
  /** The replay shown: the setup it was simulated with, its drawing, player and window (from,
   * until), and the frame drawn last. */
  let shownReplay = null;
  /** The replay's time shown (ms) and the wall time of the last frame while playing. */
  let time = 0;
  let playing = false;
  let lastWall = null;
  /** The world point (mm) at the canvas centre and the scale (CSS px per mm). */
  const view = { x: 0, y: 0, scale: 0.004 };
  let size = { width: 0, height: 0, ratio: 1 };
  let colours = readColours();
  let drag = null;
  let visible = false;
  let wasmReady = false;
  /** The status line, again after a language change. */
  let statusText = () => "";

  speed.replaceChildren(...SPEEDS.map((factor) => new Option("", String(factor), false, factor === FIRST_SPEED)));
  relabel();

  $("layout-window").addEventListener("submit", (event) => {
    event.preventDefault();
    record();
  });
  $("layout-stop").addEventListener("click", stop);
  $("layout-play").addEventListener("click", () => (playing ? pause() : play()));
  seek.addEventListener("input", () => {
    time = Number(seek.value);
    draw();
  });
  $("layout-fit").addEventListener("click", () => {
    fit();
    redraw();
  });
  $("layout-zoom-in").addEventListener("click", () => zoom(1.6, size.width / 2, size.height / 2));
  $("layout-zoom-out").addEventListener("click", () => zoom(1 / 1.6, size.width / 2, size.height / 2));

  canvas.addEventListener("wheel", (event) => {
    event.preventDefault();
    const [x, y] = offset(event);
    zoom(Math.exp(-event.deltaY * 0.0015), x, y);
  }, { passive: false });
  canvas.addEventListener("pointerdown", (event) => {
    if (event.button !== 0) return;
    canvas.setPointerCapture(event.pointerId);
    drag = { x: event.clientX, y: event.clientY, viewX: view.x, viewY: view.y };
    hideTip();
  });
  canvas.addEventListener("pointermove", (event) => {
    if (drag) {
      view.x = drag.viewX - (event.clientX - drag.x) / view.scale;
      view.y = drag.viewY + (event.clientY - drag.y) / view.scale;
      redraw();
    } else {
      showTip(event);
    }
  });
  const release = () => {
    drag = null;
  };
  canvas.addEventListener("pointerup", release);
  canvas.addEventListener("pointercancel", release);
  canvas.addEventListener("pointerleave", hideTip);
  new ResizeObserver(() => {
    resize();
    redraw();
  }).observe($("layout-stage"));
  matchMedia("(prefers-color-scheme: dark)").addEventListener("change", () => {
    colours = readColours();
    redraw();
  });

  /** The worker that records replays, its messages heard. */
  function recorder() {
    const each = new Worker(new URL("./recorder.js", import.meta.url), { type: "module" });
    each.addEventListener("message", ({ data }) => {
      if (data.run !== run) return;
      if (data.type === "progress") {
        progress(data.progress);
      } else if (data.type === "replay") {
        recorded(data.replay);
      } else {
        finishRecording();
        status(() => t("layout.failed", { message: data.message }));
      }
    });
    each.addEventListener("error", (event) => {
      finishRecording();
      status(() => t("layout.failed", { message: event.message }));
    });
    return each;
  }

  /** The wasm module works: the setup can be simulated. */
  function ready() {
    wasmReady = true;
    showSetup();
  }

  /** The Layout view shows: its controls for the setup; another view pauses the replay. */
  function shown(name) {
    visible = name === "layout";
    if (!visible) {
      pause();
      return;
    }
    showSetup();
    resize();
    redraw();
  }

  /** The setup changed: a replay shown stays, said to be of the earlier setup. */
  function changed() {
    if (!visible) return;
    showSetup();
    if (shownReplay && !shownReplay.stale) {
      shownReplay.stale = true;
      status(() => t("layout.changed"));
    }
  }

  function relabel() {
    for (const option of speed.options) option.textContent = t(`layout.speed.${option.value}`);
    $("layout-legend").replaceChildren(...LEGEND.map(([shape, colour, label]) => legendItem(shape, colour, t(label))));
    showButtons();
    status(statusText);
    if (visible) showSetup();
    if (shownReplay) {
      showClock();
      showStats();
    }
  }

  /** The setup's dataset and strategy, and whether it can be replayed. */
  function showSetup() {
    const snapshot = setup.snapshot();
    const layout = snapshot?.entry.info.layout;
    $("layout-record").disabled = !wasmReady || !layout || Boolean(recording);
    $("layout-setup").textContent = snapshot ? t("layout.setup", { dataset: snapshot.dataset, strategy: snapshot.strategy }) : "";
    if (!layout && !recording && !shownReplay) status(() => t("layout.empty"));
    else if (statusText() === t("layout.empty")) status(() => "");
  }

  /** The window the inputs choose: from the start of a day, for a length (ms). */
  function chosenWindow() {
    const day = Math.max(1, Math.round(Number($("layout-day").value)));
    const from = (day - 1) * DAY;
    return { from, until: from + Number($("layout-length").value) };
  }

  /** Replays the window of the setup: as a finished run of it recorded it, or simulated up to
   * the window's end. */
  function record() {
    const snapshot = setup.snapshot();
    if (!snapshot?.entry.info.layout || recording) return;
    pause();
    const { from, until } = chosenWindow();
    run += 1;
    recording = { snapshot, day: from / DAY + 1, until, length: until - from };
    const { entry } = snapshot;
    // Replication 0, with the flow factors a finished run of it measured if QTS needs them.
    const config = { ...snapshot.config, replication: 0 };
    const kept = finishedReplay(entry.id, config, snapshot.code, { from, until });
    if (kept) {
      recorded(kept);
      return;
    }
    const measured = flowFactors(entry.id, config);
    worker.postMessage({
      run,
      key: entry.id,
      bytes: sent === entry.id ? undefined : entry.bytes,
      config: measured ? { ...config, flow_factors: measured } : config,
      code: snapshot.code,
      window: { from, until },
    });
    sent = entry.id;
    $("layout-stop").hidden = false;
    showSetup();
    status(() => t("layout.starting"));
  }

  /** Where the recording is: a QTS pre-run first measures the flow factors over the whole run
   * (no finished run of the setup gave them), then the run goes up to the window's end. */
  function progress(update) {
    if (update.pass < update.passes - 1) {
      status(() => t("layout.preRun", { day: dayText(update) }));
      return;
    }
    const days = Math.ceil(recording.until / DAY);
    const day = Math.min(days, Math.floor(update.now / DAY) + 1);
    status(() => t("layout.recording", { day: formatNumber(day, 0), days: formatNumber(days, 0) }));
  }

  /** The recording is done: its replay is shown from its start, paused; none if the run ended
   * before the window. */
  function recorded(replay) {
    const { snapshot, day, length } = recording;
    finishRecording();
    if (!replay) {
      status(() => t("layout.afterEnd"));
      return;
    }
    let player;
    try {
      player = new ReplayPlayer(snapshot.entry.dataset, replay);
    } catch (error) {
      status(() => t("layout.failed", { message: error instanceof Error ? error.message : String(error) }));
      return;
    }
    const first = shownReplay?.snapshot.entry !== snapshot.entry;
    const drawing = first ? snapshot.entry.dataset.layout() : shownReplay.drawing;
    shownReplay?.player.free();
    const { from, until } = player.window();
    shownReplay = { snapshot, drawing, player, from, until, frame: null };
    time = from;
    seek.min = String(from);
    seek.max = String(until);
    seek.step = "1000";
    seek.value = String(time);
    $("layout-body").hidden = false;
    status(() =>
      t("layout.ready", {
        day: formatNumber(day, 0),
        length: lengthText(length),
        size: formatNumber(replay.byteLength / 1e6, 2),
      }),
    );
    showButtons();
    resize();
    if (first) fit();
    redraw();
  }

  /** Stops the recording under way: its worker ends, another takes its place. */
  function stop() {
    if (!recording) return;
    worker.terminate();
    worker = recorder();
    sent = null;
    finishRecording();
    status(() => t("layout.stopped"));
  }

  function finishRecording() {
    recording = null;
    $("layout-stop").hidden = true;
    showSetup();
  }

  function play() {
    if (!shownReplay) return;
    if (time >= shownReplay.until) time = shownReplay.from;
    playing = true;
    lastWall = null;
    showButtons();
    requestAnimationFrame(frame);
  }

  function pause() {
    playing = false;
    showButtons();
  }

  /** An animation frame while playing: the replay moves on by the wall time since the last. */
  function frame(wall) {
    if (!playing || !visible) return;
    const step = lastWall === null ? 0 : Math.min(wall - lastWall, MAX_STEP);
    lastWall = wall;
    time = Math.min(time + step * Number(speed.value), shownReplay.until);
    seek.value = String(time);
    draw();
    if (time >= shownReplay.until) pause();
    else requestAnimationFrame(frame);
  }

  function showButtons() {
    const button = $("layout-play");
    button.disabled = !shownReplay;
    button.textContent = t(playing ? "layout.pause" : "layout.play");
  }

  function status(render) {
    statusText = render;
    $("layout-status").textContent = render();
  }

  // ---- drawing ----

  /** The page's colours, and its monospaced font for the floor's labels. */
  function readColours() {
    const style = getComputedStyle(document.documentElement);
    const read = Object.fromEntries(TOKENS.map((name) => [name, style.getPropertyValue(`--${name}`).trim()]));
    return { ...read, font: `${LABEL}px ${style.getPropertyValue("--mono").trim()}` };
  }

  function resize() {
    const box = $("layout-stage").getBoundingClientRect();
    const ratio = devicePixelRatio || 1;
    size = { width: box.width, height: box.height, ratio };
    for (const each of [canvas, background]) {
      each.width = Math.max(1, Math.round(box.width * ratio));
      each.height = Math.max(1, Math.round(box.height * ratio));
    }
  }

  /** Shows the whole floor. */
  function fit() {
    if (!shownReplay || size.width === 0) return;
    const [x0, y0, x1, y1] = shownReplay.drawing.bounds;
    view.x = (x0 + x1) / 2;
    view.y = (y0 + y1) / 2;
    view.scale = 0.95 * Math.min(size.width / (x1 - x0), size.height / (y1 - y0));
  }

  /** Zooms by `factor` keeping the point at (`x`, `y`) on the canvas in place. */
  function zoom(factor, x, y) {
    if (!shownReplay) return;
    const [wx, wy] = world(x, y);
    view.scale = Math.min(Math.max(view.scale * factor, 0.0005), 0.5);
    view.x = wx - (x - size.width / 2) / view.scale;
    view.y = wy + (y - size.height / 2) / view.scale;
    redraw();
  }

  const screenX = (x) => (x - view.x) * view.scale + size.width / 2;
  const screenY = (y) => size.height / 2 - (y - view.y) * view.scale;

  function world(x, y) {
    return [view.x + (x - size.width / 2) / view.scale, view.y - (y - size.height / 2) / view.scale];
  }

  function offset(event) {
    const box = canvas.getBoundingClientRect();
    return [event.clientX - box.left, event.clientY - box.top];
  }

  /** The background again, then the frame. */
  function redraw() {
    if (!shownReplay || !visible || size.width === 0) return;
    drawBackground();
    draw();
  }

  function drawBackground() {
    const drawing = shownReplay.drawing;
    const c = background.getContext("2d");
    c.setTransform(size.ratio, 0, 0, size.ratio, 0, 0);
    c.fillStyle = colours.card;
    c.fillRect(0, 0, size.width, size.height);
    // Intrabays as panels with their names.
    c.fillStyle = colours.surface;
    for (const bay of drawing.bays) {
      if (bay.interbay) continue;
      const [x0, y0, x1, y1] = bay.bounds;
      c.fillRect(screenX(x0 - BAY_MARGIN), screenY(y1 + BAY_MARGIN), (x1 - x0 + 2 * BAY_MARGIN) * view.scale, (y1 - y0 + 2 * BAY_MARGIN) * view.scale);
    }
    if (view.scale > NAMES_FROM) {
      c.fillStyle = colours.muted;
      c.font = colours.font;
      for (const bay of drawing.bays) {
        if (bay.interbay) continue;
        c.fillText(bay.name, screenX(bay.bounds[0] - BAY_MARGIN) + 3, screenY(bay.bounds[3] + BAY_MARGIN) + LABEL + 1);
      }
    }
    // Rails, curves on their arcs.
    c.strokeStyle = colours["line-strong"];
    c.lineWidth = 1;
    c.beginPath();
    for (const rail of drawing.rails) {
      const [x, y] = drawing.nodes[rail.from];
      c.moveTo(screenX(x), screenY(y));
      if (rail.arc) {
        const { cx, cy, radius, start, sweep } = rail.arc;
        c.arc(screenX(cx), screenY(cy), radius * view.scale, -start, -(start + sweep), sweep > 0);
      } else {
        const [tx, ty] = drawing.nodes[rail.to];
        c.lineTo(screenX(tx), screenY(ty));
      }
    }
    c.stroke();
    // Footprints: tools outlined, commit and complete stations filled, stockers dashed.
    c.beginPath();
    for (const tool of drawing.tools) if (tool) rectPath(c, tool);
    c.stroke();
    c.fillStyle = colours.text;
    c.beginPath();
    for (const station of [...drawing.commits, ...drawing.completes]) rectPath(c, station);
    c.fill();
    c.setLineDash([3, 3]);
    c.beginPath();
    for (const stocker of drawing.stockers) rectPath(c, stocker);
    c.stroke();
    c.setLineDash([]);
    if (view.scale >= BUFFERS_FROM) {
      c.fillStyle = colours.line;
      const half = Math.max(1, (FOUP / 2) * view.scale);
      for (const port of drawing.ports) {
        if (port.role === "buffer") c.fillRect(screenX(port.x) - half, screenY(port.y) - half, 2 * half, 2 * half);
      }
    }
  }

  /** The frame at the replay's time on the background: tool states, FOUPs and vehicles. */
  function draw() {
    if (!shownReplay || !visible || size.width === 0) return;
    context.setTransform(1, 0, 0, 1, 0, 0);
    context.drawImage(background, 0, 0);
    context.setTransform(size.ratio, 0, 0, size.ratio, 0, 0);
    const { drawing, player } = shownReplay;
    const frame = player.frame(time);
    shownReplay.frame = frame;
    showClock();
    // Tools in their state's colour.
    const byColour = new Map();
    frame.tools.forEach((state, index) => {
      const tool = drawing.tools[index];
      const colour = STATE_COLOUR[state];
      if (!tool || !colour) return;
      if (!byColour.has(colour)) byColour.set(colour, []);
      byColour.get(colour).push(tool);
    });
    for (const [colour, tools] of byColour) {
      context.fillStyle = colours[colour];
      context.beginPath();
      for (const tool of tools) rectPath(context, tool);
      context.fill();
    }
    // Vehicles, then FOUPs: at ports, and on the vehicles carrying them.
    const { vehicles, foups } = frame;
    drawVehicles(drawing, vehicles);
    const half = Math.max(1.2, (FOUP / 2) * view.scale);
    const committed = new Map();
    for (const hot of [false, true]) {
      context.fillStyle = hot ? colours.warning : colours["text-2"];
      context.beginPath();
      foups.lot.forEach((_, row) => {
        if (HOT.has(foups.kind[row]) !== hot) return;
        const [place, index] = [foups.place[row], foups.index[row]];
        let at = null;
        if (place === "port") at = drawing.ports[index];
        else if (place === "vehicle") at = middle(vehicles, index);
        else if (place === "commit" && !hot) committed.set(index, (committed.get(index) ?? 0) + 1);
        if (at) context.rect(screenX(at.x) - half, screenY(at.y) - half, 2 * half, 2 * half);
      });
      context.fill();
    }
    // FOUPs in commit stations, counted.
    context.fillStyle = colours.text;
    context.font = colours.font;
    for (const [port, count] of committed) {
      const at = drawing.ports[port];
      context.fillText(String(count), screenX(at.x) + 4, screenY(at.y) - 4);
    }
    showStats();
  }

  /** Vehicle bodies from their fronts to their rears on the rails (a straight body on a curve
   * spans its chord), or dots at their middles when small. */
  function drawVehicles(drawing, vehicles) {
    const width = Math.max(1.5, VEHICLE_WIDTH * view.scale);
    for (const [activity, colour] of Object.entries(ACTIVITY_COLOUR)) {
      context.fillStyle = colours[colour];
      context.beginPath();
      vehicles.activity.forEach((each, id) => {
        if (each !== activity) return;
        if (drawing.vehicle_lengths[id] * view.scale < DOT_BELOW) {
          const at = middle(vehicles, id);
          const [x, y] = [screenX(at.x), screenY(at.y)];
          context.moveTo(x + 2.5, y);
          context.arc(x, y, 2.5, 0, 2 * Math.PI);
          return;
        }
        const [x, y] = [screenX(vehicles.x[id]), screenY(vehicles.y[id])];
        const [tx, ty] = [screenX(vehicles.tail_x[id]), screenY(vehicles.tail_y[id])];
        // Half the width across the body, from rear to front.
        const scale = width / 2 / Math.hypot(x - tx, y - ty);
        const [nx, ny] = [(ty - y) * scale, (x - tx) * scale];
        context.moveTo(x + nx, y + ny);
        context.lineTo(x - nx, y - ny);
        context.lineTo(tx - nx, ty - ny);
        context.lineTo(tx + nx, ty + ny);
        context.closePath();
      });
      context.fill();
    }
  }

  function rectPath(c, station) {
    const w = station.width * view.scale;
    const h = station.height * view.scale;
    c.rect(screenX(station.x) - w / 2, screenY(station.y) - h / 2, w, h);
  }

  function showClock() {
    const day = Math.floor(time / DAY) + 1;
    const seconds = Math.floor((time % DAY) / 1000);
    const pad = (value) => String(value).padStart(2, "0");
    const clock = `${pad(Math.floor(seconds / 3600))}:${pad(Math.floor(seconds / 60) % 60)}:${pad(seconds % 60)}`;
    $("layout-clock").textContent = t("layout.clock", { day: formatNumber(day, 0), time: clock });
  }

  // ---- numbers and details ----

  function showStats() {
    const frame = shownReplay?.frame;
    if (!frame) return;
    const { drawing } = shownReplay;
    const { vehicles, foups } = frame;
    const counts = { tool: 0, buffer: 0, vehicle: 0, inside: 0, commit: 0 };
    foups.place.forEach((place, row) => {
      if (place === "port") counts[drawing.ports[foups.index[row]].role === "tool" ? "tool" : "buffer"] += 1;
      else if (place === "vehicle") counts.vehicle += 1;
      else if (place === "tool") counts.inside += 1;
      else if (place === "commit") counts.commit += 1;
    });
    const busy = vehicles.activity.filter((activity) => activity !== "idle").length;
    const delivered = frame.delivered;
    const rows = [
      ["layout.stat.wip", formatNumber(foups.lot.length, 0)],
      ["layout.stat.busy", t("layout.stat.busyValue", { busy: formatNumber(busy, 0), total: formatNumber(vehicles.x.length, 0) })],
      ["layout.stat.foups", formatNumber(counts.tool, 0)],
      ["layout.stat.buffered", formatNumber(counts.buffer, 0)],
      ["layout.stat.carried", formatNumber(counts.vehicle, 0)],
      ["layout.stat.inside", formatNumber(counts.inside, 0)],
      ["layout.stat.committed", formatNumber(counts.commit, 0)],
      ["layout.stat.deliveries", formatNumber(delivered, 0)],
      ["layout.stat.t2t", delivered ? t("kpi.suffix.percent", { value: formatNumber((100 * frame.tool_to_tool) / delivered, 1) }) : "–"],
      ["layout.stat.carry", delivered ? t("kpi.suffix.seconds", { value: formatNumber(frame.carried / delivered / 1000, 1) }) : "–"],
    ];
    $("layout-stats").replaceChildren(
      ...rows.map(([label, value]) => {
        const pair = document.createElement("div");
        pair.append(element("dt", t(label)), element("dd", value));
        return pair;
      }),
    );
  }

  /** The vehicle or tool under the pointer, described. */
  function showTip(event) {
    if (!shownReplay?.frame) return;
    const [x, y] = offset(event);
    const text = vehicleAt(x, y) ?? toolAt(x, y);
    const tip = $("layout-tip");
    if (!text) {
      tip.hidden = true;
      return;
    }
    tip.textContent = text;
    tip.hidden = false;
    tip.style.left = `${Math.min(x + 14, size.width - tip.offsetWidth - 4)}px`;
    tip.style.top = `${Math.max(4, y - tip.offsetHeight - 10)}px`;
  }

  function hideTip() {
    $("layout-tip").hidden = true;
  }

  function vehicleAt(x, y) {
    const { vehicles, foups } = shownReplay.frame;
    let best = null;
    let nearest = 8;
    vehicles.x.forEach((_, id) => {
      const at = middle(vehicles, id);
      const distance = Math.hypot(screenX(at.x) - x, screenY(at.y) - y);
      if (distance < nearest) {
        nearest = distance;
        best = id;
      }
    });
    if (best === null) return null;
    const lines = [
      t("layout.tip.vehicle", {
        id: best,
        activity: t(`activity.${vehicles.activity[best]}`),
        speed: formatNumber(vehicles.speed[best], 2),
      }),
    ];
    foups.lot.forEach((lot, row) => {
      if (foups.place[row] === "vehicle" && foups.index[row] === best) lines.push(t("layout.tip.lot", { lot }));
    });
    return lines.join("\n");
  }

  function toolAt(x, y) {
    const [wx, wy] = world(x, y);
    const { drawing, frame } = shownReplay;
    const index = drawing.tools.findIndex(
      (tool) => tool && Math.abs(wx - tool.x) <= tool.width / 2 && Math.abs(wy - tool.y) <= tool.height / 2,
    );
    if (index < 0) return null;
    const lines = [t("layout.tip.tool", { name: drawing.tools[index].name, state: t(`toolState.${frame.tools[index]}`) })];
    const { foups } = frame;
    const inside = foups.place.filter((place, row) => place === "tool" && foups.index[row] === index).length;
    if (inside) lines.push(t("layout.tip.inside", { foups: inside }));
    return lines.join("\n");
  }

  return { ready, shown, changed, relabel, window: chosenWindow };
}

/** The middle of vehicle `id`'s body (mm), between its front and its rear: where it carries a
 * FOUP. */
function middle(vehicles, id) {
  return { x: (vehicles.x[id] + vehicles.tail_x[id]) / 2, y: (vehicles.y[id] + vehicles.tail_y[id]) / 2 };
}

/** A window's length (ms) in words. */
function lengthText(length) {
  return t(`layout.length.${Math.round(length / 60_000)}`);
}

/** A legend entry: a swatch of the shape in a colour token, and its text. */
function legendItem(shape, colour, text) {
  const swatch = element("span", "");
  swatch.className = `swatch swatch-${shape}`;
  swatch.style.setProperty("--swatch", `var(--${colour})`);
  const item = document.createElement("li");
  item.append(swatch, element("span", text));
  return item;
}

function element(tag, text) {
  const node = document.createElement(tag);
  node.textContent = text;
  return node;
}
