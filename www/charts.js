// Horizontal bar charts and the tooltip the charts and the fab map share.

const tooltip = document.getElementById("tooltip");

/** Shows `text()` (lines) next to `element` while it is pointed at or focused. */
export function withTooltip(element, text) {
  const show = () => {
    tooltip.textContent = text();
    tooltip.hidden = false;
    const box = element.getBoundingClientRect();
    const tip = tooltip.getBoundingClientRect();
    const left = Math.min(
      Math.max(8, box.left + box.width / 2 - tip.width / 2),
      innerWidth - tip.width - 8,
    );
    const above = box.top - tip.height - 8;
    tooltip.style.left = `${left}px`;
    tooltip.style.top = `${above >= 8 ? above : box.bottom + 8}px`;
  };
  const hide = () => {
    tooltip.hidden = true;
  };
  element.addEventListener("pointerenter", show);
  element.addEventListener("pointerleave", hide);
  element.addEventListener("focus", show);
  element.addEventListener("blur", hide);
}

/**
 * A horizontal bar chart on a common scale up to `max`. Each row has a `label`, stacked
 * `segments` ({value, kind}; the kind picks the colour) from 0, an optional `ci` (95%
 * half-width, a whisker at the end of the bar), the `value` text beside it and the `title` lines
 * of its tooltip.
 */
export function barChart(rows, max) {
  const chart = element("div", "bars");
  for (const row of rows) {
    const track = element("div", "bar-track");
    track.tabIndex = 0;
    let start = 0;
    for (const segment of row.segments) {
      const fill = element("span", "bar-fill");
      fill.dataset.kind = segment.kind;
      fill.style.left = percent(start / max);
      fill.style.width = percent(segment.value / max);
      start += segment.value;
      track.append(fill);
    }
    if (row.ci) {
      const [low, high] = [Math.max(0, start - row.ci), Math.min(max, start + row.ci)];
      const whisker = element("span", "bar-ci");
      whisker.style.left = percent(low / max);
      whisker.style.width = percent((high - low) / max);
      track.append(whisker);
    }
    withTooltip(track, () => row.title);
    const line = element("div", "bar-row");
    line.append(
      element("span", "bar-label", row.label),
      track,
      element("span", "bar-value", row.value),
    );
    chart.append(line);
  }
  return chart;
}

/** Colour keys of stacked segments, named in `label(kind)`. */
export function legend(kinds, label) {
  const list = element("ul", "legend");
  for (const kind of kinds) {
    const swatch = element("span", "swatch");
    swatch.dataset.kind = kind;
    const item = element("li", "");
    item.append(swatch, document.createTextNode(label(kind)));
    list.append(item);
  }
  return list;
}

function percent(share) {
  return `${Math.min(100, Math.max(0, share * 100))}%`;
}

function element(tag, className, text) {
  const node = document.createElement(tag);
  node.className = className;
  if (text != null) node.textContent = text;
  return node;
}
