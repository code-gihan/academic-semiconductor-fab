//! Random number streams: one Xoshiro256++ stream per (seed, replication, purpose), 2^128 draws
//! apart, so runs that differ only in strategy share their random numbers per purpose (common
//! random numbers). `libm` keeps native and wasm results bit-identical.

use des_core::Time;
use rand_xoshiro::Xoshiro256PlusPlus;
use rand_xoshiro::rand_core::{Rng, SeedableRng};

use crate::data::Dist;

/// What a random number is drawn for; each purpose has its own stream.
#[derive(Clone, Copy)]
pub(crate) enum Purpose {
    Process,
    Setup,
    Transport,
    Sampling,
    Rework,
    Breakdown,
    Pm,
}

const PURPOSES: usize = 7;

pub(crate) struct Streams([Xoshiro256PlusPlus; PURPOSES]);

impl Streams {
    pub(crate) fn new(seed: u64, replication: u32) -> Self {
        let mut next = Xoshiro256PlusPlus::seed_from_u64(seed);
        for _ in 0..replication as usize * PURPOSES {
            next.jump();
        }
        Self(std::array::from_fn(|_| {
            let stream = next.clone();
            next.jump();
            stream
        }))
    }

    /// Uniform on [0, 1).
    fn unit(&mut self, purpose: Purpose) -> f64 {
        (self.0[purpose as usize].next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Draw rounded to whole ms.
    pub(crate) fn sample(&mut self, purpose: Purpose, dist: Dist) -> Time {
        match dist {
            Dist::Constant(time) => time,
            Dist::Uniform { mean, half_width } => {
                mean - half_width + (self.unit(purpose) * (2 * half_width) as f64).round() as Time
            }
            Dist::Exponential { mean } => {
                (-(mean as f64) * libm::log(1.0 - self.unit(purpose))).round() as Time
            }
        }
    }

    /// Bernoulli trial; a certain outcome draws nothing.
    pub(crate) fn chance(&mut self, purpose: Purpose, probability: f64) -> bool {
        probability >= 1.0 || self.unit(purpose) < probability
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn streams_are_reproducible_and_independent() {
        let draw = |seed, replication, purpose| {
            let mut streams = Streams::new(seed, replication);
            (0..4)
                .map(|_| {
                    streams.sample(
                        purpose,
                        Dist::Uniform {
                            mean: 1_000,
                            half_width: 1_000,
                        },
                    )
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(draw(7, 0, Purpose::Process), draw(7, 0, Purpose::Process));
        assert_ne!(draw(7, 0, Purpose::Process), draw(7, 0, Purpose::Setup));
        assert_ne!(draw(7, 0, Purpose::Process), draw(7, 1, Purpose::Process));
        assert_ne!(draw(7, 0, Purpose::Process), draw(8, 0, Purpose::Process));
    }

    #[test]
    fn samples_stay_in_range() {
        let mut streams = Streams::new(1, 0);
        for _ in 0..10_000 {
            let uniform = streams.sample(
                Purpose::Process,
                Dist::Uniform {
                    mean: 100,
                    half_width: 5,
                },
            );
            assert!((95..=105).contains(&uniform));
            assert!(streams.sample(Purpose::Breakdown, Dist::Exponential { mean: 100 }) >= 0);
        }
        assert_eq!(streams.sample(Purpose::Process, Dist::Constant(42)), 42);
        assert!(streams.chance(Purpose::Sampling, 1.0));
    }
}
