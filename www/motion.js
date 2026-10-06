// Motion of the page (anime.js): entrances, counts, the sliding tab indicator, live numbers that
// glide to their new value, pops and pulses. None when the user prefers reduced motion; the
// page shows the same end state either way.
import { animate, remove, spring, splitText, stagger, utils } from "./vendor/anime.esm.min.js";

const reduced = matchMedia("(prefers-reduced-motion: reduce)");
/** Numbers shown by elements: {value (shown now), format}; their animations target this. */
const numbers = new WeakMap();

/** Whether motion is off: other modules' animations follow the same choice. */
export function still() {
  return reduced.matches;
}

/** Fades `targets` in from below, one every `step` ms. */
export function reveal(targets, step = 55) {
  if (reduced.matches) return;
  remove(targets);
  animate(targets, {
    opacity: [0, 1],
    y: [14, 0],
    duration: 640,
    delay: stagger(step),
    ease: "outExpo",
  });
}

/** Raises the words of `element` one after another, after `delay` ms; its text is whole again
 * afterwards. */
export function riseWords(element, delay = 0) {
  if (reduced.matches) return;
  const split = splitText(element, { words: { wrap: "clip" } });
  animate(split.words, {
    y: ["105%", "0%"],
    duration: 900,
    delay: stagger(55, { start: delay }),
    ease: "outExpo",
    onComplete: () => {
      // A new language may have replaced the split text meanwhile: it stays.
      const replaced = split.words[0]?.isConnected ? null : element.textContent;
      split.revert();
      if (replaced !== null) element.textContent = replaced;
    },
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
    duration: 1100,
    delay,
    ease: "outExpo",
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
  animate(indicator, { ...place, ease: spring({ bounce: 0.25, duration: 420 }) });
}

/** Pops `targets` in, one after another: a fade, a little rise and scale. */
export function popIn(targets, step = 70) {
  if (reduced.matches) return;
  remove(targets);
  animate(targets, {
    opacity: [0, 1],
    scale: [0.94, 1],
    y: [10, 0],
    delay: stagger(step),
    ease: spring({ bounce: 0.3, duration: 520 }),
  });
}

/** A short pulse that marks a choice. */
export function pulse(element) {
  if (reduced.matches) return;
  remove(element);
  animate(element, { scale: [0.97, 1], duration: 360, ease: "outBack" });
}
