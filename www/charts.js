// Charts of the page, drawn by Apache ECharts (www/vendor, loaded on first use) in the colours of
// the CSS tokens: horizontal bars with confidence intervals, lines with interval bands, lines that
// grow while a run goes, heatmaps and timelines. A chart follows the size of its box and the
// colour scheme, and its instance goes when its box leaves the page; all by events.
import { formatNumber, t } from "./i18n.js";

const dark = matchMedia("(prefers-color-scheme: dark)");
const still = matchMedia("(prefers-reduced-motion: reduce)");
/** Height of a bar row or a timeline lane, and the bar rows shown before a chart scrolls. */
const ROW = 24;
const ROWS = 30;
/** The library once loading has started, the charts drawn, and the groups made. */
let library = null;
const live = new Set();
let groups = 0;

dark.addEventListener("change", () => {
  for (const chart of live) chart.redraw();
});

/** Loads the chart library, as the first chart would: early, for charts soon to come. */
export function loadCharts() {
  if (!library) {
    library = import("./vendor/echarts.esm.min.js");
    // A failure shows in the charts that need the library, not as an early unhandled rejection.
    library.catch(() => {});
  }
  return library;
}

/** A name for line charts that point at and zoom to the same x together ([lineChart]). */
export function chartGroup() {
  return `charts-${groups++}`;
}

/**
 * Horizontal bars on a common scale from 0 to `max` (default: a round number past the longest
 * bar and whisker), a row per item: `rows` [{label, values (one per part, stacked), ci (95%
 * half-width of the sum, a whisker), text (beside the bar), title (tooltip lines), selected}];
 * `parts` [{kind, label}] (the kind picks the colour; a legend if several); `onSelect(index)` for a
 * click on a row; `animate` plays the entrance. Past 30 rows the chart scrolls.
 */
export function barChart({ rows, parts, max: given, label, onSelect, animate }) {
  const max = given ?? round(Math.max(...rows.map((row) => sum(row.values) + (row.ci ?? 0))));
  const legend = parts.length > 1;
  const chosen = rows.findIndex((row) => row.selected);
  const height = Math.min(rows.length, ROWS) * ROW + 30 + (legend ? 30 : 0);
  const scrolls = rows.length > ROWS;
  const textWidth = Math.max(...rows.map((row) => textSize(row.text))) + 14;
  return chart(
    height,
    (colours, width) => ({
      ...base(colours, label),
      grid: { left: 4, right: textWidth + (scrolls ? 26 : 4), top: 4, bottom: legend ? 54 : 24 },
      legend: legend ? legendOf(colours, parts.map((part) => part.label), { bottom: 0 }) : undefined,
      tooltip: {
        ...tooltipOf(colours),
        trigger: "axis",
        axisPointer: { type: "shadow", shadowStyle: { color: colours.accentSoft, opacity: 0.6 } },
        formatter: (items) => html(rows[items[0].dataIndex].title),
      },
      xAxis: {
        ...valueAxis(colours),
        min: 0,
        max,
        axisLabel: { color: colours.muted, formatter: (value) => formatNumber(value, max < 5 ? 1 : 0) },
      },
      yAxis: {
        ...categoryAxis(colours, rows.map((row) => row.label), labelWidth(width)),
        triggerEvent: Boolean(onSelect),
        // The chosen row's label in bold.
        axisLabel: {
          color: colours.text,
          width: labelWidth(width),
          overflow: "truncate",
          formatter: (value, index) => (index === chosen ? `{chosen|${value}}` : value),
          rich: { chosen: { fontWeight: "bold", color: colours.accent } },
        },
      },
      dataZoom: scrolls
        ? [
            { ...sliderOf(colours), yAxisIndex: 0, orient: "vertical", right: 2, width: 14, top: 4, bottom: legend ? 54 : 24, startValue: 0, endValue: ROWS - 1, zoomLock: true, showDetail: false, brushSelect: false },
            { type: "inside", yAxisIndex: 0, zoomOnMouseWheel: false, moveOnMouseWheel: true, moveOnMouseMove: false },
          ]
        : undefined,
      series: [
        ...parts.map((part, index) => ({
          type: "bar",
          name: part.label,
          stack: "bars",
          barMaxWidth: 14,
          // Stacked parts touch: a hairline of the surface between them.
          itemStyle: { color: colours.kind(part.kind), borderColor: colours.card, borderWidth: parts.length > 1 ? 1 : 0 },
          data: rows.map((row) => row.values[index]),
        })),
        // The chosen row's band behind its bars.
        {
          type: "custom",
          silent: true,
          z: 0,
          encode: { y: 0 },
          data: chosen >= 0 ? [[chosen]] : [],
          renderItem: (params, api) => {
            const [, middle] = api.coord([0, api.value(0)]);
            const band = api.size([0, 1])[1];
            const area = params.coordSys;
            return {
              type: "rect",
              shape: { x: area.x, y: middle - band / 2, width: area.width, height: band },
              style: { fill: colours.accentSoft },
            };
          },
        },
        {
          type: "custom",
          silent: true,
          z: 5,
          encode: { y: 0 },
          data: rows.map((_, index) => [index]),
          renderItem: (params, api) => whisker(rows[params.dataIndex], params.dataIndex, api, colours, max),
        },
      ],
    }),
    onSelect &&
      ((instance) => {
        // A click on a row's band or on its label.
        instance.getZr().on("click", (event) => {
          const point = [event.offsetX, event.offsetY];
          if (!instance.containPixel({ gridIndex: 0 }, point)) return;
          const index = Math.round(instance.convertFromPixel({ gridIndex: 0 }, point)[1]);
          if (index >= 0 && index < rows.length) onSelect(index);
        });
        instance.on("click", "yAxis", (event) => {
          const index = rows.findIndex((row) => row.label === event.value);
          if (index >= 0) onSelect(index);
        });
      }),
    animate,
  );
}

