// The race of the Home view, also the Setup view's demo: [P2]'s experiment on dataset 2 (two
// years, the first one warm-up) with its three dispatching rules, a few runs each under the same
// random numbers, and the CQT violations [P2] Table 5 reports for them (default segments, no
// stopping, 10 replications).
const DAY = 86_400_000;

export const RACE = {
  dataset: "ds2",
  settings: { horizon: 730 * DAY, warm_up: null, seed: 1, load: 1, replications: 3 },
  rules: [
    { key: "base", strategy: { queue_time: "none", ranking: {} }, paper: 17.5 },
    { key: "qtcr", strategy: { queue_time: "qtcr", ranking: {} }, paper: 9.4 },
    { key: "qts", strategy: { queue_time: "qts", ranking: {} }, paper: 9.3 },
  ],
};
