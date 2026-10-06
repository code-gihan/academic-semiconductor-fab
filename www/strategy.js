// Strategy editor: the queue-time rule, the lot ranking per tool group, early batch starts,
// stopping limits, the engineering rule and super hot reservation of a configuration in the
// core's schema, for the tool groups of a dataset (its info). Edits change the configuration in
// place and are reported through `changed(config)`; the core checks it before a run.
import { formatNumber, t } from "./i18n.js";
import { infoButton, withTooltip } from "./tooltip.js";

const HOUR = 3_600_000;
/** Ranking criteria in the order offered; {qt_within: ms} is the one with a value. */
const CRITERIA = [
  "qt_within",
  "qtcr",
  "qts",
  "qt_deadline",
  "priority",
  "least_setup",
  "fifo",
  "critical_ratio",
  "due_date",
  "shortest_step",
  "least_remaining",
];
/** Most criteria of a ranking (the core's MAX_CRITERIA). */
const MAX_CRITERIA = 6;
/** [P2] Table 3: stepper limits [front, total]. */
const STOPPING = {
  high: { LithoTrack_FE_95: [90, 130], LithoTrack_FE_115: [90, 220] },
  medium: { LithoTrack_FE_95: [60, 95], LithoTrack_FE_115: [70, 150] },
  small: { LithoTrack_FE_95: [50, 85], LithoTrack_FE_115: [55, 125] },
};
/** [P1] §V: CAtE (production, engineering) hours per dataset, CoT triggers. */
const CATE = {
  ds3: [
    [151.2, 16.8],
    [75.6, 8.4],
    [21.6, 2.4],
  ],
  ds4: [
    [134.6, 33.4],
    [67.2, 16.8],
    [19.2, 4.8],
  ],
};
const COT = [100, 50, 25, 10];

/** The kind of a criterion: its name, or the key of its object. */
export function criterionKind(criterion) {
  return typeof criterion === "string" ? criterion : Object.keys(criterion)[0];
}

/** A criterion in words. */
export function criterionText(criterion) {
  const kind = criterionKind(criterion);
  return kind === "qt_within"
    ? t("criterion.qt_within", { hours: hours(criterion.qt_within) })
    : t(`criterion.${kind}`);
}

/** Criteria in words, most significant first. */
export function rankingText(criteria) {
  return criteria.map(criterionText).join(" › ");
}

function hours(ms) {
  return formatNumber(ms / HOUR, Number.isInteger(ms / HOUR) ? 0 : 1);
}

/** Ms of a positive number of hours typed into `input`, or null. */
function hoursOf(input) {
  const value = Number(input.value);
  return input.value !== "" && value > 0 ? Math.round(value * HOUR) : null;
}

/** A positive whole number typed into `input`, or null. */
function countOf(input) {
  const value = Number(input.value);
  return input.value !== "" && Number.isInteger(value) && value > 0 ? value : null;
}

/** The editor in `root`: `show(info, dataset, config)` edits `config` for that dataset;
 * `render()` draws it again (a new language). */
