//! Release plan of a run: the dataset's lots planned to start before the horizon, with release
//! times divided by the load factor and due-date offsets kept.

use des_core::Time;

use super::Error;
use super::stats::LotKind;
use crate::data::{Dataset, LotRelease, ReleaseStream};

pub(super) struct Plan {
    pub streams: Vec<ReleaseStream>,
    /// Releases made per stream.
    pub stream_next: Vec<u32>,
    /// Listed lots and initial WIP starting before the horizon, by start time (file order within).
    pub listed: Vec<LotRelease>,
    pub listed_next: usize,
    /// Lots per part still to be released.
    pub remaining: Vec<u32>,
}

impl Plan {
    pub(super) fn new(data: &Dataset, horizon: Time, load: f64) -> Result<Self, Error> {
        if !(load > 0.0 && load.is_finite()) {
            return Err(Error(format!("load factor {load} must be positive")));
        }
        let scale = |time: Time| (time as f64 / load).round() as Time;
        let kind_known = |part: usize, priority: u32| {
            LotKind::of(data.parts[part].engineering, priority).ok_or_else(|| {
                Error(format!(
                    "{} lots with priority {priority} have no lot kind",
                    data.parts[part].name
                ))
            })
        };

        let mut remaining = vec![0; data.parts.len()];
        let mut streams = Vec::with_capacity(data.streams.len());
        for stream in &data.streams {
            kind_known(stream.part, stream.priority)?;
            let stream = ReleaseStream {
                start: scale(stream.start),
                interval: scale(stream.interval),
                ..*stream
            };
            if stream.interval == 0 {
                return Err(Error(format!(
                    "load factor {load} leaves no time between releases"
                )));
            }
            remaining[stream.part] += releases_before(&stream, horizon) * stream.lots;
            streams.push(stream);
        }

        let mut listed = Vec::new();
        // Per part: (first, last, count) of the listed releases (initial WIP excluded).
        let mut spans: Vec<Option<(Time, Time, u32)>> = vec![None; data.parts.len()];
        for lot in &data.lots {
            kind_known(lot.part, lot.priority)?;
            let start = scale(lot.start);
            if lot.step.is_none() {
                let span = spans[lot.part].get_or_insert((start, start, 0));
                *span = (span.0.min(start), span.1.max(start), span.2 + 1);
            }
            if start < horizon {
                remaining[lot.part] += 1;
                listed.push(LotRelease {
                    start,
                    due: start + (lot.due - lot.start),
                    ..*lot
                });
            }
        }
        listed.sort_by_key(|lot| lot.start);
        // A list must reach the horizon: its next release, at the list's mean spacing, would be
        // due after it.
        for (part, span) in spans.iter().enumerate() {
            if let &Some((first, last, count)) = span
                && count > 1
                && last + (last - first) / Time::from(count - 1) < horizon
            {
                return Err(Error(format!(
                    "the release list of {} ends before the horizon",
                    data.parts[part].name
                )));
            }
        }
        Ok(Self {
            stream_next: vec![0; streams.len()],
            streams,
            listed,
            listed_next: 0,
            remaining,
        })
    }
}

/// Releases of `stream` starting before `horizon`.
pub(super) fn releases_before(stream: &ReleaseStream, horizon: Time) -> u32 {
    if stream.start >= horizon {
        return 0;
    }
    let fitting = (horizon - 1 - stream.start) / stream.interval + 1;
    stream.count.min(u32::try_from(fitting).unwrap_or(u32::MAX))
}
