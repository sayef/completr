//! Title keys read back from the texts: documents sorted by their lowercased text.

use super::*;
use crate::codec::BitWriter;

/// Positions between two sampled keys.
const SAMPLE: usize = 64;

/// Positions, or nodes, under one node of the popularity tree.
const FANOUT: usize = 64;

/// Locals in title-key order, equal keys by local, with the key of every `SAMPLE`-th position, and a
/// tree of the most popular position under each node, for the most popular titles of a range.
pub(crate) struct Titles {
    order: Packed,
    sample_ends: Column<u32>,
    samples: Bytes,
    /// Per level, bottom first, each node's best `rank`; a level-0 node holds `FANOUT` positions.
    best: Vec<Column<u64>>,
}

/// A weight as an unsigned integer in the same order.
pub(crate) fn weight_order(weight: f32) -> u32 {
    let bits = weight.to_bits();
    if bits >> 31 == 1 {
        !bits
    } else {
        bits | 1 << 31
    }
}

/// A position's rank: a heavier weight first, then an earlier position.
fn rank(weight: f32, pos: usize) -> u64 {
    u64::from(weight_order(weight)) << 32 | u64::from(u32::MAX - pos as u32)
}

/// A title order and its sampled keys, as written.
#[derive(Default)]
pub(super) struct TitleOrder {
    order: Vec<u32>,
    sample_ends: Vec<u32>,
    samples: Vec<u8>,
}

impl TitleOrder {
    /// Appends `locals`, all titled `key`.
    fn push(&mut self, key: &[u8], locals: impl IntoIterator<Item = u32>) -> Result<(), Error> {
        for local in locals {
            if self.order.len().is_multiple_of(SAMPLE) {
                self.samples.extend_from_slice(key);
                self.sample_ends.push(offset(self.samples.len())?);
            }
            self.order.push(local);
        }
        Ok(())
    }
}

/// The popularity tree above its bottom level.
fn write_levels(w: &mut Writer, mut level: Vec<u64>) {
    let mut levels = vec![];
    while !level.is_empty() {
        let up: Vec<u64> = level
            .chunks(FANOUT)
            .map(|c| *c.iter().max().expect("a chunk"))
            .collect();
        let top = level.len() == 1;
        levels.push(std::mem::replace(&mut level, up));
        if top {
            break;
        }
    }
    w.u64(levels.len() as u64);
    for level in &levels {
        w.column(level);
    }
}

impl Titles {
    /// `weight(local)` is a document's weight.
    pub(super) fn write(w: &mut Writer, titles: &TitleOrder, weight: impl Fn(u32) -> f32) {
        Packed::write(w, &titles.order);
        w.column(&titles.sample_ends);
        w.bytes(&titles.samples);
        let level = titles
            .order
            .chunks(FANOUT)
            .enumerate()
            .map(|(c, locals)| {
                let ranks = locals.iter().enumerate();
                ranks
                    .map(|(i, &l)| rank(weight(l), c * FANOUT + i))
                    .max()
                    .expect("a chunk")
            })
            .collect();
        write_levels(w, level);
    }

    pub(super) fn read(r: &mut Reader, n: usize) -> Result<Self, Error> {
        let (order, sample_ends, samples) = (Packed::read(r)?, r.column()?, r.bytes()?);
        let corrupt = || Error::Corrupt("invalid title order".into());
        let levels = r.u64()?;
        if levels > 16 {
            return Err(corrupt());
        }
        let best = (0..levels)
            .map(|_| r.column())
            .collect::<Result<Vec<Column<u64>>, _>>()?;
        let mut width = n;
        let shaped = best.iter().all(|level| {
            width = width.div_ceil(FANOUT);
            level.len() == width
        }) && best.last().is_none_or(|l| l.len() == 1);
        let titles = Self {
            order,
            sample_ends,
            samples,
            best,
        };
        let ends = titles.sample_ends.as_slice();
        let valid = shaped
            && titles.order.len == n
            && ends.len() == n.div_ceil(SAMPLE)
            && ends
                .last()
                .is_none_or(|&e| e as usize == titles.samples.as_ref().len());
        valid
            .then_some(titles)
            .ok_or_else(|| Error::Corrupt("invalid title order".into()))
    }

    pub(super) fn size(&self) -> usize {
        let tree: usize = self.best.iter().map(|l| l.len() * 8).sum();
        self.order.size() + self.sample_ends.len() * 4 + self.samples.as_ref().len() + tree
    }

