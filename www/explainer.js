// The Home view's picture of a critical queue time (anime.js): a lot leaves step A and waits in
// the queue of step B while the limit's clock runs; it starts B in time, then, the next time
// round, after the clock ran out. It plays while the Home view shows and the page is visible, and
// stands still (a lot waiting, the clock part run) for those who prefer reduced motion.
import { createTimeline } from "./vendor/anime.esm.min.js";
import { still } from "./motion.js";

/** Lot positions along the picture, from its place at step A: in B's queue, at step B (user
 * units of the SVG). */
const AT_A = 0;
const IN_QUEUE = 160;
const AT_B = 280;

let timeline = null;

/** Plays the picture in `svg` while `playing` (the view shows, the page is visible). */
export function explain(svg, playing) {
  if (still()) return;
  if (!timeline) timeline = build(svg);
  if (playing) timeline.play();
  else timeline.pause();
}

function build(svg) {
  const lot = svg.querySelector(".lot");
  const clock = svg.querySelector(".clock-run");
  const ring = svg.querySelector(".clock");
  const [glowA, glowB] = svg.querySelectorAll(".glow");
  const ok = svg.querySelector(".badge-ok");
  const late = svg.querySelector(".badge-late");
  const flash = { opacity: [0, 1], duration: 260, alternate: true, loop: 1 };
  // One round from `start`: the lot waits `wait` ms in the queue; in time, the clock is part run
  // when it starts B, too late, the clock ran out after `limit` ms. Returns the round's end.
  const round = (tl, start, wait, limit) => {
    const queued = start + 1000;
    const started = queued + wait;
    const badge = limit < wait ? late : ok;
    tl.set(lot, { x: AT_A }, start)
      .set(clock, { strokeDashoffset: 100 }, start)
      .call(() => ring.classList.remove("late"), start)
      .add(lot, { opacity: [0, 1], duration: 300 }, start)
      .add(glowA, flash, start)
      .add(lot, { x: IN_QUEUE, duration: 700, ease: "inOutQuad" }, start + 300)
      .add(clock, { strokeDashoffset: 100 - (100 * Math.min(wait, limit)) / limit, duration: Math.min(wait, limit), ease: "linear" }, queued)
      .add(lot, { x: AT_B, duration: 520, ease: "inOutQuad" }, started)
      .add(glowB, flash, started + 420)
      .add(badge, { opacity: [0, 1], scale: [0.8, 1], duration: 380, ease: "outBack" }, started + 420)
      .add([lot, badge], { opacity: 0, duration: 300 }, started + 1700);
    if (badge === late) tl.call(() => ring.classList.add("late"), queued + limit);
    return started + 2000;
  };
  const tl = createTimeline({ loop: true, autoplay: false });
  const second = round(tl, 0, 1300, 2200);
  round(tl, second, 2300, 1500);
  return tl;
}
