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
    ],
    dict[str, float],
]
"""A lot ranking criterion, smallest value first; {"qt_within": ms} ranks the lots with at most
that much queue-time slack first."""

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
    queue_time: Literal["none", "qtcr", "qts"]
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

class DatasetInfo(TypedDict):
    """Indices refer to the lists: areas, tool_groups, routes; segments are in route and step
    order."""

    areas: list[str]
    tool_groups: list[ToolGroupInfo]
    parts: list[PartInfo]
    routes: list[RouteInfo]
    segments: list[SegmentInfo]
    periods: list[PeriodInfo]

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
        "finished": bool,
    },
)

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
    state: Literal["moving", "queued", "processing"]
    tool: Optional[int]
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

Results = dict[str, Any]
"""Reporting periods, lot counts, end, events and QTS flow factors of a finished run (README)."""

class Dataset:
    """A decoded dataset, shared by the simulations of it."""

    def __init__(self, bytes: bytes) -> None:
        """Decodes a dataset file (`smt2020 convert` output) of this build's format version."""
    def info(self) -> DatasetInfo:
        """Its areas, tool groups, parts, routes, CQT segments and periods, by name and index."""

def load_dataset(source: Union[str, PathLike[str]]) -> Dataset:
    """"ds1" to "ds4" are the bundled datasets; any other source is the path of a dataset file
    or of an AutoSched model directory (*.asd)."""

class Simulation:
    """One run of a configuration on a dataset from time 0, advanced in steps. Pausing leaves
    the results unchanged; QTS without flow factors measures them in a first pass."""

    def __init__(self, dataset: Dataset, config: Config) -> None:
        """The simulation of `config` on `dataset` at time 0 (ValueError if invalid)."""
    def config(self) -> Config: ...
    def reset(self, config: Optional[Config] = None) -> None:
        """Starts over at time 0 with `config`, or with the same configuration."""
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
    def results(self) -> Results:
        """Results of the finished run (RuntimeError before)."""

def summarize(results: list[Results]) -> list[Summary]:
    """Measures of one configuration's replications: means and 95% confidence intervals."""

def csv(summaries: list[Summary]) -> str:
    """Summaries as CSV: period,scope,item,kind,measure,n,mean,std,ci95."""

def digest(results: Results) -> str:
    """Equal digests mean bit-identical results."""