    fn sample(&self, i: usize) -> &[u8] {
        let ends = self.sample_ends.as_slice();
        let start = i.checked_sub(1).map_or(0, |p| ends[p] as usize);
        self.samples
            .as_ref()
            .get(start..ends[i] as usize)
            .unwrap_or(&[])
    }

    /// The build's order: `keys` sorted, each entry's value a local.
    pub(super) fn order_of(mut keys: KeyArena) -> Result<TitleOrder, Error> {
        let groups = keys.sorted_groups();
        let mut titles = TitleOrder::default();
        for &(start, end) in &groups {
            titles.push(
                keys.key(start),
                keys.entries[start..end].iter().map(|e| e.2),
            )?;
        }
        Ok(titles)
    }

    /// Writes the parts' orders merged by key, locals mapped by `place(part, local)`, as they come:
    /// as [`Titles::write`] would write the merged order of all `n` documents.
    pub(super) fn merge(
        w: &mut Writer,
        parts: &[Part],
        n: usize,
        place: impl Fn(usize, u32) -> Option<u32>,
    ) -> Result<(), Error> {
        use std::cmp::Reverse;
        use std::collections::BinaryHeap;
        let mut cursors: Vec<TitleCursor> =
            parts.iter().map(|p| p.segment.title_cursor(b"")).collect();
        // The parts' next titles, smallest first.
        let mut heap: BinaryHeap<Reverse<(Vec<u8>, usize)>> = BinaryHeap::new();
        for (i, c) in cursors.iter_mut().enumerate() {
            if c.advance() {
                heap.push(Reverse((c.key().to_vec(), i)));
            }
        }
        let width = (64 - n.saturating_sub(1).leading_zeros()).max(1);
        w.u64(u64::from(width) | (n as u64) << 8);
        let mut bits = BitWriter::new(w);
        let (mut sample_ends, mut samples) = (Vec::new(), Vec::new());
        let (mut level, mut best, mut pos) = (Vec::with_capacity(n.div_ceil(FANOUT)), 0, 0);
        let (mut group, mut taken) = (Vec::new(), Vec::new());
        while let Some(Reverse((key, i))) = heap.pop() {
            taken.clear();
            taken.push(i);
            while heap.peek().is_some_and(|Reverse((k, _))| *k == key) {
                taken.push(heap.pop().expect("peeked").0 .1);
            }
            group.clear();
            for &part in &taken {
                let segment = parts[part].segment;
                group.extend(
                    cursors[part]
                        .locals()
                        .iter()
                        .filter_map(|&l| Some((place(part, l)?, segment.weight(l as usize)))),
                );
            }
            group.sort_unstable_by_key(|&(placed, _)| placed);
            for &(placed, weight) in &group {
                if pos == n {
                    return Err(Error::Corrupt("titles out of step with documents".into()));
                }
                if pos % SAMPLE == 0 {
                    samples.extend_from_slice(&key);
                    sample_ends.push(offset(samples.len())?);
                }
                bits.push(u64::from(placed), width);
                best = best.max(rank(weight, pos));
                pos += 1;
                if pos % FANOUT == 0 {
                    level.push(std::mem::take(&mut best));
                }
            }
            for &part in &taken {
                if cursors[part].advance() {
                    heap.push(Reverse((cursors[part].key().to_vec(), part)));
                }
            }
        }
        if pos != n {
            return Err(Error::Corrupt("titles out of step with documents".into()));
        }
        if pos % FANOUT != 0 {
            level.push(best);
        }
        let w = bits.finish();
        w.column(&sample_ends);
        w.bytes(&samples);
        write_levels(w, level);
        Ok(())
    }
}

impl Segment {
    /// The title key of `local`: its lowercased text, terminated.
    pub(crate) fn title_key(&self, local: usize, key: &mut Vec<u8>) {
        key.clear();
        if local < self.len() {
            self.texts.append(local, key);
        }
        if key.is_ascii() {
            key.make_ascii_lowercase();
        } else {
            let lower = text::lower(&String::from_utf8_lossy(key));
            key.clear();
            key.extend_from_slice(lower.as_bytes());
        }
        key.push(TERMINATOR);
    }

    fn title_at(&self, i: usize) -> usize {
        self.titles.order.get(i) as usize
    }

