//! Integers in blocks of `BLOCK`, each bit packed as offsets from a line through its block, as in
//! tantivy's blockwise linear codec.

use crate::codec::{BitWriter, Bytes, Column, Reader, Writer};
use crate::Error;

const BLOCK: usize = 128;

pub(crate) struct Blocked {
    len: usize,
    /// Per block, its base and slope, then its first bit `<< 8 | width`: the `i`th value is
    /// `base + slope * i` plus its packed offset, all wrapping.
    headers: Column<u64>,
    bits: Column<u64>,
}

/// Values pushed one at a time, each block fitted and packed as it fills, so only the packed form
/// is held.
#[derive(Default)]
pub(crate) struct BlockedWriter {
    headers: Vec<u64>,
    words: Vec<u64>,
    bits: usize,
    block: Vec<u64>,
    len: usize,
}

impl BlockedWriter {
    pub(crate) fn push(&mut self, v: u64) {
        self.block.push(v);
        self.len += 1;
        if self.block.len() == BLOCK {
            self.close();
        }
    }

    fn put(&mut self, v: u64, width: u32) {
        if width == 0 {
            return;
        }
        let shift = self.bits % 64;
        if shift == 0 {
            self.words.push(0);
        }
        *self.words.last_mut().expect("pushed") |= v << shift;
        if shift + width as usize > 64 {
            self.words.push(v >> (64 - shift));
        }
        self.bits += width as usize;
    }

    fn close(&mut self) {
        let block = std::mem::take(&mut self.block);
        let (base, slope, width) = fit(&block);
        self.headers
            .extend([base, slope, (self.bits as u64) << 8 | u64::from(width)]);
        for (i, &v) in block.iter().enumerate() {
            self.put(offset(v, base, slope, i), width);
        }
        self.block = block;
        self.block.clear();
    }

    /// The values as a column held in memory.
    pub(crate) fn into_blocked(self) -> Blocked {
        let mut w = Writer::default();
        self.finish(&mut w);
        let mut r = Reader::new(Bytes::from_vec(w.buf)).expect("aligned");
        Blocked::read(&mut r, false).expect("well formed")
    }

    pub(crate) fn finish(mut self, w: &mut Writer) {
        if !self.block.is_empty() {
            self.close();
        }
        w.u64(self.len as u64);
        w.column(&self.headers);
        w.column(&self.words);
    }
}

/// The base, slope and offset width of a block: a line through it, or its minimum when the line's
/// offsets would not fit in 64 bits.
fn fit(block: &[u64]) -> (u64, u64, u32) {
    let (first, last) = (block[0], block[block.len() - 1]);
    let slope = if block.len() > 1 && last >= first {
        (last - first) / (block.len() as u64 - 1)
    } else {
        0
    };
    let line = |slope: u64| {
        let (lo, hi) =
            block
                .iter()
                .enumerate()
                .fold((i128::MAX, i128::MIN), |(lo, hi), (i, &v)| {
                    let r = i128::from(v) - i128::from(first) - i128::from(slope) * i as i128;
                    (lo.min(r), hi.max(r))
                });
        (lo, 128 - (hi - lo).leading_zeros())
    };
    let (slope, (lo, width)) = match line(slope) {
        (_, width) if width > 64 => (0, line(0)),
        fitted => (slope, fitted),
    };
    ((i128::from(first) + lo) as u64, slope, width)
}

fn offset(v: u64, base: u64, slope: u64, i: usize) -> u64 {
    v.wrapping_sub(base)
        .wrapping_sub(slope.wrapping_mul(i as u64))
}

