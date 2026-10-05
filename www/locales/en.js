// English texts of the page, the default language. Every locale file has these keys; {name} marks
// a value the page inserts.
export default {
  "meta.title": "SMT2020 Fab Simulator",
  "meta.description":
    "Run the SMT2020 semiconductor fab testbed in your browser: a Rust discrete-event simulator compiled to WebAssembly.",

  "header.title": "SMT2020 Fab Simulator",
  "header.tagline":
    "Simulate the four SMT2020 semiconductor fab testbeds and compare operating strategies, right in your browser.",
  "header.language": "Language",
  "header.source": "Source code",
  "intro.local":
    "The discrete-event simulator is written in Rust and runs here as WebAssembly, one replication per CPU thread. Nothing is uploaded.",

  "dataset.heading": "Dataset",
  "dataset.ds1.name": "DS1 · HV/LM",
  "dataset.ds1.description":
    "High volume, low mix: 2 products, 1,043 tools, periodic releases, FIFO dispatching.",
  "dataset.ds2.name": "DS2 · LV/HM",
  "dataset.ds2.description":
    "Low volume, high mix: 10 products, 913 tools, a release list with a due date per lot, critical ratio dispatching.",
  "dataset.ds3.name": "DS3 · HV/LM + engineering",
  "dataset.ds3.description": "DS1 plus engineering lots of 1 product (40 a week): 1,135 tools.",
  "dataset.ds4.name": "DS4 · LV/HM + engineering",
  "dataset.ds4.description": "DS2 plus engineering lots of 3 products (80 a week): 1,068 tools.",
  "dataset.file.name": "Your dataset file",
  "dataset.file.description":
    "A .bin file made with smt2020 convert, for example from other order files of a model.",
  "dataset.file.label": "Dataset file (.bin)",
  "dataset.common":
    "All four: 105 tool groups in 11 areas, 10,000 wafer starts a week (400 production lots of 25 wafers) and an initial WIP.",

  "strategy.heading": "Operating strategy",
  "strategy.queueTime.label": "CQT dispatching",
  "strategy.queueTime.none": "None (BASE)",
  "strategy.queueTime.qtcr": "QTCR: queue time critical ratio",
  "strategy.queueTime.qts": "QTS: queue time scheduling",
  "strategy.queueTime.hint":
    "A critical queue time (CQT) limits the wait between two steps. QTCR serves first the lots whose time left to the limit is short against their remaining work; QTS gives every step a deadline from flow factors measured in a pre-run, so a replication takes about twice as long.",
  "strategy.stopping.label": "Stopping",
  "strategy.stopping.none": "Off",
  "strategy.stopping.high": "High limits (90/130 · 90/220)",
  "strategy.stopping.medium": "Medium limits (60/95 · 70/150)",
  "strategy.stopping.small": "Small limits (50/85 · 55/125)",
  "strategy.stopping.hint":
    "Holds a lot at the start of a CQT interval while a stepper group of the interval (LithoTrack_FE_95 · FE_115) has too many CQT lots in queue or in process (first limit), or also on their way to it (second limit).",
  "strategy.engineering.label": "Engineering lots",
  "strategy.engineering.none": "BASE (no engineering lots)",
  "strategy.engineering.base": "BASE: priorities of the data",
  "strategy.engineering.ef": "EF: engineering first",
  "strategy.engineering.cate":
    "CAtE {cycle} h: {production} h production, {engineering} h engineering",
  "strategy.engineering.cateSet":
    "CAtE {cycle} h ({dataset}): {production} h production, {engineering} h engineering",
  "strategy.engineering.cot": "CoT {trigger}: engineering first from {trigger} waiting",
  "strategy.engineering.hint":
    "How production and engineering lots share the fab (DS3, DS4). EF ranks EHL > PHL > ERL > PRL at every tool group. At the steppers, CAtE alternates production and engineering windows, and CoT serves production first until N engineering lots wait, then those N.",
  "strategy.superHot.label": "Reserve tools for super hot lots",
  "strategy.superHot.short": "Super hot reservation",
  "strategy.superHot.hint":
    "When a super hot lot starts processing, one tool of its next step waits for it (AutoSched rule_HotLotFIRST). Off in the AutoSched models.",

  "settings.heading": "Run settings",
  "settings.horizon.label": "End time (days)",
  "settings.horizon.hint":
    "Simulated days from 2018-01-01. The first year is warm-up and each further year is reported; lots released before the end time are then completed.",
  "settings.replications.label": "Replications",
  "settings.replications.hint":
    "Independent runs with different random numbers, {threads} at a time on this device. Results show their mean and 95% confidence interval.",
  "settings.seed.label": "Random seed",
  "settings.seed.hint":
    "Runs with the same seed and replication number draw the same random numbers, so strategies are compared under equal conditions.",
  "settings.load.label": "Load factor",
  "settings.load.hint":
    "Scales the release rate: 1 is the planned 10,000 wafers a week, 0.9 is 90%.",

  "run.start": "Run simulation",
  "run.cancel": "Cancel",
  "run.duration":
    "A 730-day replication takes about 15–25 s on one core of a recent desktop (QTS about twice as long); replications running side by side can each take longer.",

  "status.loadingWasm": "Loading the simulator…",
  "status.wasmFailed":
    "The simulator could not be loaded ({message}). Use a recent Chrome, Edge, Firefox or Safari.",
  "status.loadingDataset": "Loading {dataset}…",
  "status.starting": "Starting the run…",
  "status.done": "Finished in {time}.",
  "status.cancelled": "Cancelled.",
  "status.error": "The run failed: {message}",
  "error.noFile": "Choose a dataset file first.",
  "error.fetch": "Could not load {file} (HTTP {status}).",
  "error.worker": "A Web Worker failed: {message}",

  "progress.done": "Replications done: {done}/{count}",
  "progress.day": "day {day} of {days}",
  "progress.drain": "finishing released lots ({wip} left)",
  "progress.preRun": "QTS pre-run",
  "progress.mainRun": "QTS main run",
  "progress.elapsed": "{time} elapsed",
  "progress.remaining": "about {time} left",

  "results.heading": "Results",
  "results.period": "Report period",
  "results.periodOption": "{name} · days {start}–{end}",
  "results.periodHint":
    "WarmUp: the first year. Period_n: from the second year through year n + 1, cut at the end time. Drain: lots completed after the end time.",
  "results.downloadJson": "Download JSON",
  "results.downloadCsv": "Download CSV",
  "results.ci":
    "Values are means over the replications; ± is the half-width of their 95% confidence interval (Student t).",
  "results.kinds":
    "Lot kinds: PRL production, PHL production hot, SHL super hot, ERL engineering, EHL engineering hot.",
  "results.reproduce.title": "Reproducibility and performance",
  "results.reproduce.text":
    "Equal digests mean bit-identical results: a replication's config from the JSON download, run with smt2020 run --config, gives the same digest.",
  "results.digest": "Replication {replication}: {digest} ({time})",
  "results.performance":
    "Replications: {count} · workers: {workers} · total {time} · {perReplication} per replication · {events} million events/s · peak memory {memory} MB",

  "kpi.started": "Lots released",
  "kpi.completed": "Lots completed",
  "kpi.wip": "Mean WIP (lots)",
  "kpi.prlCt": "Cycle time, PRL (days)",
  "kpi.prlOnTime": "On time, PRL (%)",
  "kpi.erlCt": "Cycle time, ERL (days)",
  "kpi.cqt": "CQT violations (%)",

  "table.kind.title": "Lot kinds",
  "table.kind.item": "Kind",
  "table.kind.note":
    "ACT: average cycle time from release to completion. On time: completed by the due date. FF: flow factor, cycle time over raw processing time.",
  "table.ff.title": "Flow factor percentiles",
  "table.ff.item": "Kind",
  "table.ff.note": "Flow factors of the completed lots: P0 is the minimum, P50 the median, P100 the maximum.",
  "table.lot.title": "Products and lot kinds",
  "table.lot.item": "Product, kind",
  "table.lot.note": "The measures of the lot kinds table, per product.",
  "table.cqt.title": "Critical queue time intervals",
  "table.cqt.item": "Intervals",
  "table.cqt.note":
    "%VL: completed intervals over the limit; > 1 h, 2 h, 4 h: over by more than that. AVL and AONT: mean excess and mean slack per completed interval. Litho: intervals through the steppers.",
  "table.area.title": "Areas",
  "table.area.item": "Area",
  "table.area.note":
    "Availability: neither down nor in PM. SDT share: PM share of the down time. Utilization: setup, load, unload and processing; Max: its busiest tool group.",
  "table.toolGroup.title": "Tool groups by utilization",
  "table.toolGroup.item": "Tool group",
  "table.toolGroup.note": "Share of tool time per state (%).",

  "col.completed": "Completed",
  "col.ctMean": "ACT (d)",
  "col.ctStd": "CT std (d)",
  "col.onTime": "On time (%)",
  "col.ffMean": "Mean FF",
  "col.percentile": "P{p}",
  "col.vl": "%VL",
  "col.vl1h": "> 1 h (%)",
  "col.vl2h": "> 2 h (%)",
  "col.vl4h": "> 4 h (%)",
  "col.avl": "AVL (h)",
  "col.aont": "AONT (h)",
  "col.availability": "Availability (%)",
  "col.sdtShare": "SDT share (%)",
  "col.util": "Utilization (%)",
  "col.utilMax": "Max (%)",
  "col.down": "Down",
  "col.pm": "PM",
  "col.setup": "Setup",
  "col.process": "Process",
  "col.load": "Load",
  "col.unload": "Unload",
  "col.idle": "Idle",

  "cqt.litho": "Litho",
  "cqt.rest": "Rest",
  "cqt.total": "Total",

  "common.on": "On",
  "common.off": "Off",

  "python.heading": "Python package",
  "python.intro":
    "The same simulator as a Python module (Python 3.9 or later on Windows, Linux and macOS) with the four datasets: run, pause, inspect and reset simulations in your own code. Install it from this page:",
  "python.downloads": "Or download the wheel of your platform:",
  "python.none": "The wheels are built when the page is deployed; this copy has none.",
  "python.windows": "Windows (x64)",
  "python.linux": "Linux (x86-64)",
  "python.macos": "macOS (Intel and Apple silicon)",
  "python.quickstart": "Quick start",
  "python.more": "Every function and option, also for JavaScript and Rust:",

  "footer.references": "References",
  "footer.data":
    "Data: SMT2020 testbed release 1.0 (2020), FernUniversität in Hagen. The dataset files are converted from its AutoSched AP models.",
};
