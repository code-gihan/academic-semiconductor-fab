//! Binary arithmetic coding with fixed probabilities. Each bit is coded with the probability of
//! its context, counted over the whole window first (the same coding runs twice: on a
//! [`Counter`], then on an [`Encoder`]), so that any stretch decodes from a saved [`ReadState`]
//! alone. The range coder is LZMA's: a 32-bit range over the bytes written, carries passed on.
//! Small alphabets go bit by bit down a binary tree; integers go as their bit length in unary,
//! then their bits below the leading one at even odds (Elias γ).

/// Probabilities are in 1/2^12.
const PRECISION: u32 = 12;
const TOP: u32 = 1 << 24;
const EVEN: u16 = 1 << (PRECISION - 1);

/// Codes bits: writes or counts them as given, or reads them into the arguments.
pub(super) trait Coder {
    /// Codes `bit` with the probability of `context`.
    fn bit(&mut self, context: usize, bit: &mut bool);

    /// Codes the low `count` bits of `value` at even odds, highest first.
    fn bits(&mut self, count: u32, value: &mut u64);
}

/// Hands out the contexts of a window's coding, part by part.
#[derive(Default)]
pub(super) struct Contexts(usize);

impl Contexts {
    /// The first of `count` new contexts.
    pub(super) fn take(&mut self, count: usize) -> usize {
        let first = self.0;
        self.0 += count;
        first
    }

    pub(super) fn count(&self) -> usize {
        self.0
    }
}

/// Counts the bits of each context.
pub(super) struct Counter {
    counts: Vec<[u64; 2]>,
}

impl Counter {
    pub(super) fn new(contexts: usize) -> Self {
        Self {
            counts: vec![[0; 2]; contexts],
        }
    }

    /// Each context's probability of a 0 as counted, short of certainty; even if unused.
    pub(super) fn probabilities(&self) -> Vec<u16> {
        let one = f64::from(1u32 << PRECISION);
        self.counts
            .iter()
            .map(|&[zeros, ones]| match zeros + ones {
                0 => EVEN,
                total => (zeros as f64 / total as f64 * one)
                    .round()
                    .clamp(1.0, one - 1.0) as u16,
            })
            .collect()
    }
}

impl Coder for Counter {
    fn bit(&mut self, context: usize, bit: &mut bool) {
        self.counts[context][usize::from(*bit)] += 1;
    }

    fn bits(&mut self, _: u32, _: &mut u64) {}
}

/// Writes bits with fixed probabilities.
pub(super) struct Encoder<'p> {
    probabilities: &'p [u16],
    low: u64,
    range: u32,
    /// The byte held back for a carry, and how many bytes are held back with it.
    cache: u8,
    held: u64,
    out: Vec<u8>,
}

impl<'p> Encoder<'p> {
    pub(super) fn new(probabilities: &'p [u16]) -> Self {
        Self {
            probabilities,
            low: 0,
            range: u32::MAX,
            cache: 0,
            held: 1,
            out: Vec::new(),
        }
    }

    /// The bytes written, enough to decode every bit.
    pub(super) fn finish(mut self) -> Vec<u8> {
        for _ in 0..5 {
            self.shift();
        }
        self.out
    }

    /// Moves the top byte of `low` out, once no carry can change it.
    fn shift(&mut self) {
        if self.low < 0xFF00_0000 || self.low >= 1 << 32 {
            let carry = (self.low >> 32) as u8;
            let mut byte = self.cache;
            while self.held > 0 {
                self.out.push(byte.wrapping_add(carry));
                byte = 0xFF;
                self.held -= 1;
            }
            self.cache = (self.low >> 24) as u8;
        }
        self.held += 1;
        self.low = (self.low & 0x00FF_FFFF) << 8;
    }

    fn normalize(&mut self) {
        while self.range < TOP {
            self.range <<= 8;
            self.shift();
        }
    }
}

