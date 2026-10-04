//! Sorted posting lists, delta coded in blocks of `BLOCK` gaps, each block led by its bit width.

use crate::codec::{Column, Reader, Writer};
use crate::Error;

const BLOCK: usize = 128;
const COUNT_BITS: u32 = 5;
const WIDTH_BITS: u32 = 6;

/// Where encoded lists go, bit by bit.
trait Bits {
    fn put(&mut self, v: u64, width: u32);
    fn bits(&self) -> usize;
}

/// Appends a non-empty, ascending list to `out`: its count, then its blocks, the first gap being the
/// first value; returns its first bit.
fn encode(out: &mut impl Bits, list: &[u32]) -> Result<u64, Error> {
    if list.is_empty() || list.windows(2).any(|w| w[0] >= w[1]) {
        return Err(Error::input("postings must be ascending and unique"));
    }
    let start = out.bits() as u64;
    let count = list.len() as u64;
    if count > u64::from(u32::MAX) {
        return Err(Error::input("too many postings for one key"));
    }
    let n = 64 - count.leading_zeros();
    out.put(u64::from(n - 1), COUNT_BITS);
    out.put(count & !(1 << (n - 1)), n - 1);
    let mut prev = 0;
    for block in list.chunks(BLOCK) {
        let mut p = prev;
        let width = block
            .iter()
            .map(|&v| {
                let gap = v - p;
                p = v;
                32 - gap.leading_zeros()
            })
            .max()
            .unwrap_or(0);
        out.put(u64::from(width), WIDTH_BITS);
        for &v in block {
            out.put(u64::from(v - prev), width);
            prev = v;
        }
    }
    Ok(start)
}

/// Lists appended to one bit stream held in memory.
#[derive(Default)]
pub(crate) struct ListWriter {
    words: Vec<u64>,
    bits: usize,
    lists: u64,
}

impl Bits for ListWriter {
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

    fn bits(&self) -> usize {
        self.bits
    }
}

impl ListWriter {
    /// Appends a non-empty, ascending list; returns its first bit.
    pub(crate) fn push(&mut self, list: &[u32]) -> Result<u64, Error> {
        let start = encode(self, list)?;
        self.lists += 1;
        Ok(start)
    }

    pub(crate) fn write(&self, w: &mut Writer) {
        w.u64(self.lists);
        w.u64(self.bits as u64);
        w.column(&self.words);
    }
}

/// Lists written straight to a writer as they come, their counts patched in when finished.
pub(crate) struct ListStream<'w, 's> {
    w: &'w mut Writer<'s>,
    header: usize,
    column: usize,
    word: u64,
    bits: usize,
    lists: u64,
}

impl Bits for ListStream<'_, '_> {
    fn put(&mut self, v: u64, width: u32) {
        if width == 0 {
            return;
        }
        let shift = self.bits % 64;
        self.word |= v << shift;
        self.bits += width as usize;
        if shift + width as usize >= 64 {
            self.w.raw(&self.word.to_le_bytes());
            self.word = if shift == 0 { 0 } else { v >> (64 - shift) };
        }
    }

    fn bits(&self) -> usize {
        self.bits
    }
}

impl<'w, 's> ListStream<'w, 's> {
    pub(crate) fn new(w: &'w mut Writer<'s>) -> Self {
        let header = w.position();
        w.u64(0);
        w.u64(0);
        let column = w.begin_run();
        Self {
            w,
            header,
            column,
            word: 0,
            bits: 0,
            lists: 0,
        }
    }

    pub(crate) fn push(&mut self, list: &[u32]) -> Result<u64, Error> {
        let start = encode(self, list)?;
        self.lists += 1;
        Ok(start)
    }

    pub(crate) fn finish(self) -> &'w mut Writer<'s> {
        if !self.bits.is_multiple_of(64) {
            self.w.raw(&self.word.to_le_bytes());
        }
        self.w.end_run(self.column, self.bits.div_ceil(64));
        self.w.patch_u64(self.header, self.lists);
        self.w.patch_u64(self.header + 8, self.bits as u64);
        self.w
    }
}

pub(crate) struct Lists {
    words: Column<u64>,
    bits: u64,
}

