// Types of SMT2020 strategy code: the functions a simulation calls and what they see. Define any
// of priority, admit and startBatch. Times and work are in ms; the views are valid during the
// call only.

/** One minute, hour and day, in ms. */
declare const MINUTE: number;
declare const HOUR: number;
declare const DAY: number;

/** A lot. */
interface Lot {
  /** Release number: lots are numbered from 0 in release order. */
  readonly id: number;
  /** Its product. */
  readonly part: string;
  /** Regular, hot, super hot, engineering regular or engineering hot. */
  readonly kind: "PRL" | "PHL" | "SHL" | "ERL" | "EHL";
  /** Dispatching priority: 10 regular, 20 hot, 30 super hot (the engineering rule may change it). */
  readonly priority: number;
  readonly wafers: number;
  /** Release time and due date. */
  readonly release: number;
  readonly due: number;
  /** The step it waits at: its index in the route, its name and its tool group. */
  readonly step: number;
  readonly stepName: string;
  readonly toolGroup: string;
  /** Expected processing time from this step to the end of the route. */
  readonly remaining: number;
  /** Expected duration of this step. */
  readonly stepTime: number;
  /** The CQT segment it is in, or null. */
  readonly cqt: Cqt | null;
}

/** The CQT segment a lot is in: its exit step must start by `deadline`. */
interface Cqt {
  /** Index of the segment in the dataset's CQT segments. */
  readonly segment: number;
  /** Time limit of the segment. */
  readonly limit: number;
  /** End of the entrance step; `deadline` = `entered` + `limit`. */
  readonly entered: number;
  readonly deadline: number;
  /** Index of the exit step in the route. */
  readonly exit: number;
  /** Expected work from the lot's step until the exit step starts. */
  readonly beforeExit: number;
  /** Queue-time slack: `deadline` − now − `beforeExit`. Negative: expected to miss the limit. */
  readonly slack: number;
}

/** The CQT segment a lot is about to start, with the counts stopping limits read. */
interface Segment {
  /** Index of the segment in the dataset's CQT segments. */
  readonly segment: number;
  readonly limit: number;
  /** Index of the exit step in the route. */
  readonly exit: number;
  /** Its tool groups after the entrance step. */
  readonly groups: SegmentGroup[];
}

/** Lots in CQT segments at a tool group. */
interface SegmentGroup {
  readonly toolGroup: string;
  /** Queued or processing at the group. */
  readonly front: number;
  /** Those and the ones still to reach it in their segment. */
  readonly total: number;
}

/** A batch below its minimum size: the best-ranked lot's batch kind, filled in rank order. */
interface Batch {
  readonly toolGroup: string;
  /** The step of its first lot: index in the route, and name. */
  readonly step: number;
  readonly stepName: string;
  readonly lots: number;
  readonly wafers: number;
  /** The step's batch size limits, in wafers. */
  readonly min: number;
  readonly max: number;
  /** The earliest arrival of its lots in the queue. */
  readonly oldest: number;
  /** The least queue-time slack of its lots in CQT segments, or null. */
  readonly slack: number | null;
}