    /// The first position whose key is not below `bound`.
    fn title_lower_bound(&self, bound: &[u8]) -> usize {
        let len = self.titles.order.len;
        // Sampled keys narrow the search to one gap between samples.
        let (mut below, mut above) = (0, self.titles.sample_ends.len());
        while below < above {
            let mid = below + (above - below) / 2;
            if self.titles.sample(mid) < bound {
                below = mid + 1;
            } else {
                above = mid;
            }
        }
        let (mut lo, mut hi) = match below {
            0 => (0, 0),
            b => ((b - 1) * SAMPLE + 1, (b * SAMPLE).min(len)),
        };
        let mut key = Vec::new();
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            self.title_key(self.title_at(mid), &mut key);
            if key.as_slice() < bound {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        lo
    }

    /// Positions of the titles starting with `prefix`.
    pub(crate) fn title_range(&self, prefix: &[u8]) -> std::ops::Range<usize> {
        let start = self.title_lower_bound(prefix);
        let mut successor = prefix.to_vec();
        while successor.last() == Some(&u8::MAX) {
            successor.pop();
        }
        let end = match successor.last_mut() {
            Some(last) => {
                *last += 1;
                self.title_lower_bound(&successor)
            }
            None => self.titles.order.len,
        };
        start..end.max(start)
    }

    /// The title key at position `i`.
    pub(crate) fn title_key_at(&self, i: usize, key: &mut Vec<u8>) {
        self.title_key(self.title_at(i), key);
    }

    /// The first position in `lo..hi` whose key is not below `bound`.
    pub(crate) fn title_lower_bound_in(&self, bound: &[u8], mut lo: usize, mut hi: usize) -> usize {
        let mut key = Vec::new();
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            self.title_key_at(mid, &mut key);
            if key.as_slice() < bound {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        lo
    }

    fn position_rank(&self, pos: usize) -> u64 {
        self.title_local(pos)
            .map_or(0, |local| rank(self.weight(local as usize), pos))
    }

    /// Calls `f(local)` for the titles at positions `range`, heaviest first and then in title order,
    /// until it returns true.
    pub(crate) fn titles_by_weight(
        &self,
        range: std::ops::Range<usize>,
        mut f: impl FnMut(u32) -> bool,
    ) {
        use std::collections::BinaryHeap;
        let best = &self.titles.best;
        let (start, end) = (range.start, range.end.min(self.titles.order.len));
        if start >= end || best.is_empty() {
            return;
        }
        // (rank, level + 1 or 0 for a position, index)
        let mut heap: BinaryHeap<(u64, usize, usize)> = BinaryHeap::new();
        let span = |level: usize| FANOUT.pow(level as u32 + 1);
        // Nodes wholly inside the range go in whole; a node across an edge is opened.
        let mut open = vec![(best.len() - 1, 0usize)];
        while let Some((level, i)) = open.pop() {
            let (lo, hi) = (i * span(level), ((i + 1) * span(level)).min(end));
            if hi <= start || lo >= end {
                continue;
            }
            if start <= lo && hi <= end {
                if let Some(&r) = best[level].as_slice().get(i) {
                    heap.push((r, level + 1, i));
                }
            } else if level == 0 {
                for pos in lo.max(start)..hi {
                    heap.push((self.position_rank(pos), 0, pos));
                }
            } else {
                open.extend((i * FANOUT..(i + 1) * FANOUT).map(|j| (level - 1, j)));
            }
        }
        while let Some((_, level, i)) = heap.pop() {
            match level {
                0 => {
                    if let Some(local) = self.title_local(i) {
                        if f(local) {
                            return;
                        }
                    }
                }
                1 => {
                    for pos in i * FANOUT..((i + 1) * FANOUT).min(end) {
                        heap.push((self.position_rank(pos), 0, pos));
                    }
                }
                _ => {
                    let below = best[level - 2].as_slice();
                    let children = below.iter().enumerate().skip(i * FANOUT).take(FANOUT);
                    heap.extend(children.map(|(j, &r)| (r, level - 1, j)));
                }
            }
        }
    }

    /// The local at title position `i`, if valid.
    pub(crate) fn title_local(&self, i: usize) -> Option<u32> {
        let local = self.title_at(i);
        (local < self.len()).then_some(local as u32)
    }

    /// Titles whose key starts with `prefix`, one key at a time.
    pub(crate) fn title_cursor(&self, prefix: &[u8]) -> TitleCursor<'_> {
        TitleCursor {
            seg: self,
            prefix: prefix.to_vec(),
            at: self.title_lower_bound(prefix),
            key: Vec::new(),
            next: Vec::new(),
            loaded: false,
            locals: Vec::new(),
        }
    }

    /// The locals titled exactly `key`, a term key.
    pub(crate) fn titled(&self, key: &[u8]) -> Vec<u32> {
        let mut cursor = self.title_cursor(key);
        if cursor.advance() && cursor.key() == key {
            cursor.locals
        } else {
            Vec::new()
        }
    }

    /// Whether the order is a permutation sorted by key, then local.
    pub(super) fn titles_sorted(&self) -> bool {
        let n = self.len();
        let mut seen = vec![false; n];
        let (mut previous, mut key) = (Vec::new(), Vec::new());
        let mut previous_local = 0;
        for i in 0..n {
            let local = self.title_at(i);
            if local >= n || std::mem::replace(&mut seen[local], true) {
                return false;
            }
            self.title_key(local, &mut key);
            if i > 0 && (key < previous || key == previous && local < previous_local)
                || i % SAMPLE == 0 && self.titles.sample(i / SAMPLE) != key.as_slice()
            {
                return false;
            }
            std::mem::swap(&mut key, &mut previous);
            previous_local = local;
        }
        true
    }
}

pub(crate) struct TitleCursor<'a> {
    seg: &'a Segment,
    prefix: Vec<u8>,
    /// The next position to read.
    at: usize,
    key: Vec<u8>,
    /// The key at `at`, when `loaded`.
    next: Vec<u8>,
    loaded: bool,
    locals: Vec<u32>,
}