impl Lists {
    pub(crate) fn read(r: &mut Reader, bound: u32, full: bool) -> Result<Self, Error> {
        let (count, bits) = (r.u64()?, r.u64()?);
        let lists = Self {
            words: r.column()?,
            bits,
        };
        let mut valid = bits.div_ceil(64) == lists.words.len() as u64;
        if valid && full {
            // Written back to back: each list must decode, in bounds, up to the next.
            let mut at = 0;
            for _ in 0..count {
                if at >= bits {
                    valid = false;
                    break;
                }
                let mut list = lists.list(at, bound);
                let len = list.left;
                valid &= (&mut list).count() == len as usize && list.bit <= bits as usize;
                at = list.bit as u64;
            }
            valid &= at == bits;
        }
        valid
            .then_some(lists)
            .ok_or_else(|| Error::Corrupt("invalid postings".into()))
    }

    /// The list at bit `start`; one corrupt ends early, at a value of `bound` or more.
    pub(crate) fn list(&self, start: u64, bound: u32) -> List<'_> {
        let mut list = List {
            words: self.words.as_slice(),
            bit: usize::try_from(start).unwrap_or(usize::MAX),
            end: self.bits as usize,
            left: 0,
            block_left: 0,
            width: 0,
            prev: 0,
            bound,
        };
        if list.bit < list.end {
            let n = list.read(COUNT_BITS) as u32 + 1;
            list.left = (1u64 << (n - 1) | list.read(n - 1)).min(u64::from(u32::MAX)) as u32;
        }
        list
    }

    pub(crate) fn size(&self) -> usize {
        24 + self.words.len() * 8
    }
}

pub(crate) struct List<'a> {
    words: &'a [u64],
    bit: usize,
    end: usize,
    left: u32,
    block_left: u32,
    width: u32,
    prev: u32,
    bound: u32,
}

impl List<'_> {
    pub(crate) fn empty() -> Self {
        List {
            words: &[],
            bit: 0,
            end: 0,
            left: 0,
            block_left: 0,
            width: 0,
            prev: 0,
            bound: 0,
        }
    }

    #[inline]
    fn read(&mut self, width: u32) -> u64 {
        if width == 0 {
            return 0;
        }
        let (at, shift) = (self.bit / 64, self.bit % 64);
        let mut v = self.words.get(at).map_or(0, |w| w >> shift);
        if shift + width as usize > 64 {
            v |= self.words.get(at + 1).map_or(0, |w| w << (64 - shift));
        }
        self.bit = self.bit.saturating_add(width as usize);
        v & ((1 << width) - 1)
    }
}

impl Iterator for List<'_> {
    type Item = u32;

    #[inline]
    fn next(&mut self) -> Option<u32> {
        if self.left == 0 {
            return None;
        }
        if self.block_left == 0 {
            self.width = self.read(WIDTH_BITS) as u32;
            self.block_left = BLOCK as u32;
        }
        let gap = self.read(self.width.min(32));
        let v = u64::from(self.prev) + gap;
        if self.width > 32 || self.bit > self.end || v >= u64::from(self.bound) {
            self.left = 0;
            return None;
        }
        self.prev = v as u32;
        self.left -= 1;
        self.block_left -= 1;
        Some(self.prev)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (0, Some(self.left as usize))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::Bytes;

    #[test]
    fn lists_roundtrip() {
        let mut state = 9u64;
        let mut next = || {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (state >> 33) as u32
        };
        let mut lists: Vec<Vec<u32>> = vec![vec![0], vec![0, 1, 2], vec![u32::MAX - 1]];
        for len in [2, 127, 128, 129, 1000] {
            let mut list: Vec<u32> = (0..len).map(|_| next() % 100_000).collect();
            list.sort_unstable();
            list.dedup();
            lists.push(list);
        }
        lists.push((0..5000).map(|i| i * 3).collect());
        let mut writer = ListWriter::default();
        let starts: Vec<u64> = lists.iter().map(|l| writer.push(l).unwrap()).collect();
        assert!(writer.push(&[3, 3]).is_err());
        let mut w = Writer::default();
        writer.write(&mut w);
        let mut r = Reader::new(Bytes::from_vec(std::mem::take(&mut w.buf))).unwrap();
        let read = Lists::read(&mut r, u32::MAX, true).unwrap();
        for (list, &start) in lists.iter().zip(&starts) {
            assert_eq!(&read.list(start, u32::MAX).collect::<Vec<_>>(), list);
        }
        assert_eq!(read.list(starts[1], 2).collect::<Vec<_>>(), [0, 1]);
        let mut streamed = Writer::default();
        let mut stream = ListStream::new(&mut streamed);
        for list in &lists {
            stream.push(list).unwrap();
        }
        stream.finish();
        let mut held = Writer::default();
        writer.write(&mut held);
        assert_eq!(streamed.buf, held.buf);
    }
}
