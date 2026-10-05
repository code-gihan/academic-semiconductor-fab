//! SMT2020 semiconductor manufacturing testbed (Kopp et al., 2020) on top of `des-core`: dataset
//! model and AutoSched loader, simulation with the papers' operating strategies, and measures
//! over replications.
//!
//! ```no_run
//! use std::sync::Arc;
//! use smt2020::{Config, DAY, Dataset, Simulation};
//!
//! let dataset = Arc::new(Dataset::from_bytes(&std::fs::read("ds1.bin")?)?);
//! let mut simulation = Simulation::new(dataset, Config::new(730 * DAY))?;
//! simulation.run(Some(100 * DAY))?; // paused at day 100
//! let queued = simulation.tool_groups().iter().map(|group| group.queue).sum::<usize>();
//! simulation.run(None)?; // to the end
//! let table = smt2020::report::summarize(&[simulation.results()?]);
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

/// Enum of unit variants serialized as the given names, which [`name`](#method.name) returns.
macro_rules! named_enum {
    (
        $(#[$meta:meta])*
        $vis:vis enum $name:ident {
            $( $(#[$variant_meta:meta])* $variant:ident = $text:literal, )*
        }
    ) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
        $vis enum $name {
            $( $(#[$variant_meta])* $variant, )*
        }

        impl $name {
            /// Every variant, in declaration order.
            pub const ALL: &'static [Self] = &[$(Self::$variant),*];

            /// Serialized name.
            pub fn name(self) -> &'static str {
                match self {
                    $( Self::$variant => $text, )*
                }
            }
        }

        impl serde::Serialize for $name {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_str(self.name())
            }
        }

        impl<'de> serde::Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let name = String::deserialize(deserializer)?;
                match name.as_str() {
                    $( $text => Ok(Self::$variant), )*
                    other => Err(serde::de::Error::unknown_variant(other, &[$($text),*])),
                }
            }
        }
    };
}

pub mod asd;
pub mod data;
pub mod info;
pub mod report;
mod rng;
pub mod sim;

pub use data::Dataset;
pub use des_core::{DAY, HOUR, MINUTE, SECOND, Time};
pub use info::DatasetInfo;
pub use sim::{Config, Criterion, Progress, Results, Simulation};
