//! AMHS measures (\[SMAT2022\] §4): deliveries by origin and destination, the tool-to-tool share,
//! where their time went (waiting for a vehicle, driving empty, driving loaded against its
//! unobstructed time, by bay distance), and the vehicles' time per activity.

use des_core::Time;
use serde::{Deserialize, Serialize};

/// Bay distance classes of loaded drives: 0 to 9, 10 or more, and drives with an end outside the
/// intrabays.
pub const BAY_DISTANCE_CLASSES: usize = 12;

/// The AMHS in a window.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AmhsReport {
    /// Vehicles in service.
    pub vehicles: u32,
    pub moves: Moves,
    /// Sums over the deliveries (ms): from the request to a vehicle taking it, the vehicle's
    /// drive to the pickup, the loaded drive (pickup done to arrival), and from the request to
    /// the drop-off done.
    pub vehicle_wait: Time,
    pub empty_drive: Time,
    pub loaded_drive: Time,
    pub delivery: Time,
    /// Loaded drives by bay distance (0–9, 10+, outside the intrabays).
    pub bay_distances: Vec<BayDistance>,
    /// Vehicle time per activity, summed over the vehicles (ms).
    pub vehicle_time: VehicleTimes,
    /// Time vehicles stood on the way behind others or at zones, summed (ms).
    pub blocked: Time,
    /// Zone requests that had to wait, and their wait (ms).
    pub zone_waits: u64,
    pub zone_wait: Time,
    /// Distance driven empty and loaded (m).
    pub empty_distance: f64,
    pub loaded_distance: f64,
}

/// Deliveries by origin and destination: tools, track buffers, commit and complete stations.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Moves {
    pub tool_to_tool: u64,
    pub tool_to_buffer: u64,
    pub tool_to_complete: u64,
    pub buffer_to_tool: u64,
    pub buffer_to_buffer: u64,
    pub buffer_to_complete: u64,
    pub commit_to_tool: u64,
    pub commit_to_buffer: u64,
    pub commit_to_complete: u64,
}

impl Moves {
    pub fn total(&self) -> u64 {
        self.tool_to_tool
            + self.tool_to_buffer
            + self.tool_to_complete
            + self.buffer_to_tool
            + self.buffer_to_buffer
            + self.buffer_to_complete
            + self.commit_to_tool
            + self.commit_to_buffer
            + self.commit_to_complete
    }

    fn count(&mut self, from: End, to: End) {
        let count = match (from, to) {
            (End::Tool, End::Tool) => &mut self.tool_to_tool,
            (End::Tool, End::Buffer) => &mut self.tool_to_buffer,
            (End::Tool, End::Complete) => &mut self.tool_to_complete,
            (End::Buffer, End::Tool) => &mut self.buffer_to_tool,
            (End::Buffer, End::Buffer) => &mut self.buffer_to_buffer,
            (End::Buffer, End::Complete) => &mut self.buffer_to_complete,
            (End::Commit, End::Tool) => &mut self.commit_to_tool,
            (End::Commit, End::Buffer) => &mut self.commit_to_buffer,
            (End::Commit, End::Complete) => &mut self.commit_to_complete,
            _ => unreachable!("deliveries start at tools, buffers or commits"),
        };
        *count += 1;
    }
}

/// Loaded drives of a bay distance class.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BayDistance {
    pub drives: u64,
    /// Sum of drive times, and of their unobstructed times (alone on the rails, ZCUs free) (ms).
    pub time: Time,
    pub unobstructed: Time,
}

/// Vehicle time per activity (ms).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VehicleTimes {
    pub idle: Time,
    pub to_pickup: Time,
    pub loading: Time,
    pub to_dropoff: Time,
    pub unloading: Time,
}

/// What a delivery starts or ends at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum End {
    Tool,
    Buffer,
    Commit,
    Complete,
}

/// A delivery done, for the statistics.
pub(super) struct Delivered {
    pub from: End,
    pub to: End,
    pub vehicle_wait: Time,
    pub empty_drive: Time,
    pub loaded_drive: Time,
    pub unobstructed: Time,
    pub delivery: Time,
    /// Bay distance class.
    pub class: usize,
}

/// Window sums since the last reset.
#[derive(Default)]
pub(super) struct Stats {
    pub report: AmhsReport,
}

impl Stats {
    pub(super) fn new(vehicles: u32) -> Self {
        Self {
            report: AmhsReport {
                vehicles,
                bay_distances: vec![BayDistance::default(); BAY_DISTANCE_CLASSES],
                ..AmhsReport::default()
            },
        }
    }

    pub(super) fn delivered(&mut self, done: &Delivered) {
        let report = &mut self.report;
        report.moves.count(done.from, done.to);
        report.vehicle_wait += done.vehicle_wait;
        report.empty_drive += done.empty_drive;
        report.loaded_drive += done.loaded_drive;
        report.delivery += done.delivery;
        let class = &mut report.bay_distances[done.class];
        class.drives += 1;
        class.time += done.loaded_drive;
        class.unobstructed += done.unobstructed;
    }

    /// Starts a new window.
    pub(super) fn reset(&mut self) {
        *self = Self::new(self.report.vehicles);
    }
}

/// Class of a bay distance (`None`: an end outside the intrabays).
pub(super) fn class(distance: Option<u32>) -> usize {
    match distance {
        Some(distance) => (distance as usize).min(10),
        None => 11,
    }
}
