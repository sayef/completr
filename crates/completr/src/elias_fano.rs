//! Elias-Fano coding of a sorted sequence of integers, read in place.

use crate::codec::{BitWriter, Column, Reader, Writer};
use crate::trie::select_in_word;
use crate::Error;

/// Zeros of the high bits between two stored select positions.
const SAMPLE: usize = 512;

/// Low bits of each value bit packed, high bits in unary: each value sets bit `high + index`.
pub(crate) struct EliasFano {
    len: usize,
    universe: u64,
    low_bits: u32,
    lows: Column<u64>,
    highs: Column<u64>,
    /// Position of every `SAMPLE`-th zero of `highs`.
    samples: Column<u64>,
}

impl EliasFano {
    fn low_bits(len: usize, universe: u64) -> u32 {
        if len == 0 || universe <= len as u64 {
            0
        } else {
            (universe / len as u64).ilog2()
        }
    }

    fn high_bits(len: usize, universe: u64, low_bits: u32) -> Option<usize> {
        len.checked_add(usize::try_from(universe >> low_bits).ok()?)?
            .checked_add(1)
    }

    /// Writes `len` sorted values below `universe`. `values` is called twice.
    pub(crate) fn write<I: Iterator<Item = u64>>(
        w: &mut Writer,
        len: usize,
        universe: u64,
        values: impl Fn() -> I,
    ) {
        let l = Self::low_bits(len, universe);
        w.u64(len as u64);
        w.u64(universe);
        w.u64(u64::from(l));
        let mut lows = BitWriter::new(w, len * l as usize);
        if l > 0 {
            for v in values() {
                lows.push(v & ((1 << l) - 1), l);
            }
        }
        lows.finish();
        let bits = Self::high_bits(len, universe, l).expect("universe fits in memory");
        let mut highs = BitWriter::new(w, bits);
        let (mut samples, mut zeros, mut pos) = (Vec::new(), 0usize, 0usize);
        let mut zeros_to = |highs: &mut BitWriter, target: usize, pos: &mut usize| {
            let n = target - zeros;
            let mut next = zeros.next_multiple_of(SAMPLE);
            while next < target {
                samples.push((*pos + next - zeros) as u64);
                next += SAMPLE;
            }
            highs.zeros(n);
            *pos += n;
            zeros = target;
        };
        for v in values() {
            zeros_to(&mut highs, (v >> l) as usize, &mut pos);
            highs.push(1, 1);
            pos += 1;
        }
        let total_zeros = bits - len;
        zeros_to(&mut highs, total_zeros, &mut pos);
        highs.finish();
        w.column(&samples);
    }

    pub(crate) fn read(r: &mut Reader, full: bool) -> Result<Self, Error> {
        let corrupt = || Error::Corrupt("invalid Elias-Fano sequence".into());
        let len = usize::try_from(r.u64()?).map_err(|_| corrupt())?;
        let universe = r.u64()?;
        let low_bits = r.u64()?;
        let (lows, highs, samples) = (r.column()?, r.column()?, r.column()?);
        if low_bits != u64::from(Self::low_bits(len, universe)) {
            return Err(corrupt());
        }
        let low_bits = low_bits as u32;
        let bits = Self::high_bits(len, universe, low_bits).ok_or_else(corrupt)?;
        let low_words = len
            .checked_mul(low_bits as usize)
            .ok_or_else(corrupt)?
            .div_ceil(64);
        let ef = Self {
            len,
            universe,
            low_bits,
            lows,
            highs,
            samples,
        };
        let valid = ef.lows.len() == low_words
            && ef.highs.len() == bits.div_ceil(64)
            && ef.samples.len() == (bits - len).div_ceil(SAMPLE)
            && (!full || ef.verify(bits));
        valid.then_some(ef).ok_or_else(corrupt)
    }

    /// Every bit and sample, against the header.
    fn verify(&self, bits: usize) -> bool {
        let highs = self.highs.as_slice();
        let tail = bits % 64;
        let ones: usize = highs.iter().map(|w| w.count_ones() as usize).sum();
        let padding_clear = tail == 0 || highs.last().is_none_or(|w| w >> tail == 0);
        let samples_match = (0..self.samples.len())
            .all(|k| self.select0(k * SAMPLE) == Some(self.samples.as_slice()[k] as usize));
        ones == self.len
            && padding_clear
            && samples_match
            && self.range(0, u64::MAX).count() == self.len
            && self
                .range(0, u64::MAX)
                .last()
                .is_none_or(|v| v < self.universe)
    }