impl Coder for Encoder<'_> {
    fn bit(&mut self, context: usize, bit: &mut bool) {
        let bound = (self.range >> PRECISION) * u32::from(self.probabilities[context]);
        if *bit {
            self.low += u64::from(bound);
            self.range -= bound;
        } else {
            self.range = bound;
        }
        self.normalize();
    }

    fn bits(&mut self, count: u32, value: &mut u64) {
        for index in (0..count).rev() {
            self.range >>= 1;
            if *value >> index & 1 == 1 {
                self.low += u64::from(self.range);
            }
            self.normalize();
        }
    }
}

/// Where a reading stands; reading on from a saved one gives the same bits.
#[derive(Clone, Copy, Debug)]
pub(super) struct ReadState {
    range: u32,
    code: u32,
    at: usize,
}

/// Reads bits written with fixed probabilities.
pub(super) struct Reading<'a> {
    bytes: &'a [u8],
    probabilities: &'a [u16],
    state: ReadState,
    damaged: bool,
}

impl<'a> Reading<'a> {
    /// Reads `bytes` from their start.
    pub(super) fn start(bytes: &'a [u8], probabilities: &'a [u16]) -> Self {
        let state = ReadState {
            range: u32::MAX,
            code: 0,
            at: 0,
        };
        let mut reading = Self::resume(bytes, probabilities, state);
        // The first byte is always 0.
        reading.damaged = reading.byte() != 0;
        for _ in 0..4 {
            reading.state.code = reading.state.code << 8 | u32::from(reading.byte());
        }
        reading
    }

    /// Reads `bytes` on from `state`.
    pub(super) fn resume(bytes: &'a [u8], probabilities: &'a [u16], state: ReadState) -> Self {
        Self {
            bytes,
            probabilities,
            state,
            damaged: false,
        }
    }

    pub(super) fn state(&self) -> ReadState {
        self.state
    }

    /// Whether the bytes were no stream: they did not start as one, or ran out.
    pub(super) fn damaged(&self) -> bool {
        self.damaged
    }

    fn byte(&mut self) -> u8 {
        match self.bytes.get(self.state.at) {
            Some(&byte) => {
                self.state.at += 1;
                byte
            }
            None => {
                self.damaged = true;
                0
            }
        }
    }

    fn normalize(&mut self) {
        while self.state.range < TOP {
            self.state.range <<= 8;
            self.state.code = self.state.code << 8 | u32::from(self.byte());
        }
    }
}

impl Coder for Reading<'_> {
    fn bit(&mut self, context: usize, bit: &mut bool) {
        let state = &mut self.state;
        let bound = (state.range >> PRECISION) * u32::from(self.probabilities[context]);
        *bit = state.code >= bound;
        if *bit {
            state.code -= bound;
            state.range -= bound;
        } else {
            state.range = bound;
        }
        self.normalize();
    }

    fn bits(&mut self, count: u32, value: &mut u64) {
        *value = 0;
        for _ in 0..count {
            self.state.range >>= 1;
            let bit = self.state.code >= self.state.range;
            if bit {
                self.state.code -= self.state.range;
            }
            *value = *value << 1 | u64::from(bit);
            self.normalize();
        }
    }
}

/// Codes `value` (below 2^`depth`) down a binary tree whose 2^`depth` − 1 nodes have the
/// contexts from `contexts` on.
pub(super) fn tree(coder: &mut impl Coder, contexts: usize, depth: u32, value: &mut u32) {
    let mut node = 1usize;
    for level in (0..depth).rev() {
        let mut bit = *value >> level & 1 == 1;
        coder.bit(contexts + node - 1, &mut bit);
        node = node << 1 | usize::from(bit);
    }
    *value = (node - (1 << depth)) as u32;
}

/// Codes `value` as its bit length in unary (from the `count` contexts from `contexts` on, the
/// last shared by every longer length), then its bits below the leading one at even odds.
pub(super) fn gamma(coder: &mut impl Coder, contexts: usize, count: usize, value: &mut u64) {
    let length = u64::BITS - value.leading_zeros();
    let mut read = 0;
    while read < u64::BITS {
        let mut longer = read < length;
        coder.bit(contexts + (read as usize).min(count - 1), &mut longer);
        if !longer {
            break;
        }
        read += 1;
    }
    if read == 0 {
        *value = 0;
        return;
    }
    let lead = 1u64 << (read - 1);
    let mut low = *value & (lead - 1);
    coder.bits(read - 1, &mut low);
    *value = lead | low;
}

