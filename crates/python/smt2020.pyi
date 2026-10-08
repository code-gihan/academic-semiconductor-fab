"""SMT2020 semiconductor fab simulator (Rust core): datasets, operating strategies, step-wise
runs and replication statistics. Times are ms; DAY, HOUR, MINUTE and SECOND convert."""

from os import PathLike
from typing import Any, Callable, Literal, Optional, TypedDict, Union

__version__: str
DAY: int
HOUR: int
MINUTE: int
SECOND: int

class Limits(TypedDict):
    front: int
    total: int

class Stopping(TypedDict, total=False):
    limits: dict[str, Limits]
    default: Limits

Criterion = Union[
    Literal[
        "priority",
        "least_setup",
        "fifo",
        "critical_ratio",
        "due_date",
        "shortest_step",
        "least_remaining",
        "qtcr",
        "qts",
        "qt_deadline",
        "code",
    ],
    dict[str, float],
]
"""A lot ranking criterion, smallest value first; {"qt_within": ms} ranks the lots with at most
that much queue-time slack first; "code" by the strategy code's priority."""

class AmhsConfig(TypedDict, total=False):
    """AMHS settings of a dataset with a layout (SMAT2022); speeds mm/s, accelerations mm/s²."""

    vehicles: Optional[int]
    """Vehicles in service: the layout's first ones; all if None."""
    dispatch: Literal["nearest", "bay"]
    """"nearest" idle vehicle along the rails, or the SMAT2022 simulator's "bay" rings."""
    look_ahead: Optional[int]
    """Lots (batches) a tool may have assigned beyond the jobs it runs at once; None as the
    SMAT2022 simulator (as many as its ports hold, a batch tool none)."""
    hoist: float
    """Time to load or unload a FOUP at a port (ms)."""
    roam_limit: int
    max_speed: Optional[float]
    acceleration: Optional[float]
    deceleration: Optional[float]
    straight_speed: Optional[float]
    curve_speed: Optional[float]

class _ConfigRequired(TypedDict):
    horizon: float
    """Lots planned to start before the horizon (ms) are released; the run then goes on until
    every released lot is complete."""

class Config(_ConfigRequired, total=False):
    warm_up: Optional[float]
    """Reporting periods WarmUp [0, warm_up) and Period_1 [warm_up, horizon) instead of the
    dataset's."""
    seed: int
    replication: int
    load: float
    reserve_super_hot: bool
    queue_time: Literal["none", "qtcr", "qts", "code"]
    flow_factors: Optional[list[list[Optional[float]]]]
    ranking: dict[str, list[Criterion]]
    """Criteria per tool group (1 to 6, distinct, most significant first); other groups rank by
    the dataset's with the queue-time rule before FIFO/CR."""
    batch_start_within: Optional[float]
    """A batch below its minimum size also starts once one of its lots has at most this much
    queue-time slack (ms)."""
    stopping: Optional[Stopping]
    engineering: Union[Literal["base", "engineering_first"], dict[str, Any]]
    """"base", "engineering_first", {"cate": {"production": ms, "engineering": ms}} or
    {"cot": {"trigger": lots}}."""
    amhs: Optional[AmhsConfig]
    """AMHS settings of a dataset with a layout, which always runs its AMHS."""

class ToolGroupInfo(TypedDict):
    name: str
    area: int
    tools: int
    batching: bool
    setup_runs: bool
    stepper: bool
    ranks: list[Criterion]

class PartInfo(TypedDict):
    name: str
    family: str
    engineering: bool
    route: int

class StepInfo(TypedDict):
    name: str
    tool_group: int

class RouteInfo(TypedDict):
    name: str
    steps: list[StepInfo]

class SegmentInfo(TypedDict):
    route: int
    entry: int
    exit: int
    limit: int
    litho: bool
    tool_groups: list[int]

class PeriodInfo(TypedDict):
    name: str
    start: int
    report: bool
    reset: bool

class LayoutInfo(TypedDict):
    """An AMHS layout's size."""

    vehicles: int
    bays: int
    rails: int
    rail_length: float
    """m."""
    zones: int
    tool_ports: int
    buffers: int