/**
 * Lines over x: `series` [{label, kind, points: [[x, y, low?, high?]]}] (low–high draws a band),
 * x and y written by `x(value)` and `y(value)`. It zooms on x; charts of the same `group`
 * ([chartGroup]) point at and zoom to the same x; `animate` plays the entrance.
 */
export function lineChart({ series, x, y, label, group, animate, height = 240 }) {
  const legend = series.length > 1;
  return chart(
    height,
    (colours) => ({
      ...base(colours, label),
      grid: { left: 4, right: 14, top: legend ? 34 : 12, bottom: 58 },
      legend: legend ? legendOf(colours, series.map((line) => line.label), { top: 0 }) : undefined,
      tooltip: {
        ...tooltipOf(colours),
        trigger: "axis",
        formatter: (items) => {
          const lines = items.flatMap((item) => {
            const line = series.find((each) => each.label === item.seriesName);
            if (!line) return [];
            const [, value, low, high] = line.points[item.dataIndex];
            const band = low == null ? "" : ` (${y(low)}–${y(high)})`;
            return [`${line.label}: ${y(value)}${band}`];
          });
          return html([x(items[0].value[0]), ...lines].join("\n"));
        },
      },
      xAxis: {
        type: "value",
        min: "dataMin",
        max: "dataMax",
        axisLabel: { color: colours.muted, formatter: x, hideOverlap: true, alignMinLabel: "left", alignMaxLabel: "right" },
        axisLine: { lineStyle: { color: colours.line } },
        axisTick: { lineStyle: { color: colours.line } },
        splitLine: { show: false },
      },
      yAxis: { ...valueAxis(colours), axisLabel: { color: colours.muted, formatter: y } },
      dataZoom: [
        { type: "inside", filterMode: "none" },
        { ...sliderOf(colours), filterMode: "none", height: 16, bottom: 8, labelFormatter: x },
      ],
      series: series.flatMap((line, index) => {
        const colour = colours.kind(line.kind);
        const banded = line.points.some((point) => point.length > 2);
        // The band: its low edge, invisible, and its width stacked on it; unnamed, so neither in
        // the legend nor in the tooltip.
        const band = {
          type: "line",
          stack: `band-${index}`,
          stackStrategy: "all",
          symbol: "none",
          silent: true,
          lineStyle: { opacity: 0 },
        };
        return [
          ...(banded
            ? [
                { ...band, data: line.points.map(([at, , low]) => [at, low]) },
                {
                  ...band,
                  areaStyle: { color: colour, opacity: 0.18 },
                  data: line.points.map(([at, , low, high]) => [at, high - low]),
                },
              ]
            : []),
          {
            type: "line",
            name: line.label,
            showSymbol: false,
            lineStyle: { color: colour, width: 2 },
            itemStyle: { color: colour },
            data: line.points.map(([at, value]) => [at, value]),
          },
        ];
      }),
    }),
    group &&
      ((instance, echarts) => {
        instance.group = group;
        echarts.connect(group);
      }),
    animate,
  );
}

