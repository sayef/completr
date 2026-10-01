//! Integers in blocks of `BLOCK`, each bit packed as offsets from its block's minimum.

use crate::codec::{BitWriter, Column, Reader, Writer};
use crate::Error;

const BLOCK: usize = 128;

pub(crate) struct Blocked {
    len: usize,
    /// Per block, its minimum, then its first bit `<< 8 | width`.
    headers: Column<u64>,
    bits: Column<u64>,
}

impl Blocked {
    /// Writes `len` values; `values` is called twice.
    pub(crate) fn write_with<I: Iterator<Item = u64>>(
        w: &mut Writer,
        len: usize,
        values: impl Fn() -> I,
    ) {
        let mut headers = Vec::with_capacity(len.div_ceil(BLOCK) * 2);
        let mut block = Vec::with_capacity(BLOCK);
        let mut start = 0u64;
        let mut close = |block: &mut Vec<u64>, headers: &mut Vec<u64>| {
            let min = block.iter().copied().min().unwrap_or(0);
            let max = block.iter().copied().max().unwrap_or(0);
            let width = 64 - (max - min).leading_zeros();
            headers.extend([min, start << 8 | u64::from(width)]);
            start += (block.len() * width as usize) as u64;
            block.clear();
        };
        for v in values() {
            block.push(v);
            if block.len() == BLOCK {
                close(&mut block, &mut headers);
            }
        }
        if !block.is_empty() {
            close(&mut block, &mut headers);
        }
        w.u64(len as u64);
        w.column(&headers);
        let mut bits = BitWriter::new(w, start as usize);
        for (i, v) in values().enumerate() {
            let (min, meta) = (headers[i / BLOCK * 2], headers[i / BLOCK * 2 + 1]);
            let width = (meta & 0xff) as u32;
            let offset = v - min;
            if width == 64 {
                bits.push(offset & u64::from(u32::MAX), 32);
                bits.push(offset >> 32, 32);
            } else if width > 0 {
                bits.push(offset, width);
            }
        }
        bits.finish();
    }

    #[cfg(test)]
    fn write(w: &mut Writer, values: &[u64]) {
        Self::write_with(w, values.len(), || values.iter().copied());
    }

    pub(crate) fn read(r: &mut Reader) -> Result<Self, Error> {
        let corrupt = || Error::Corrupt("invalid blocked column".into());
        let len = usize::try_from(r.u64()?).map_err(|_| corrupt())?;
        let (headers, bits): (Column<u64>, Column<u64>) = (r.column()?, r.column()?);
        let blocks = len.div_ceil(BLOCK);
        let available = bits.len() as u64 * 64;
        let valid = headers.len() == blocks * 2
            && headers.as_slice().chunks(2).enumerate().all(|(b, h)| {
                let (start, width) = (h[1] >> 8, h[1] & 0xff);
                let count = (len - b * BLOCK).min(BLOCK) as u64;
                width <= 64 && start + count * width <= available
            });
        valid
            .then_some(Self { len, headers, bits })
            .ok_or_else(corrupt)
    }

    pub(crate) fn len(&self) -> usize {
        self.len
    }

    pub(crate) fn get(&self, i: usize) -> u64 {
        let headers = self.headers.as_slice();
        let (min, meta) = (headers[i / BLOCK * 2], headers[i / BLOCK * 2 + 1]);
        let width = (meta & 0xff) as usize;
        if width == 0 {
            return min;
        }
        let words = self.bits.as_slice();
        let at = (meta >> 8) as usize + i % BLOCK * width;
        let mut v = words[at / 64] >> (at % 64);
        if at % 64 + width > 64 {
            v |= words[at / 64 + 1] << (64 - at % 64);
        }
        min.wrapping_add(v & (u64::MAX >> (64 - width)))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::Bytes;

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
            let column = Blocked::read(&mut r).unwrap();
            r.finish().unwrap();
            assert_eq!(column.iter().collect::<Vec<_>>(), values);
        }
    }
}