class DatasetInfo(TypedDict):
    """Indices refer to the lists: areas, tool_groups, routes; segments are in route and step
    order."""

    areas: list[str]
    tool_groups: list[ToolGroupInfo]
    parts: list[PartInfo]
    routes: list[RouteInfo]
    segments: list[SegmentInfo]
    periods: list[PeriodInfo]
    layout: Optional[LayoutInfo]
    """The AMHS layout's size; None without one."""

# "pass" is a keyword: the functional form declares it.
Progress = TypedDict(
    "Progress",
    {
        "pass": int,
        "passes": int,
        "now": int,
        "horizon": int,
        "released": int,
        "completed": int,
        "wip": int,
        "cqt_completed": int,
        "cqt_violated": int,
        "finished": bool,
    },
)

# Events from "from" up to "until" (ms, both required), at these tool groups and of these lots
# (release numbers); empty lists pass all. "from" is a keyword: the functional form declares it.
EventFilter = TypedDict(
    "EventFilter",
    {"from": float, "until": float, "tool_groups": list[str], "lots": list[int]},
    total=False,
)

# A window from "from" up to "until" (ms); "from" is a keyword: the functional form declares it.
ReplayWindow = TypedDict("ReplayWindow", {"from": float, "until": float})

class Recording(TypedDict, total=False):
    violations: bool
    """Every CQT segment completion over its limit."""
    tool_groups: bool
    """Each tool group's mean queue and tool time per state, day by day."""
    events: Optional[EventFilter]
    """The events of a window."""
    replay: Optional[ReplayWindow]
    """The AMHS of a window, for a ReplayPlayer (a dataset with a layout)."""

class Violations(TypedDict):
    lot: list[int]
    part: list[int]
    kind: list[str]
    segment: list[int]
    release: list[int]
    entered: list[int]
    arrived: list[int]
    exit: list[int]

class ToolGroupDays(TypedDict):
    day: list[int]
    tool_group: list[int]
    queue: list[float]
    down: list[int]
    pm: list[int]
    setup: list[int]
    process: list[int]
    load: list[int]
    unload: list[int]
    idle: list[int]

class Events(TypedDict):
    time: list[int]
    kind: list[
        Literal[
            "release",
            "arrive",
            "start",
            "end",
            "complete",
            "pickup",
            "dropoff",
            "down",
            "up",
            "pm_start",
            "pm_end",
        ]
    ]
    lot: list[Optional[int]]
    part: list[Optional[int]]
    tool: list[Optional[int]]
    tool_group: list[Optional[int]]
    step: list[Optional[int]]

class Records(TypedDict):
    """Tables of equal-length columns (pandas.DataFrame(records["violations"])); indices refer to
    the dataset info."""

    violations: Violations
    tool_groups: ToolGroupDays
    events: Events

class DaySummary(TypedDict):
    day: int
    scope: Literal["fab", "cqt"]
    measure: str
    n: int
    mean: float
    std: Optional[float]
    ci95: Optional[float]

class LotStatus(TypedDict):
    id: int
    part: str
    kind: Literal["PRL", "PHL", "SHL", "ERL", "EHL"]
    priority: int
    wafers: int
    release: int
    due: int
    step: int
    step_name: str
    tool_group: str
    state: Literal["moving", "queued", "processing", "leaving"]
    tool: Optional[int]
    port: Optional[int]
    """AMHS: the port its FOUP stands at (a commit station's while in it)."""
    vehicle: Optional[int]
    """AMHS: the vehicle carrying its FOUP."""
    cqt_exit: Optional[int]
    cqt_deadline: Optional[int]

class ToolStatus(TypedDict):
    id: int
    tool_group: str
    state: Literal["down", "pm", "setup", "process", "load", "unload", "idle"]
    setup: Optional[str]
    lots: list[int]

class ToolGroupStatus(TypedDict):
    name: str
    area: str
    tools: int
    queue: int
    down: int
    pm: int
    setup: int
    process: int
    load: int
    unload: int
    idle: int

class CqtReport(TypedDict):
    """CQT segment completions (exit step started); violation and slack are totals in ms."""

    completed: int
    violated: int
    violated_1h: int
    violated_2h: int
    violated_4h: int
    violation: int
    slack: int