/// Codes `value` (an f64) as its 64 bits at even odds.
pub(super) fn raw(coder: &mut impl Coder, value: &mut f64) {
    let mut bits = value.to_bits();
    coder.bits(64, &mut bits);
    *value = f64::from_bits(bits);
}

/// Bits needed for values up to `max`.
pub(super) fn width(max: u64) -> u32 {
    u64::BITS - max.leading_zeros()
}

/// Signed integers as unsigned ones, small either way: 0, −1, 1, −2, …
pub(super) fn zigzag(value: i64) -> u64 {
    (value << 1 ^ value >> 63) as u64
}

pub(super) fn unzigzag(value: u64) -> i64 {
    (value >> 1) as i64 ^ -((value & 1) as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A reproducible stream of pseudo-random numbers.
    fn numbers(seed: u64) -> impl FnMut() -> u64 {
        let mut state = seed;
        move || {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            state >> 11
        }
    }

    /// Codes a mix of skewed bits, raw bits, trees and integers; returns them as coded.
    fn code(coder: &mut impl Coder, values: &mut [(bool, u64, u32, u64, i64)]) {
        for (bit, raw_bits, symbol, integer, signed) in values.iter_mut() {
            coder.bit(0, bit);
            coder.bits(37, raw_bits);
            tree(coder, 1, 3, symbol);
            gamma(coder, 8, 12, integer);
            let mut zigzagged = zigzag(*signed);
            gamma(coder, 20, 6, &mut zigzagged);
            *signed = unzigzag(zigzagged);
        }
    }

    #[test]
    fn what_is_written_reads_back_from_any_saved_state() {
        let mut next = numbers(7);
        let original: Vec<_> = (0..20_000)
            .map(|index| {
                let integer = match index % 4 {
                    0 => 0,
                    1 => next() % 7,
                    2 => next() >> (next() % 53),
                    _ => u64::MAX - next() % 3,
                };
                (
                    next() % 100 < 97,
                    next() & ((1 << 37) - 1),
                    (next() % 8) as u32,
                    integer,
                    (next() as i64).wrapping_mul(if index % 2 == 0 { 1 } else { -1 })
                        >> (next() % 64),
                )
            })
            .collect();
        let mut counter = Counter::new(26);
        code(&mut counter, &mut original.clone());
        let probabilities = counter.probabilities();
        let mut encoder = Encoder::new(&probabilities);
        code(&mut encoder, &mut original.clone());
        let bytes = encoder.finish();
        // Skewed bits cost little: well under the 37 raw bits and the rest per value.
        assert!(bytes.len() < original.len() * 15, "{} bytes", bytes.len());
        let mut reading = Reading::start(&bytes, &probabilities);
        let mut saved = Vec::new();
        for (index, expected) in original.iter().enumerate() {
            if index % 1_000 == 0 {
                saved.push((index, reading.state()));
            }
            let mut value = (false, 0, 0, 0, 0);
            code(&mut reading, std::slice::from_mut(&mut value));
            assert_eq!(&value, expected, "value {index}");
        }
        assert!(!reading.damaged());
        for (index, state) in saved {
            let mut reading = Reading::resume(&bytes, &probabilities, state);
            let mut value = (false, 0, 0, 0, 0);
            code(&mut reading, std::slice::from_mut(&mut value));
            assert_eq!(value, original[index], "from the state at value {index}");
        }
    }

    #[test]
    fn short_or_foreign_bytes_are_damaged() {
        let probabilities = [EVEN; 4];
        let mut encoder = Encoder::new(&probabilities);
        let mut value = 12_345u64;
        gamma(&mut encoder, 0, 4, &mut value);
        let bytes = encoder.finish();
        let mut reading = Reading::start(&bytes[..2], &probabilities);
        gamma(&mut reading, 0, 4, &mut value);
        assert!(reading.damaged());
        assert!(Reading::start(&[1, 0, 0, 0, 0], &probabilities).damaged());
    }
}
