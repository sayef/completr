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

    /// A writer for about `estimate` sorted values below `universe`.
    pub(crate) fn writer<'w, 's>(
        w: &'w mut Writer<'s>,
        universe: u64,
        estimate: usize,
    ) -> EliasFanoWriter<'w, 's> {
        let low_bits = Self::low_bits(estimate, universe);
        let header = w.position();
        w.u64(0);
        w.u64(universe);
        w.u64(u64::from(low_bits));
        EliasFanoWriter {
            lows: BitWriter::new(w),
            header,
            universe,
            low_bits,
            highs: Vec::new(),
            len: 0,
        }
    }

    /// Writes `len` sorted values below `universe`.
    #[cfg(test)]
    fn write(w: &mut Writer, len: usize, universe: u64, values: impl Iterator<Item = u64>) {
        let mut writer = Self::writer(w, universe, len);
        values.for_each(|v| writer.push(v));
        writer.finish();
    }

    pub(crate) fn read(r: &mut Reader, full: bool) -> Result<Self, Error> {
        let corrupt = || Error::Corrupt("invalid Elias-Fano sequence".into());
        let len = usize::try_from(r.u64()?).map_err(|_| corrupt())?;
        let universe = r.u64()?;
        let low_bits = r.u64()?;
        let (lows, highs, samples) = (r.column()?, r.column()?, r.column()?);
        if low_bits >= 64 {
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

    #[inline]
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

/// Values pushed in order: the low bits go out as they come, the high bits, about two per value,
/// are held until the end.
pub(crate) struct EliasFanoWriter<'w, 's> {
    lows: BitWriter<'w, 's>,
    header: usize,
    universe: u64,
    low_bits: u32,
    highs: Vec<u64>,
    len: usize,
}

impl EliasFanoWriter<'_, '_> {
    pub(crate) fn push(&mut self, v: u64) {
        let l = self.low_bits;
        if l > 0 {
            self.lows.push(v & ((1 << l) - 1), l);
        }
        let bit = (v >> l) as usize + self.len;
        if bit / 64 >= self.highs.len() {
            self.highs.resize(bit / 64 + 1, 0);
        }
        self.highs[bit / 64] |= 1 << (bit % 64);
        self.len += 1;
    }

    pub(crate) fn finish(mut self) {
        let bits = EliasFano::high_bits(self.len, self.universe, self.low_bits)
            .expect("universe fits in memory");
        self.highs.resize(bits.div_ceil(64), 0);
        let w = self.lows.finish();
        w.patch_u64(self.header, self.len as u64);
        w.column(&self.highs);
        let mut samples = Vec::new();
        let mut zeros = 0usize;
        for (i, &word) in self.highs.iter().enumerate() {
            let valid = (bits - i * 64).min(64);
            let free = !word & (u64::MAX >> (64 - valid));
            let count = free.count_ones() as usize;
            let mut next = zeros.next_multiple_of(SAMPLE);
            while next < zeros + count {
                samples.push((i * 64 + select_in_word(free, next - zeros)) as u64);
                next += SAMPLE;
            }
            zeros += count;
        }
        w.column(&samples);
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

    #[inline]
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
        EliasFano::write(&mut w, values.len(), universe, values.iter().copied());
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