class SegmentLot(TypedDict):
    id: int
    kind: Literal["PRL", "PHL", "SHL", "ERL", "EHL"]
    step: int
    state: Literal["moving", "queued", "processing", "leaving"]
    entered: int
    slack: int
    """Latest start of the exit step (entered + limit) − now − expected work until it."""

class SegmentStatus(TypedDict):
    lots: list[SegmentLot]
    """Lots from the end of the entrance step until the exit step starts, by id."""
    cqt: CqtReport
    """Completions since time 0 (not reset by reporting periods)."""

class VehicleStatus(TypedDict):
    id: int
    activity: Literal["idle", "to_pickup", "loading", "to_dropoff", "unloading"]
    link: int
    offset: float
    x: float
    y: float
    heading: float
    """Rail, position of its front on it (mm), its point and heading (rad)."""
    tail_x: float
    tail_y: float
    """Point of its rear along the rails (mm)."""
    speed: float
    """m/s."""
    lot: Optional[int]

class FoupStatus(TypedDict):
    port: int
    lot: int
    kind: Literal["PRL", "PHL", "SHL", "ERL", "EHL"]

class FoupCount(TypedDict):
    at: int
    foups: int

class Moves(TypedDict):
    """Deliveries by origin and destination."""

    tool_to_tool: int
    tool_to_buffer: int
    tool_to_complete: int
    buffer_to_tool: int
    buffer_to_buffer: int
    buffer_to_complete: int
    commit_to_tool: int
    commit_to_buffer: int
    commit_to_complete: int

class BayDistance(TypedDict):
    """Loaded drives of a bay distance class: their times summed, and their unobstructed times
    (alone on the rails, zones free), ms."""

    drives: int
    time: int
    unobstructed: int

class VehicleTimes(TypedDict):
    """Vehicle time per activity, summed over the vehicles (ms)."""

    idle: int
    to_pickup: int
    loading: int
    to_dropoff: int
    unloading: int

class AmhsReport(TypedDict):
    """The AMHS in a window; sums over the deliveries in ms."""

    vehicles: int
    moves: Moves
    vehicle_wait: int
    empty_drive: int
    loaded_drive: int
    delivery: int
    bay_distances: list[BayDistance]
    """By bay distance class: 0 to 9, 10 or more, outside the intrabays."""
    vehicle_time: VehicleTimes
    blocked: int
    zone_waits: int
    zone_wait: int
    empty_distance: float
    loaded_distance: float
    """m."""

class AmhsStatus(TypedDict):
    vehicles: list[VehicleStatus]
    foups: list[FoupStatus]
    """FOUPs at ports of tools and track buffers."""
    kept: list[int]
    """Ports kept for a FOUP on its way."""
    committed: list[FoupCount]
    """FOUPs in commit stations by the station's port."""
    inside: list[FoupCount]
    """FOUPs in batch tools by tool."""
    tools: list[Literal["down", "pm", "setup", "process", "load", "unload", "idle"]]
    backlog: int
    """Transports waiting for a vehicle."""
    report: AmhsReport
    """The AMHS measures of the reporting window so far."""

class VehicleFrames(TypedDict):
    """Every vehicle, by id: its front's point (mm) and heading (rad), its rear's point along the
    rails (mm), its speed (m/s) and what it does."""

    x: list[float]
    y: list[float]
    heading: list[float]
    tail_x: list[float]
    tail_y: list[float]
    speed: list[float]
    activity: list[Literal["idle", "to_pickup", "loading", "to_dropoff", "unloading"]]

class FoupFrames(TypedDict):
    """Every FOUP in the fab, by lot: the lot's kind and where the FOUP is (the port, vehicle or
    tool, or the commit station's port)."""

    lot: list[int]
    kind: list[Literal["PRL", "PHL", "SHL", "ERL", "EHL"]]
    place: list[Literal["port", "vehicle", "tool", "commit"]]
    index: list[int]

class Frame(TypedDict):
    """The fab at an instant of a replay."""

    time: float
    vehicles: VehicleFrames
    foups: FoupFrames
    tools: list[Literal["down", "pm", "setup", "process", "load", "unload", "idle"]]
    delivered: int
    """Deliveries picked up and set down within the window so far, those from a tool's port to
    a tool's, and their time on vehicles (ms)."""
    tool_to_tool: int
    carried: float