/**
 * Lines that grow while a run goes: `series` [{label, kind}] over x from 0 to `max`, a band
 * `shade` {to, label} from 0 (the warm-up), x and y written by `x(value)` and `y(value)`, the y
 * axis by `tick(value)`. Returns {box, set(points)}: `set` takes a list of [x, y] points per series
 * and draws them.
 */
export function liveLines({ series, max, shade, x, y, tick = y, label, height = 300 }) {
  let points = series.map(() => []);
  let instance = null;
  const box = chart(
    height,
    (colours) => ({
      ...base(colours, label),
      animation: false,
      grid: { left: 4, right: 16, top: 34, bottom: 28, containLabel: true },
      legend: legendOf(colours, series.map((line) => line.label), { top: 0 }),
      tooltip: {
        ...tooltipOf(colours),
        trigger: "axis",
        axisPointer: { type: "line", lineStyle: { color: colours.muted, width: 1 } },
        formatter: (items) =>
          html([x(items[0].value[0]), ...items.map((item) => `${item.seriesName}: ${y(item.value[1])}`)].join("\n")),
      },
      xAxis: {
        type: "value",
        min: 0,
        max,
        // The end (a horizon, rarely a round number) would crowd the last round label.
        axisLabel: { color: colours.muted, formatter: x, hideOverlap: true, showMaxLabel: false },
        axisLine: { lineStyle: { color: colours.line } },
        axisTick: { show: false },
        splitLine: { show: false },
      },
      yAxis: { ...valueAxis(colours), min: 0, axisLabel: { color: colours.muted, formatter: tick } },
      series: series.map((line, index) => ({
        type: "line",
        name: line.label,
        showSymbol: false,
        lineStyle: { color: colours.kind(line.kind), width: 2, cap: "round", join: "round" },
        itemStyle: { color: colours.kind(line.kind) },
        emphasis: { focus: "series" },
        data: points[index],
        markArea:
          index === 0 && shade
            ? {
                silent: true,
                itemStyle: { color: colours.track, opacity: 0.55 },
                label: { color: colours.muted, position: "insideTop", formatter: shade.label },
                data: [[{ xAxis: 0 }, { xAxis: shade.to }]],
              }
            : undefined,
      })),
    }),
    (drawn) => {
      instance = drawn;
    },
  );
  return {
    box,
    set(next) {
      points = next;
      instance?.setOption({ series: points.map((data) => ({ data })) });
    },
  };
}

/**
 * A heatmap: rows × columns of `values` (≥ 0), labelled by `rows` and `columns`, shaded from the
 * track colour to the warning colour; `title(row, column)` is a cell's tooltip and
 * `onSelect(row, column)` its click.
 */
