// Motion of the page (anime.js): entrances, counts, pulses and disclosures. None when the user
// prefers reduced motion; the page shows the same end state either way.
import { animate, remove, stagger } from "./vendor/anime.esm.min.js";

const reduced = matchMedia("(prefers-reduced-motion: reduce)");

/** Fades `targets` in from below, one every `step` ms. */
export function reveal(targets, step = 45) {
  if (reduced.matches) return;
  remove(targets);
  animate(targets, {
    opacity: [0, 1],
    y: [10, 0],
    duration: 420,
    delay: stagger(step),
    ease: "outCubic",
  });
}

/** Counts `element` up from 0 to `value`, written by `format`. */
export function countUp(element, value, format) {
  element.textContent = format(value);
  if (reduced.matches) return;
  const count = { value: 0 };
  animate(count, {
    value,
    duration: 900,
    ease: "outExpo",
    onUpdate: () => {
      element.textContent = format(count.value);
    },
  });
}

/** A short pulse that marks a choice. */
export function pulse(element) {
  if (reduced.matches) return;
  remove(element);
  animate(element, { scale: [0.97, 1], duration: 360, ease: "outBack" });
}

/** Opens or closes a disclosure, its height animated. */
export function toggle(element, open) {
  remove(element);
  if (reduced.matches) {
    element.hidden = !open;
    return;
  }
  if (open) {
    element.hidden = false;
    animate(element, {
      height: [0, element.scrollHeight],
      opacity: [0, 1],
      duration: 260,
      ease: "outQuad",
      onComplete: () => element.style.removeProperty("height"),
    });
  } else {
    animate(element, {
      height: [element.scrollHeight, 0],
      opacity: [1, 0],
      duration: 200,
      ease: "inQuad",
      onComplete: () => {
        element.hidden = true;
        element.style.removeProperty("height");
        element.style.removeProperty("opacity");
      },
    });
  }
}