export function strategyEditor(root, changed) {
  let info = null;
  let dataset = "";
  let config = null;
  /** The ranking the builder applies. */
  let draft = [{ qt_within: 2 * HOUR }, "priority", "fifo"];
  /** Tool groups chosen in the ranking table, by index. */
  const checked = new Set();
  const filter = { cqt: true, area: "", text: "" };
  /** Per tool group: CQT segments it serves (after their entrance step). */
  let segments = [];
  /** Parts drawn again on their own. */
  let builder;
  let groupTable;
  let stoppingTable;

  function show(newInfo, newDataset, newConfig) {
    info = newInfo;
    dataset = newDataset;
    config = newConfig;
    checked.clear();
    segments = info.tool_groups.map(() => 0);
    for (const segment of info.segments) {
      for (const group of segment.tool_groups) segments[group] += 1;
    }
    render();
  }

  function render() {
    if (!info) return;
    root.replaceChildren(
      queueTimeCard(),
      rankingCard(),
      batchCard(),
      stoppingCard(),
      engineeringCard(),
      superHotCard(),
    );
  }

  function edit() {
    changed(config);
  }

  // ---- queue-time rule ----

  function queueTimeCard() {
    const select = el("select", { "aria-label": t("strategy.queueTime.label") });
    for (const rule of ["none", "qtcr", "qts"]) {
      select.append(new Option(t(`strategy.queueTime.${rule}`), rule, false, rule === queueTimeRule()));
    }
    select.addEventListener("change", () => {
      config.queue_time = select.value;
      edit();
      drawGroups();
    });
    return card("strategy.queueTime.label", "strategy.queueTime.short", "strategy.queueTime.hint", "[P2]", select);
  }

  function queueTimeRule() {
    return config.queue_time ?? "none";
  }

  // ---- ranking per tool group ----

  function rankingCard() {
    builder = el("div", { class: "builder" });
    drawBuilder();
    const only = el("input", { type: "checkbox" });
    only.checked = filter.cqt;
    only.addEventListener("change", () => {
      filter.cqt = only.checked;
      drawGroups();
    });
    const area = el("select");
    area.append(new Option(t("ranking.allAreas"), ""));
    info.areas.forEach((name, index) => area.append(new Option(name, String(index))));
    area.value = filter.area;
    area.addEventListener("change", () => {
      filter.area = area.value;
      drawGroups();
    });
    const search = el("input", { type: "search", placeholder: t("ranking.search") });
    search.value = filter.text;
    search.addEventListener("input", () => {
      filter.text = search.value.trim().toLowerCase();
      drawGroups();
    });
    const filters = el(
      "div",
      { class: "row filters" },
      el("label", { class: "check" }, only, t("ranking.onlyCqt")),
      labelled(area, "ranking.area"),
      search,
    );
    groupTable = el("div");
    drawGroups();
    const help = el(
      "details",
      { class: "help" },
      el("summary", {}, t("ranking.help")),
      el(
        "dl",
        { class: "explanations" },
        ...CRITERIA.flatMap((kind) => [
          el("dt", {}, criterionText(kind === "qt_within" ? { qt_within: 2 * HOUR } : kind)),
          el("dd", {}, t(`criterion.${kind}.hint`)),
        ]),
      ),
    );
    return card("ranking.heading", "ranking.short", "ranking.hint", null, builder, help, filters, groupTable);
  }

  function drawBuilder() {
    const chips = el("ol", { class: "chips-edit" });
    draft.forEach((criterion, index) => {
      const kind = criterionKind(criterion);
      const item = el("li", { class: "chip" });
      withTooltip(item, () => t(`criterion.${kind}.hint`));
      if (kind === "qt_within") {
        const input = el("input", { type: "number", min: "0.1", step: "any", class: "short" });
        input.value = String(criterion.qt_within / HOUR);
        input.setAttribute("aria-label", t("ranking.hours"));
        input.addEventListener("change", () => {
          const ms = hoursOf(input);
          if (ms) draft[index] = { qt_within: ms };
          drawBuilder();
        });
        item.append(el("span", {}, t("criterion.qt_within.prefix")), input, el("span", {}, t("criterion.qt_within.suffix")));
      } else {
        item.append(el("span", {}, criterionText(criterion)));
      }
      const move = (offset, label, text) => {
        const button = el("button", { type: "button", class: "icon", "aria-label": t(label) }, text);
        button.disabled = !draft[index + offset];
        button.addEventListener("click", () => {
          [draft[index], draft[index + offset]] = [draft[index + offset], draft[index]];
          drawBuilder();
        });
        return button;
      };
      const remove = el("button", { type: "button", class: "icon", "aria-label": t("ranking.remove") }, "×");
      remove.addEventListener("click", () => {
        draft.splice(index, 1);
        drawBuilder();
      });
      item.append(move(-1, "ranking.earlier", "‹"), move(1, "ranking.later", "›"), remove);
      chips.append(item);
    });
    const used = new Set(draft.map(criterionKind));
    const add = el("select", { "aria-label": t("ranking.add") });
    add.append(new Option(t("ranking.add"), ""));
    for (const kind of CRITERIA.filter((kind) => !used.has(kind))) {
      add.append(new Option(criterionText(kind === "qt_within" ? { qt_within: 2 * HOUR } : kind), kind));
    }
    add.disabled = draft.length >= MAX_CRITERIA;
    add.addEventListener("change", () => {
      draft.push(add.value === "qt_within" ? { qt_within: 2 * HOUR } : add.value);
      drawBuilder();
    });
    const apply = el("button", { type: "button", class: "primary" }, t("ranking.apply", { count: checked.size }));
    apply.disabled = checked.size === 0 || draft.length === 0;
    apply.addEventListener("click", () => {
      for (const group of checked) {
        config.ranking[info.tool_groups[group].name] = structuredClone(draft);
      }
      edit();
      drawGroups();
    });
    const inherit = el("button", { type: "button" }, t("ranking.inherit"));
    inherit.disabled = checked.size === 0;
    inherit.addEventListener("click", () => {
      for (const group of checked) delete config.ranking[info.tool_groups[group].name];
      edit();
      drawGroups();
    });
    builder.replaceChildren(
      el("p", { class: "label" }, t("ranking.builder")),
      draft.length > 0 ? chips : el("p", { class: "hint" }, t("ranking.empty")),
      el("div", { class: "row" }, add, apply, inherit),
    );
  }

  function shownGroups() {
    return info.tool_groups
      .map((group, index) => ({ group, index }))
      .filter(
        ({ group, index }) =>
          (!filter.cqt || segments[index] > 0) &&
          (filter.area === "" || group.area === Number(filter.area)) &&
          group.name.toLowerCase().includes(filter.text),
      );
  }

  function drawGroups() {
    const rows = shownGroups();
    const all = el("input", { type: "checkbox", "aria-label": t("ranking.selectAll") });
    all.checked = rows.length > 0 && rows.every(({ index }) => checked.has(index));
    all.addEventListener("change", () => {
      for (const { index } of rows) {
        if (all.checked) checked.add(index);
        else checked.delete(index);
      }
      drawGroups();
      drawBuilder();
    });
    const head = el(
      "tr",
      {},
      el("th", { scope: "col" }, all),
      el("th", { scope: "col" }, t("ranking.col.group")),
      el("th", { scope: "col" }, t("ranking.col.area")),
      el("th", { scope: "col", class: "number" }, t("ranking.col.tools")),
      el("th", { scope: "col", class: "number" }, t("ranking.col.segments")),
      el("th", { scope: "col" }, t("ranking.col.ranking")),
    );
    const body = el("tbody");
    for (const { group, index } of rows) {
      const box = el("input", { type: "checkbox", "aria-label": group.name });
      box.checked = checked.has(index);
      box.addEventListener("change", () => {
        if (box.checked) checked.add(index);
        else checked.delete(index);
        all.checked = rows.every((row) => checked.has(row.index));
        drawBuilder();
      });
      const own = config.ranking[group.name];
      const rule = queueTimeRule();
      const text = own
        ? rankingText(own)
        : `${rankingText(group.ranks)}${rule === "none" ? "" : ` · ${t("ranking.withRule", { rule: t(`rule.${rule}`) })}`}`;
      const load = el("button", { type: "button", class: own ? "link own" : "link" }, text);
      load.title = t("ranking.load");
      load.addEventListener("click", () => {
        draft = structuredClone(own ?? group.ranks);
        drawBuilder();
      });
      const tags = [
        group.batching && t("tag.batch"),
        group.setup_runs && t("tag.setupRuns"),
        group.stepper && t("tag.stepper"),
      ].filter(Boolean);
      body.append(
        el(
          "tr",
          {},
          el("td", {}, box),
          el("th", { scope: "row" }, group.name, ...tags.map((tag) => el("span", { class: "tag" }, tag))),
          el("td", {}, info.areas[group.area]),
          el("td", { class: "number" }, String(group.tools)),
          el("td", { class: "number" }, segments[index] > 0 ? String(segments[index]) : "–"),
          el("td", { class: "ranking" }, load),
        ),
      );
    }
    const custom = Object.keys(config.ranking).length;
    groupTable.replaceChildren(
      el("p", { class: "hint" }, t("ranking.summary", { shown: rows.length, custom })),
      el("div", { class: "scroll" }, el("table", { class: "groups" }, el("thead", {}, head), body)),
    );
  }

  // ---- early batch starts ----

  function batchCard() {
    const on = el("input", { type: "checkbox" });
    on.checked = config.batch_start_within != null;
    const input = el("input", { type: "number", min: "0.1", step: "any", class: "short" });
    input.value = String((config.batch_start_within ?? 2 * HOUR) / HOUR);
    input.disabled = !on.checked;
    const update = () => {
      input.disabled = !on.checked;
      const ms = hoursOf(input);
      if (on.checked && ms) {
        config.batch_start_within = ms;
      } else {
        delete config.batch_start_within;
      }
      edit();
    };
    on.addEventListener("change", update);
    input.addEventListener("change", update);
    const batches = info.tool_groups.filter((group) => group.batching).map((group) => group.name);
    return card(
      "batch.heading",
      "batch.short",
      "batch.hint",
      null,
      el("label", { class: "check" }, on, t("batch.label"), input, t("batch.unit")),
      el("p", { class: "hint" }, t("batch.groups", { groups: batches.join(", ") })),
    );
  }

  // ---- stopping ----

  function stoppingCard() {
    const on = el("input", { type: "checkbox" });
    on.checked = Boolean(config.stopping);
    on.addEventListener("change", () => {
      if (on.checked) config.stopping = { limits: {} };
      else delete config.stopping;
      edit();
      drawStopping();
    });
    const presets = el("div", { class: "row" });
    for (const name of Object.keys(STOPPING)) {
      const button = el("button", { type: "button" }, t(`strategy.stopping.${name}`));
      button.addEventListener("click", () => {
        config.stopping ??= { limits: {} };
        for (const [group, [front, total]] of Object.entries(STOPPING[name])) {
          if (info.tool_groups.some((spec) => spec.name === group)) {
            config.stopping.limits[group] = { front, total };
          }
        }
        on.checked = true;
        edit();
        drawStopping();
      });
      presets.append(button);
    }
    stoppingTable = el("div");
    drawStopping();
    const enable = el("label", { class: "check" }, on, t("stopping.enable"));
    return card("strategy.stopping.label", "stopping.short", "stopping.hint", "[P2]", enable, presets, stoppingTable);
  }

  function drawStopping() {
    if (!config.stopping) {
      stoppingTable.replaceChildren();
      return;
    }
    const limits = config.stopping.limits;
    const pair = (current, write) => {
      const input = (key) => {
        const field = el("input", { type: "number", min: "1", step: "1", class: "short", placeholder: "1000" });
        field.value = current?.[key] ?? "";
        field.setAttribute("aria-label", t(`stopping.${key}`));
        field.addEventListener("change", () => {
          const front = countOf(fields.front);
          const total = countOf(fields.total);
          write(front && total ? { front, total } : null);
          edit();
        });
        return field;
      };
      const fields = { front: input("front"), total: input("total") };
      return [fields.front, fields.total];
    };
    const head = el(
      "tr",
      {},
      el("th", { scope: "col" }, t("ranking.col.group")),
      el("th", { scope: "col" }, t("ranking.col.area")),
      el("th", { scope: "col", class: "number" }, t("ranking.col.segments")),
      el("th", { scope: "col", class: "number" }, t("stopping.front")),
      el("th", { scope: "col", class: "number" }, t("stopping.total")),
    );
    const body = el("tbody");
    const defaults = pair(config.stopping.default, (value) => {
      if (value) config.stopping.default = value;
      else delete config.stopping.default;
    });
    body.append(
      el(
        "tr",
        { class: "default" },
        el("th", { scope: "row" }, t("stopping.default")),
        el("td"),
        el("td"),
        ...defaults.map((field) => el("td", { class: "number" }, field)),
      ),
    );
    info.tool_groups.forEach((group, index) => {
      if (segments[index] === 0) return;
      const fields = pair(limits[group.name], (value) => {
        if (value) limits[group.name] = value;
        else delete limits[group.name];
      });
      body.append(
        el(
          "tr",
          {},
          el("th", { scope: "row" }, group.name),
          el("td", {}, info.areas[group.area]),
          el("td", { class: "number" }, String(segments[index])),
          ...fields.map((field) => el("td", { class: "number" }, field)),
        ),
      );
    });
    stoppingTable.replaceChildren(
      el("p", { class: "hint" }, t("stopping.tableHint")),
      el("div", { class: "scroll" }, el("table", { class: "groups" }, el("thead", {}, head), body)),
    );
  }

  // ---- engineering lots ----

  function engineeringCard() {
    const engineering = info.parts.some((part) => part.engineering);
    if (!engineering) {
      return card("strategy.engineering.label", "engineering.absent", null, "[P1]");
    }
    const rule = config.engineering ?? "base";
    const kind = typeof rule === "string" ? rule : Object.keys(rule)[0];
    const select = el("select", { "aria-label": t("strategy.engineering.label") });
    for (const value of ["base", "engineering_first", "cate", "cot"]) {
      select.append(new Option(t(`engineering.${value}`), value, false, value === kind));
    }
    const details = el("div", { class: "row" });
    const draw = () => {
      details.replaceChildren();
      if (select.value === "cate") {
        const current = config.engineering.cate;
        const production = number(current.production / HOUR, "engineering.production");
        const engineeringHours = number(current.engineering / HOUR, "engineering.engineering");
        const write = () => {
          const [p, e] = [hoursOf(production), hoursOf(engineeringHours)];
          if (p && e) config.engineering = { cate: { production: p, engineering: e } };
          edit();
        };
        production.addEventListener("change", write);
        engineeringHours.addEventListener("change", write);
        const presets = el("select", { "aria-label": t("engineering.presets") });
        presets.append(new Option(t("engineering.presets"), ""));
        for (const [set, windows] of Object.entries(CATE)) {
          for (const [p, e] of windows) {
            const label = t("strategy.engineering.cateSet", {
              cycle: formatNumber(p + e, 0),
              production: formatNumber(p, 1),
              engineering: formatNumber(e, 1),
              dataset: set.toUpperCase(),
            });
            presets.append(new Option(label, `${p}:${e}`, false, false));
          }
        }
        presets.addEventListener("change", () => {
          const [p, e] = presets.value.split(":").map(Number);
          config.engineering = { cate: { production: Math.round(p * HOUR), engineering: Math.round(e * HOUR) } };
          edit();
          draw();
        });
        details.append(labelled(production, "engineering.production"), labelled(engineeringHours, "engineering.engineering"), presets);
      } else if (select.value === "cot") {
        const trigger = number(config.engineering.cot.trigger, "engineering.trigger");
        trigger.step = "1";
        trigger.addEventListener("change", () => {
          const value = countOf(trigger);
          if (value) config.engineering = { cot: { trigger: value } };
          edit();
        });
        const presets = el("select", { "aria-label": t("engineering.presets") });
        presets.append(new Option(t("engineering.presets"), ""));
        for (const value of COT) presets.append(new Option(t("strategy.engineering.cot", { trigger: value }), String(value)));
        presets.addEventListener("change", () => {
          config.engineering = { cot: { trigger: Number(presets.value) } };
          edit();
          draw();
        });
        details.append(labelled(trigger, "engineering.trigger"), presets);
      }
    };
    select.addEventListener("change", () => {
      const defaults = CATE[dataset]?.[2] ?? CATE.ds4[2];
      config.engineering =
        select.value === "cate"
          ? { cate: { production: Math.round(defaults[0] * HOUR), engineering: Math.round(defaults[1] * HOUR) } }
          : select.value === "cot"
            ? { cot: { trigger: 25 } }
            : select.value;
      edit();
      draw();
    });
    draw();
    return card("strategy.engineering.label", "strategy.engineering.short", "strategy.engineering.hint", "[P1]", select, details);
  }

  // ---- super hot reservation ----

  function superHotCard() {
    const on = el("input", { type: "checkbox" });
    on.checked = Boolean(config.reserve_super_hot);
    on.addEventListener("change", () => {
      config.reserve_super_hot = on.checked;
      edit();
    });
    const reserve = el("label", { class: "check" }, on, t("strategy.superHot.label"));
    return card("strategy.superHot.short", "strategy.superHot.line", "strategy.superHot.hint", null, reserve);
  }

  return { show, render };
}

/** A card of the editor: its title with the (i) of `tip` and its paper, a line of what it does
 * (`short`), then the content; `tip` and `paper` are left out when null. */
function card(title, short, tip, paper, ...content) {
  const heading = el("h3", {}, t(title));
  if (tip) heading.append(infoButton(() => t(tip)));
  if (paper) heading.append(el("a", { class: "paper", href: paper === "[P1]" ? "#ref-p1" : "#ref-p2" }, paper));
  return el("section", { class: "card" }, heading, el("p", { class: "hint" }, t(short)), ...content);
}

/** `control` under its label. */
function labelled(control, key) {
  return el("label", { class: "field" }, el("span", {}, t(key)), control);
}

/** A number input of `value`, labelled by `key`. */
function number(value, key) {
  const input = el("input", { type: "number", min: "0.1", step: "any", class: "short" });
  input.value = String(Math.round(value * 100) / 100);
  input.setAttribute("aria-label", t(key));
  return input;
}

/** An element with attributes and children (strings become text). */
function el(tag, attributes = {}, ...children) {
  const node = document.createElement(tag);
  for (const [name, value] of Object.entries(attributes)) node.setAttribute(name, value);
  node.append(...children);
  return node;
}