export function heatmap({ rows, columns, values, title, onSelect, label }) {
  const max = Math.max(1, ...values.flat());
  return chart(
    rows.length * 20 + 84,
    (colours, width) => ({
      ...base(colours, label),
      grid: { left: 4, right: 14, top: 4, bottom: 70 },
      tooltip: { ...tooltipOf(colours), formatter: (item) => html(title(item.value[1], item.value[0])) },
      xAxis: {
        type: "category",
        data: columns,
        axisLabel: { color: colours.muted, hideOverlap: true, alignMinLabel: "left", alignMaxLabel: "right" },
        axisLine: { show: false },
        axisTick: { show: false },
      },
      yAxis: categoryAxis(colours, rows, labelWidth(width)),
      visualMap: {
        min: 0,
        max,
        orient: "horizontal",
        left: "center",
        bottom: 0,
        itemWidth: 10,
        itemHeight: 140,
        text: [formatNumber(max, 0), "0"],
        textStyle: { color: colours.muted },
        inRange: { color: [colours.track, colours.kind("warning")] },
      },
      series: [
        {
          type: "heatmap",
          data: values.flatMap((row, index) => row.map((value, column) => [column, index, value])),
          itemStyle: { borderColor: colours.card, borderWidth: columns.length > 80 ? 0 : 1 },
          emphasis: { itemStyle: { borderColor: colours.text, borderWidth: 1 } },
        },
      ],
    }),
    (instance) => instance.on("click", (item) => onSelect(item.value[1], item.value[0])),
  );
}

/**
 * A timeline from `from` to `until` (ms) in lanes: `lanes` [{label, bars: [{start, end, kind,
 * title}], marks: [{time, kind, title}]}], `lines` [{time, kind, label}] across the lanes, and
 * `kinds` {kind: legend label}; times written by `time(ms)`. It zooms on time.
 */
export function timeline({ lanes, from, until, time, kinds, lines = [], label }) {
  // A series per kind, so that the legend shows and hides each.
  const series = new Map();
  const add = (kind, item) => {
    if (!series.has(kind)) series.set(kind, []);
    series.get(kind).push(item);
  };
  lanes.forEach((lane, index) => {
    for (const bar of lane.bars) add(bar.kind, { value: [index, bar.start, bar.end], title: bar.title });
    for (const mark of lane.marks ?? []) {
      add(mark.kind, { value: [index, mark.time, mark.time], title: mark.title, mark: true });
    }
  });
  return chart(
    lanes.length * ROW + 100,
    (colours, width, echarts) => ({
      ...base(colours, label),
      useUTC: true,
      legend: legendOf(colours, [...[...series.keys()].map((kind) => kinds[kind]), ...lines.map((line) => line.label)], { top: 0 }),
      grid: { left: 4, right: 14, top: 34, bottom: 62 },
      tooltip: { ...tooltipOf(colours), trigger: "item", formatter: (item) => html(item.data.title) },
      xAxis: {
        type: "time",
        min: from,
        max: until,
        axisLabel: { color: colours.muted, formatter: (value) => time(value), hideOverlap: true, alignMinLabel: "left", alignMaxLabel: "right" },
        axisLine: { lineStyle: { color: colours.line } },
        splitLine: { show: true, lineStyle: { color: colours.line } },
      },
      yAxis: categoryAxis(colours, lanes.map((lane) => lane.label), labelWidth(width)),
      dataZoom: [
        { type: "inside", filterMode: "weakFilter" },
        { ...sliderOf(colours), filterMode: "weakFilter", height: 16, bottom: 8, labelFormatter: (value) => time(value) },
      ],
      series: [
        ...[...series].map(([kind, items]) => ({
          type: "custom",
          name: kinds[kind],
          itemStyle: { color: colours.kind(kind) },
          encode: { x: [1, 2], y: 0 },
          data: items,
          renderItem: (params, api) => {
            const item = items[params.dataIndex];
            const [start, middle] = api.coord([api.value(1), api.value(0)]);
            const [end] = api.coord([api.value(2), api.value(0)]);
            const band = api.size([0, 1])[1];
            const height = band * (item.mark ? 0.8 : 0.6);
            const shape = echarts.graphic.clipRectByRect(
              item.mark
                ? { x: start - 1, y: middle - height / 2, width: 2, height }
                : { x: start, y: middle - height / 2, width: Math.max(1, end - start), height },
              params.coordSys,
            );
            return shape && { type: "rect", shape, style: api.style() };
          },
        })),
        // Lines across all lanes, each a series of its own in the legend.
        ...lines.map((line) => ({
          type: "custom",
          name: line.label,
          itemStyle: { color: colours.kind(line.kind) },
          encode: { x: 0 },
          data: [{ value: [line.time], title: `${line.label}: ${time(line.time)}` }],
          renderItem: (params, api) => {
            const [at] = api.coord([api.value(0), 0]);
            const area = params.coordSys;
            if (at < area.x || at > area.x + area.width) return null;
            return {
              type: "line",
              shape: { x1: at, y1: area.y, x2: at, y2: area.y + area.height },
              style: { stroke: colours.kind(line.kind), lineWidth: 2 },
            };
          },
        })),
      ],
    }),
  );
}

