// The page's tooltip: one element, shown beside what is pointed at, focused or tapped, and kept
// inside the window.
const tooltip = document.getElementById("tooltip");

/** Shows `text()` (lines) next to `element` while it is pointed at, focused or tapped. */
export function withTooltip(element, text) {
  element.addEventListener("pointerenter", (event) => {
    if (event.pointerType !== "touch") showTip(element.getBoundingClientRect(), text());
  });
  element.addEventListener("pointerleave", hideTip);
  element.addEventListener("focus", () => showTip(element.getBoundingClientRect(), text()));
  element.addEventListener("blur", hideTip);
  // A tap shows the tip; the next tap hides it.
  element.addEventListener("click", (event) => {
    if (event.pointerType === "touch" || event.pointerType === "pen") {
      if (tooltip.hidden) showTip(element.getBoundingClientRect(), text());
      else hideTip();
    }
  });
}

/** Hides the tooltip (another view shows, say). */
export function hideTip() {
  tooltip.hidden = true;
}

/** Shows `text` beside the box, kept inside the window. */
function showTip(box, text) {
  tooltip.textContent = text;
  tooltip.hidden = false;
  const tip = tooltip.getBoundingClientRect();
  const left = Math.max(
    8,
    Math.min(box.left + box.width / 2 - tip.width / 2, innerWidth - tip.width - 8),
  );
  const above = box.top - tip.height - 8;
  tooltip.style.left = `${left}px`;
  tooltip.style.top = `${above >= 8 ? above : Math.min(box.bottom + 8, innerHeight - tip.height - 8)}px`;
}