impl TitleCursor<'_> {
    /// Moves to the next key, gathering its locals.
    pub(crate) fn advance(&mut self) -> bool {
        self.locals.clear();
        let (seg, len) = (self.seg, self.seg.titles.order.len);
        if self.at >= len {
            return false;
        }
        if !self.loaded {
            seg.title_key(seg.title_at(self.at), &mut self.next);
        }
        if !self.next.starts_with(&self.prefix) {
            self.at = len;
            return false;
        }
        std::mem::swap(&mut self.key, &mut self.next);
        self.loaded = false;
        loop {
            let local = seg.title_at(self.at);
            if local < seg.len() {
                self.locals.push(local as u32);
            }
            self.at += 1;
            if self.at >= len {
                break;
            }
            seg.title_key(seg.title_at(self.at), &mut self.next);
            if self.next != self.key {
                self.loaded = true;
                break;
            }
        }
        true
    }

    pub(crate) fn key(&self) -> &[u8] {
        &self.key
    }

    pub(crate) fn locals(&self) -> &[u32] {
        &self.locals
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursors_match_a_scan() {
        let words = [
            "Ärzte", "arm", "Zebra", "über", "Ab", "ab", "İst", "a b", "Ω",
        ];
        let docs: Vec<Document> = (0..400)
            .map(|i| {
                let text = format!("{} {}", words[i % words.len()], words[i * 7 % 5]);
                Document::new(i as u64, text, (i * 37 % 11) as f32 / 10.0)
            })
            .collect();
        let seg = Segment::build(docs.clone(), []).unwrap();
        assert!(seg.titles_sorted());
        let mut keys: Vec<(Vec<u8>, u32)> = docs
            .iter()
            .enumerate()
            .map(|(local, d)| (term_key(&text::lower(&d.text)), local as u32))
            .collect();
        keys.sort();
        for prefix in [
            "",
            "a",
            "ab",
            "ab\u{ff}",
            "ä",
            "ärzte z",
            "z",
            "zz",
            "ω",
            "\u{10ffff}",
        ] {
            let mut cursor = seg.title_cursor(prefix.as_bytes());
            let mut got = Vec::new();
            while cursor.advance() {
                got.extend(cursor.locals().iter().map(|&l| (cursor.key().to_vec(), l)));
            }
            let want: Vec<_> = keys
                .iter()
                .filter(|(k, _)| k.starts_with(prefix.as_bytes()))
                .cloned()
                .collect();
            assert_eq!(got, want, "{prefix:?}");
            let ranged: Vec<u32> = seg
                .title_range(prefix.as_bytes())
                .filter_map(|i| seg.title_local(i))
                .collect();
            assert_eq!(
                ranged,
                want.iter().map(|e| e.1).collect::<Vec<_>>(),
                "{prefix:?}"
            );
        }
        for prefix in ["", "a", "ab", "z"] {
            let range = seg.title_range(prefix.as_bytes());
            let mut want: Vec<(u32, usize)> = range
                .clone()
                .map(|p| (weight_order(seg.weight(seg.title_at(p))), p))
                .collect();
            want.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
            let mut got = Vec::new();
            seg.titles_by_weight(range, |local| {
                got.push(local);
                false
            });
            let want: Vec<u32> = want.iter().map(|&(_, p)| seg.title_at(p) as u32).collect();
            assert_eq!(got, want, "{prefix:?}");
        }
        let first = &keys[0].0;
        let titled: Vec<u32> = keys
            .iter()
            .filter(|(k, _)| k == first)
            .map(|e| e.1)
            .collect();
        assert_eq!(seg.titled(first), titled);
    }
}