/**
 * A box `height` px high with the chart `build(colours, width, echarts)` gives (an ECharts
 * option); `ready(instance, echarts)` adds events once it is drawn. It is drawn when the box first
 * has a width (with its entrance if `animate`), again for a new width or colour scheme, and let go
 * when the box leaves the page.
 */
function chart(height, build, ready, animate = false) {
  const box = document.createElement("div");
  box.className = "chart-box";
  box.style.height = `${height}px`;
  loadCharts().then((echarts) => {
    const entry = { instance: null, width: 0 };
    // Each drawing is a new instance at the box's width in the current colours: an updated one
    // would keep the label bounds of its old layout and animate from them. Only a first drawing
    // may play the entrance; a new width or scheme shows at once.
    const draw = () => {
      const first = !entry.instance;
      entry.instance?.dispose();
      entry.instance = echarts.init(box);
      const option = build(palette(), entry.width, echarts);
      if (!(first && animate)) option.animation = false;
      entry.instance.setOption(option);
      ready?.(entry.instance, echarts);
    };
    // A new colour scheme: drawn now, or when a hidden box shows again.
    entry.redraw = () => {
      if (box.clientWidth > 0) draw();
      else entry.width = 0;
    };
    const observer = new ResizeObserver(() => {
      if (!box.isConnected) {
        observer.disconnect();
        live.delete(entry);
        entry.instance?.dispose();
        return;
      }
      // A box in a hidden view keeps its chart for when it shows again.
      const width = box.clientWidth;
      if (width === 0 || width === entry.width) return;
      entry.width = width;
      draw();
      live.add(entry);
    });
    observer.observe(box);
  }, (error) => {
    box.classList.add("chart-failed");
    box.textContent = t("chart.failed", { message: error.message });
  });
  return box;
}

/** The CSS tokens as colours, `kind(name)` for a token by name (the process colour if none). */
function palette() {
  const style = getComputedStyle(document.documentElement);
  const token = (name) => style.getPropertyValue(`--${name}`).trim();
  return {
    font: style.fontFamily,
    text: token("text"),
    muted: token("muted"),
    line: token("line"),
    track: token("track"),
    card: token("card"),
    raised: token("raised"),
    accent: token("accent"),
    accentSoft: token("accent-soft"),
    kind: (name) => token(name) || token("process"),
  };
}

function base(colours, label) {
  return {
    animation: !still.matches,
    aria: { enabled: true, label: { description: label } },
    textStyle: { fontFamily: colours.font, color: colours.text },
  };
}

function tooltipOf(colours) {
  return {
    confine: true,
    backgroundColor: colours.raised,
    borderColor: colours.line,
    borderWidth: 1,
    padding: [7, 10],
    textStyle: { color: colours.text, fontSize: 12 },
    extraCssText: "border-radius: 10px; box-shadow: 0 12px 32px -12px rgb(0 0 0 / 0.45); max-width: 22rem; white-space: normal;",
  };
}