class Arc(TypedDict):
    cx: float
    cy: float
    radius: float
    start: float
    sweep: float
    """Angles in rad from the center; the sweep positive counterclockwise."""

# A rail from node "from" to node "to"; "from" is a keyword: the functional form declares it.
RailDrawing = TypedDict(
    "RailDrawing",
    {"from": int, "to": int, "bay": int, "length": float, "arc": Optional[Arc]},
)

class BayDrawing(TypedDict):
    name: str
    interbay: bool
    bounds: list[float]

class Station(TypedDict):
    name: str
    bay: int
    x: float
    y: float
    width: float
    height: float

class PortDrawing(TypedDict):
    x: float
    y: float
    role: Literal["tool", "buffer", "stocker", "commit", "complete"]
    station: Optional[int]

class Drawing(TypedDict):
    """An AMHS layout for drawing (mm); indices refer to the lists."""

    bounds: list[float]
    """Least x and y, greatest x and y."""
    nodes: list[list[float]]
    rails: list[RailDrawing]
    bays: list[BayDrawing]
    tools: list[Optional[Station]]
    stockers: list[Station]
    commits: list[Station]
    completes: list[Station]
    ports: list[PortDrawing]
    vehicle_lengths: list[float]

class Cqt:
    """The CQT segment a lot is in; times and work in ms."""

    segment: int
    """Index in the dataset info's segments."""
    limit: int
    entered: int
    """End of the entrance step; the exit step must start by deadline = entered + limit."""
    deadline: int
    exit: int
    before_exit: float
    """Expected work from the lot's step until the exit step starts."""
    slack: float
    """deadline - now - before_exit."""

class Lot:
    """A lot as strategy code sees it; times and work in ms."""

    id: int
    part: str
    kind: Literal["PRL", "PHL", "SHL", "ERL", "EHL"]
    priority: int
    wafers: int
    release: int
    due: int
    step: int
    step_name: str
    tool_group: str
    remaining: float
    """Expected work from this step to the end."""
    step_time: float
    cqt: Optional[Cqt]

class SegmentGroup:
    """Lots in CQT segments at a tool group: queued or processing there, and those plus the ones
    still to reach it."""

    tool_group: str
    front: int
    total: int

class Segment:
    """The CQT segment a lot is about to start."""

    segment: int
    limit: int
    exit: int
    groups: list[SegmentGroup]

class Batch:
    """A batch below its minimum size; times in ms."""

    tool_group: str
    step: int
    step_name: str
    lots: int
    wafers: int
    min: int
    max: int
    oldest: int
    """The earliest arrival of its lots in the queue."""
    slack: Optional[float]
    """The least queue-time slack of its lots in CQT segments."""

Code = Any
"""Strategy code: an object with any of the methods
priority(lot: Lot, now: int) -> float (smaller first; where the configuration ranks by code),
admit(lot: Lot, segment: Segment, now: int) -> bool | float (False or a time holds the lot),
start_batch(batch: Batch, now: int) -> bool | float (True starts it, False or a time waits)."""

class Summary(TypedDict):
    period: str
    scope: str
    item: str
    kind: Optional[str]
    measure: str
    n: int
    mean: float
    std: Optional[float]
    ci95: Optional[float]

class Comparison(TypedDict):
    """A measure of two configurations, their replications paired by seed and replication."""

    period: str
    scope: str
    item: str
    kind: Optional[str]
    measure: str
    n: int
    baseline: float
    other: float
    difference: float
    std: Optional[float]
    ci95: Optional[float]

Results = dict[str, Any]
"""Seed and replication, reporting periods (with every CQT segment and its steps), days, lot
counts, end, events and QTS flow factors of a finished run (README)."""

class Dataset:
    """A decoded dataset, shared by the simulations of it."""

    def __init__(self, bytes: bytes) -> None:
        """Decodes a dataset file (`smt2020 convert` output) of this build's format version."""
    def info(self) -> DatasetInfo:
        """Its areas, tool groups, parts, routes, CQT segments and periods, by name and index."""
    def layout(self) -> Optional[Drawing]:
        """Its AMHS layout for drawing; None without one."""

