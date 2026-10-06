// Motion of the page (anime.js): entrances, counts, the sliding tab indicator, live numbers that
// glide to their new value, and pulses. None when the user prefers reduced motion; the
// page shows the same end state either way.
import { animate, remove, spring, stagger, utils } from "./vendor/anime.esm.min.js";

const reduced = matchMedia("(prefers-reduced-motion: reduce)");
/** Numbers shown by elements: {value (shown now), format}; their animations target this. */
const numbers = new WeakMap();

/** Whether motion is off: other modules' animations follow the same choice. */
export function still() {
  return reduced.matches;
}

/** Fades `targets` in from below, one every `step` ms. */
export function reveal(targets, step = 40) {
  if (reduced.matches) return;
  remove(targets);
  animate(targets, {
    opacity: [0, 1],
    y: [6, 0],
    duration: 420,
    delay: stagger(step),
    ease: "outQuad",
  });
}

/** Counts `element` up from 0 to `value`, written by `format`, after `delay` ms. */
export function countUp(element, value, format, delay = 0) {
  const number = numberOf(element, format);
  remove(number);
  if (reduced.matches) {
    show(element, number, value);
    return;
  }
  show(element, number, 0);
  animate(number, {
    value,
    duration: 700,
    delay,
    ease: "outQuart",
    onUpdate: () => show(element, number, number.value),
  });
}

/** Moves the number `element` shows to `value`, written by `format`: gliding from the number it
 * shows now (a glide on its way turns), or at once the first time. */
export function glide(element, value, format) {
  const first = !numbers.has(element);
  const number = numberOf(element, format);
  if (first || reduced.matches || number.value === value) {
    remove(number);
    show(element, number, value);
    return;
  }
  animate(number, {
    value,
    duration: 450,
    ease: "outQuad",
    onUpdate: () => show(element, number, number.value),
  });
}

function numberOf(element, format) {
  let number = numbers.get(element);
  if (!number) {
    number = { value: 0 };
    numbers.set(element, number);
  }
  number.format = format;
  return number;
}

function show(element, number, value) {
  number.value = value;
  element.textContent = number.format(value);
}

/** Puts the tab indicator under `tab`: sliding there, or at once. */
export function slideTo(indicator, tab, animated) {
  const place = { x: tab.offsetLeft, width: tab.offsetWidth };
  remove(indicator);
  if (!animated || reduced.matches) {
    utils.set(indicator, place);
    return;
  }
  animate(indicator, { ...place, ease: spring({ bounce: 0, duration: 300 }) });
}

/** A short pulse that marks a choice. */
export function pulse(element) {
  if (reduced.matches) return;
  remove(element);
  animate(element, { scale: [0.97, 1], duration: 360, ease: "outBack" });
}