function legendOf(colours, names, place) {
  return {
    ...place,
    left: 0,
    data: names,
    itemWidth: 12,
    itemHeight: 10,
    textStyle: { color: colours.muted, fontSize: 12 },
    inactiveColor: colours.line,
  };
}

function valueAxis(colours) {
  return {
    type: "value",
    axisLine: { show: false },
    axisTick: { show: false },
    splitLine: { lineStyle: { color: colours.line } },
  };
}

/** A category axis down the side, first item at the top, labels cut to `width` px. */
function categoryAxis(colours, data, width) {
  return {
    type: "category",
    inverse: true,
    data,
    axisLine: { show: false },
    axisTick: { show: false },
    axisLabel: { color: colours.text, width, overflow: "truncate" },
  };
}

function sliderOf(colours) {
  return {
    type: "slider",
    borderColor: colours.line,
    backgroundColor: colours.track,
    fillerColor: colours.accentSoft,
    handleStyle: { color: colours.card, borderColor: colours.muted },
    moveHandleStyle: { color: colours.muted },
    textStyle: { color: colours.muted },
    dataBackground: { lineStyle: { color: colours.muted, opacity: 0.5 }, areaStyle: { color: colours.muted, opacity: 0.15 } },
    selectedDataBackground: { lineStyle: { color: colours.accent }, areaStyle: { color: colours.accent, opacity: 0.2 } },
  };
}

/** Width of side labels in a chart `width` px wide. */
function labelWidth(width) {
  return Math.round(Math.min(300, Math.max(72, width * 0.36)));
}

/** The first of 1, 1.2, 1.5, 2, 2.5, 3, 4, 5, 6, 8 × 10^k a little past `value` (1 for none). */
function round(value) {
  if (!(value > 0)) return 1;
  const magnitude = 10 ** Math.floor(Math.log10(value));
  return [1, 1.2, 1.5, 2, 2.5, 3, 4, 5, 6, 8, 10]
    .map((step) => step * magnitude)
    .find((each) => each >= value * 1.02);
}

function sum(values) {
  return values.reduce((total, value) => total + value, 0);
}

/** A whisker over the 95% interval of `row`'s sum and its text beside, for a bar chart. */
function whisker(row, index, api, colours, max) {
  const total = sum(row.values);
  const [end, middle] = api.coord([total, index]);
  const children = [];
  let right = end;
  if (row.ci) {
    const [low] = api.coord([Math.max(0, total - row.ci), index]);
    const [high] = api.coord([Math.min(max, total + row.ci), index]);
    const cap = Math.min(5, api.size([0, 1])[1] * 0.2);
    const style = { stroke: colours.text, lineWidth: 1.5, opacity: 0.55 };
    children.push(
      { type: "line", shape: { x1: low, y1: middle, x2: high, y2: middle }, style },
      { type: "line", shape: { x1: low, y1: middle - cap, x2: low, y2: middle + cap }, style },
      { type: "line", shape: { x1: high, y1: middle - cap, x2: high, y2: middle + cap }, style },
    );
    right = Math.max(right, high);
  }
  children.push({
    type: "text",
    style: { text: row.text, x: right + 6, y: middle, verticalAlign: "middle", fill: colours.muted, font: `12px ${colours.font}` },
  });
  return { type: "group", children };
}

let measurer = null;

/** Width of `text` at the charts' 12 px. */
function textSize(text) {
  measurer ??= document.createElement("canvas").getContext("2d");
  measurer.font = `12px ${getComputedStyle(document.documentElement).fontFamily}`;
  return measurer.measureText(text).width;
}

/** Tooltip lines as HTML. */
function html(text) {
  const escape = (line) => line.replace(/[&<>"']/g, (character) => `&#${character.charCodeAt(0)};`);
  return text.split("\n").map(escape).join("<br>");
}