/// Columns of values, each value pushed to one of `columns`, built in two passes over `pushes` so
/// each column holds exactly its packed form.
pub(crate) fn columns_with<I: Iterator<Item = (usize, u64)>>(
    columns: usize,
    pushes: impl Fn() -> I,
) -> Vec<Blocked> {
    let mut blocks = vec![Vec::with_capacity(BLOCK); columns];
    let (mut headers, mut bits, mut lens) = (
        vec![Vec::new(); columns],
        vec![0u64; columns],
        vec![0; columns],
    );
    let mut fitted = |c: usize, block: &[u64]| {
        let (base, slope, width) = fit(block);
        headers[c].extend([base, slope, bits[c] << 8 | u64::from(width)]);
        bits[c] += block.len() as u64 * u64::from(width);
    };
    for (c, v) in pushes() {
        blocks[c].push(v);
        lens[c] += 1;
        if blocks[c].len() == BLOCK {
            fitted(c, &blocks[c]);
            blocks[c].clear();
        }
    }
    for (c, block) in blocks.iter_mut().enumerate().filter(|(_, b)| !b.is_empty()) {
        fitted(c, block);
        block.clear();
    }
    let mut words: Vec<Vec<u64>> = bits
        .iter()
        .map(|&b| vec![0; b.div_ceil(64) as usize])
        .collect();
    let mut at = vec![0; columns];
    let mut packed = |c: usize, block: &[u64]| {
        let h = &headers[c][at[c] * 3..at[c] * 3 + 3];
        let (start, width) = ((h[2] >> 8) as usize, (h[2] & 0xff) as u32);
        for (i, &v) in block.iter().enumerate() {
            put(
                &mut words[c],
                start + i * width as usize,
                offset(v, h[0], h[1], i),
                width,
            );
        }
        at[c] += 1;
    };
    for (c, v) in pushes() {
        blocks[c].push(v);
        if blocks[c].len() == BLOCK {
            packed(c, &blocks[c]);
            blocks[c].clear();
        }
    }
    for (c, block) in blocks.iter().enumerate().filter(|(_, b)| !b.is_empty()) {
        packed(c, block);
    }
    (headers.into_iter().zip(words).zip(lens))
        .map(|((headers, words), len)| Blocked {
            len,
            headers: Column::from_words(headers),
            bits: Column::from_words(words),
        })
        .collect()
}

/// `v`, of `width` bits, at bit `at` of `words`.
fn put(words: &mut [u64], at: usize, v: u64, width: u32) {
    if width == 0 {
        return;
    }
    let (word, shift) = (at / 64, at % 64);
    words[word] |= v << shift;
    if shift + width as usize > 64 {
        words[word + 1] |= v >> (64 - shift);
    }
}

impl Blocked {
    /// Writes `len` values; `values` is called twice, for the block headers and then the bits, so
    /// the bits stream out instead of being held.
    pub(crate) fn write_with<I: Iterator<Item = u64>>(
        w: &mut Writer,
        len: usize,
        values: impl Fn() -> I,
    ) {
        let mut headers = Vec::with_capacity(len.div_ceil(BLOCK) * 3);
        let mut bits = 0u64;
        for_blocks(values(), |block| {
            let (base, slope, width) = fit(block);
            headers.extend([base, slope, bits << 8 | u64::from(width)]);
            bits += block.len() as u64 * u64::from(width);
        });
        debug_assert_eq!(headers.len(), len.div_ceil(BLOCK) * 3);
        w.u64(len as u64);
        w.column(&headers);
        let mut out = BitWriter::new(w);
        let mut b = 0;
        for_blocks(values(), |block| {
            let (base, slope, width) = (headers[b], headers[b + 1], (headers[b + 2] & 0xff) as u32);
            b += 3;
            for (i, &v) in block.iter().enumerate() {
                let v = offset(v, base, slope, i);
                if width == 64 {
                    out.push(v & u64::from(u32::MAX), 32);
                    out.push(v >> 32, 32);
                } else if width > 0 {
                    out.push(v, width);
                }
            }
        });
        out.finish();
    }

    #[cfg(test)]
    fn write(w: &mut Writer, values: &[u64]) {
        Self::write_with(w, values.len(), || values.iter().copied());
    }