def load_dataset(source: Union[str, PathLike[str]]) -> Dataset:
    """"ds1" to "ds4" and "smat2022" (DS4 with its AMHS) are the bundled datasets; any other
    source is the path of a dataset file or of an AutoSched model directory (*.asd)."""

class Simulation:
    """One run of a configuration on a dataset from time 0, advanced in steps. Pausing leaves
    the results unchanged; QTS without flow factors measures them in a first pass."""

    def __init__(
        self,
        dataset: Dataset,
        config: Config,
        recording: Optional[Recording] = None,
        code: Optional[Code] = None,
    ) -> None:
        """The simulation of `config` on `dataset` at time 0, recording what `recording` asks,
        with the strategy `code` (ValueError if invalid). Recording leaves the results
        unchanged."""
    def config(self) -> Config: ...
    def recording(self) -> Recording: ...
    def records(self) -> Records:
        """The tables recorded so far (empty during a first pass)."""
    def flow_factors(self) -> Optional[list[list[Optional[float]]]]:
        """The QTS flow factors of the configured run (given, or measured by the first pass);
        a configuration with them runs the same in one pass."""
    def reset(self, config: Optional[Config] = None) -> None:
        """Starts over at time 0 with `config`, or with the same configuration, and the same
        recording."""
    def run(
        self,
        until: Optional[float] = None,
        on_progress: Optional[Callable[[Progress], Optional[bool]]] = None,
    ) -> Progress:
        """Runs up to `until` (ms, events at it included) or to the end and returns the
        progress. `on_progress(progress)` is called once per simulated day: returning False
        pauses the run there; an exception pauses it and is raised on, as Ctrl-C is. A run with
        lots left a year after the horizon fails (RuntimeError)."""
    def progress(self) -> Progress: ...
    def lots(self) -> list[LotStatus]:
        """The lots in the fab, by id."""
    def tools(self) -> list[ToolStatus]:
        """Every tool, by id."""
    def tool_groups(self) -> list[ToolGroupStatus]:
        """Every tool group, in dataset order."""
    def segments(self) -> list[SegmentStatus]:
        """Every CQT segment, in the order of the dataset info's segments."""
    def amhs(self) -> Optional[AmhsStatus]:
        """The AMHS (vehicles, FOUPs, tool states); None without a layout."""
    def replay(self) -> Optional[bytes]:
        """The AMHS replay recorded so far (recording "replay") in its compact form, for a
        ReplayPlayer; None without one or before its window began."""
    def results(self) -> Results:
        """Results of the finished run (RuntimeError before)."""

class ReplayPlayer:
    """Plays a replay on the dataset it was recorded on: the fab at any instant of its window,
    each vehicle within 10⁻⁴ mm of where it was."""

    def __init__(self, dataset: Dataset, replay: bytes) -> None:
        """Reads and checks `replay` (ValueError if it is not one of this dataset)."""
    def window(self) -> ReplayWindow: ...
    def frame(self, time: float) -> Frame:
        """The fab at `time` (ms, within the window)."""

def summarize(results: list[Results]) -> list[Summary]:
    """Measures of one configuration's replications: means and 95% confidence intervals."""

def daily(results: list[Results]) -> list[DaySummary]:
    """Day-by-day measures of one configuration's replications: lots released, completed and
    in the fab, CQT completions, their share over the limit and mean excess."""

def csv(summaries: list[Summary]) -> str:
    """Summaries as CSV: period,scope,item,kind,measure,n,mean,std,ci95."""

def compare(baseline: list[Results], other: list[Results]) -> list[Comparison]:
    """Every measure of `other` against `baseline`, replications paired by seed and replication:
    the means and the mean difference (other - baseline) with its 95% confidence interval.
    Raises ValueError if a run has no partner."""

def comparison_csv(comparisons: list[Comparison]) -> str:
    """Comparisons as CSV: period,scope,item,kind,measure,n,baseline,other,difference,std,ci95."""

def digest(results: Results) -> str:
    """Equal digests mean bit-identical results."""