    /// Position of the `k`-th zero of the high bits.
    fn select0(&self, k: usize) -> Option<usize> {
        let highs = self.highs.as_slice();
        let mut pos = *self.samples.as_slice().get(k / SAMPLE)? as usize;
        let mut remaining = k % SAMPLE;
        loop {
            let word = !*highs.get(pos / 64)? >> (pos % 64);
            let count = word.count_ones() as usize;
            if remaining < count {
                return Some(pos + select_in_word(word, remaining));
            }
            remaining -= count;
            pos = (pos / 64 + 1) * 64;
        }
    }

    fn low(&self, i: usize) -> u64 {
        if self.low_bits == 0 {
            return 0;
        }
        let words = self.lows.as_slice();
        let at = i * self.low_bits as usize;
        let mut v = words.get(at / 64).map_or(0, |w| w >> (at % 64));
        if at % 64 + self.low_bits as usize > 64 {
            v |= words.get(at / 64 + 1).map_or(0, |w| w << (64 - at % 64));
        }
        v & ((1 << self.low_bits) - 1)
    }

    /// Values in `lo..hi`, in order.
    pub(crate) fn range(&self, lo: u64, hi: u64) -> Range<'_> {
        let high = lo >> self.low_bits;
        let pos = match high {
            0 => Some(0),
            h => usize::try_from(h - 1)
                .ok()
                .and_then(|h| self.select0(h))
                .map(|p| p + 1),
        };
        let (pos, index) = match pos {
            Some(pos) => (pos, pos.saturating_sub(high as usize)),
            None => (usize::MAX, self.len),
        };
        Range {
            ef: self,
            pos,
            index,
            lo,
            hi,
        }
    }

    pub(crate) fn size(&self) -> usize {
        24 + (self.lows.len() + self.highs.len() + self.samples.len()) * 8
    }
}

pub(crate) struct Range<'a> {
    ef: &'a EliasFano,
    pos: usize,
    index: usize,
    lo: u64,
    hi: u64,
}

impl Iterator for Range<'_> {
    type Item = u64;

    fn next(&mut self) -> Option<u64> {
        let highs = self.ef.highs.as_slice();
        while self.index < self.ef.len {
            let mut at = self.pos / 64;
            let mut word = *highs.get(at)? & (u64::MAX << (self.pos % 64));
            while word == 0 {
                at += 1;
                word = *highs.get(at)?;
            }
            let one = at * 64 + word.trailing_zeros() as usize;
            let high = one.checked_sub(self.index)? as u64;
            let v = high << self.ef.low_bits | self.ef.low(self.index);
            self.pos = one + 1;
            self.index += 1;
            if v >= self.hi {
                self.index = self.ef.len;
                return None;
            }
            if v >= self.lo {
                return Some(v);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::Bytes;

    fn roundtrip(values: &[u64], universe: u64) -> EliasFano {
        let mut w = Writer::default();
        EliasFano::write(&mut w, values.len(), universe, || values.iter().copied());
        let mut r = Reader::new(Bytes::from_vec(std::mem::take(&mut w.buf))).unwrap();
        let ef = EliasFano::read(&mut r, true).unwrap();
        r.finish().unwrap();
        ef
    }

    #[test]
    fn ranges_match_a_scan() {
        let mut state = 7u64;
        for (n, universe) in [
            (0, 0),
            (1, 1),
            (5, 3),
            (1000, 1 << 40),
            (5000, 6000),
            (3000, 100_000),
        ] {
            let mut values: Vec<u64> = (0..n)
                .map(|_| {
                    state = state
                        .wrapping_mul(6364136223846793005)
                        .wrapping_add(1442695040888963407);
                    (state >> 11) % universe.max(1)
                })
                .collect();
            values.sort_unstable();
            let ef = roundtrip(&values, universe.max(1));
            assert_eq!(ef.range(0, u64::MAX).collect::<Vec<_>>(), values);
            for lo in (0..universe).step_by((universe as usize / 37).max(1)) {
                let hi = lo + universe / 50 + 1;
                let want: Vec<u64> = values
                    .iter()
                    .copied()
                    .filter(|v| (lo..hi).contains(v))
                    .collect();
                assert_eq!(
                    ef.range(lo, hi).collect::<Vec<_>>(),
                    want,
                    "{n} {universe} {lo}"
                );
            }
        }
    }
}