    /// With `full`, every block's header is checked too; reads never go out of bounds either way.
    pub(crate) fn read(r: &mut Reader, full: bool) -> Result<Self, Error> {
        let corrupt = || Error::Corrupt("invalid blocked column".into());
        let len = usize::try_from(r.u64()?).map_err(|_| corrupt())?;
        let (headers, bits): (Column<u64>, Column<u64>) = (r.column()?, r.column()?);
        let blocks = len.div_ceil(BLOCK);
        let available = bits.len() as u64 * 64;
        let valid = headers.len() == blocks * 3
            && (!full
                || headers.as_slice().chunks(3).enumerate().all(|(b, h)| {
                    let (start, width) = (h[2] >> 8, h[2] & 0xff);
                    let count = (len - b * BLOCK).min(BLOCK) as u64;
                    width <= 64 && start + count * width <= available
                }));
        valid
            .then_some(Self { len, headers, bits })
            .ok_or_else(corrupt)
    }

    pub(crate) fn len(&self) -> usize {
        self.len
    }

    #[inline]
    pub(crate) fn get(&self, i: usize) -> u64 {
        let headers = self.headers.as_slice();
        let h = &headers[i / BLOCK * 3..i / BLOCK * 3 + 3];
        let (line, meta) = (
            h[0].wrapping_add(h[1].wrapping_mul((i % BLOCK) as u64)),
            h[2],
        );
        let width = (meta & 0xff).min(64) as usize;
        if width == 0 {
            return line;
        }
        let words = self.bits.as_slice();
        let at = ((meta >> 8) as usize).saturating_add(i % BLOCK * width);
        let word = |w: usize| words.get(w).copied().unwrap_or(0);
        let mut v = word(at / 64) >> (at % 64);
        if at % 64 + width > 64 {
            v |= word(at / 64 + 1) << (64 - at % 64);
        }
        line.wrapping_add(v & (u64::MAX >> (64 - width)))
    }

    pub(crate) fn iter(&self) -> impl ExactSizeIterator<Item = u64> + '_ {
        (0..self.len).map(|i| self.get(i))
    }

    /// Position of `v` in a sorted column.
    pub(crate) fn binary_search(&self, v: u64) -> Result<usize, usize> {
        let (mut lo, mut hi) = (0, self.len);
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            match self.get(mid).cmp(&v) {
                std::cmp::Ordering::Less => lo = mid + 1,
                std::cmp::Ordering::Equal => return Ok(mid),
                std::cmp::Ordering::Greater => hi = mid,
            }
        }
        Err(lo)
    }

    pub(crate) fn size(&self) -> usize {
        8 + (self.headers.len() + self.bits.len()) * 8
    }
}

fn for_blocks(values: impl Iterator<Item = u64>, mut each: impl FnMut(&[u64])) {
    let mut block = Vec::with_capacity(BLOCK);
    for v in values {
        block.push(v);
        if block.len() == BLOCK {
            each(&block);
            block.clear();
        }
    }
    if !block.is_empty() {
        each(&block);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_roundtrip() {
        let mut state = 3u64;
        let mut next = || {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            state
        };
        let cases: Vec<Vec<u64>> = vec![
            vec![],
            vec![5],
            vec![7; 300],
            (0..1000).map(|i| i * 11 + next() % 7).collect(),
            (0..700).map(|_| next()).collect(),
            vec![0, u64::MAX, 1, u64::MAX - 1],
        ];
        for values in cases {
            let mut w = Writer::default();
            Blocked::write(&mut w, &values);
            let mut r = Reader::new(Bytes::from_vec(std::mem::take(&mut w.buf))).unwrap();
            let column = Blocked::read(&mut r, true).unwrap();
            r.finish().unwrap();
            assert_eq!(column.iter().collect::<Vec<_>>(), values);
            let mut one = BlockedWriter::default();
            values.iter().for_each(|&v| one.push(v));
            let mut streamed = Writer::default();
            one.finish(&mut streamed);
            let mut w = Writer::default();
            Blocked::write(&mut w, &values);
            assert_eq!(streamed.buf, w.buf);
            let columns = columns_with(3, || values.iter().enumerate().map(|(i, &v)| (i % 3, v)));
            for (c, column) in columns.iter().enumerate() {
                let expected: Vec<u64> = values.iter().skip(c).step_by(3).copied().collect();
                assert_eq!(column.iter().collect::<Vec<_>>(), expected);
            }
        }
    }
}
