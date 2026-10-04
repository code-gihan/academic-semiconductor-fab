//! Simulation time: integer milliseconds since the start of the run (t = 0). Integer time keeps
//! event ordering exact and identical on native and wasm targets.

/// Instant or duration in milliseconds.
pub type Time = i64;

pub const SECOND: Time = 1_000;
pub const MINUTE: Time = 60 * SECOND;
pub const HOUR: Time = 60 * MINUTE;
pub const DAY: Time = 24 * HOUR;
