use std::fs::File;
use std::path::Path;
use std::sync::Arc;

use rayon::prelude::*;
use rustc_hash::FxHashMap;

use crate::blocked::{Blocked, BlockedWriter};
use crate::codec::{Bytes, Column, Out, Reader, Writer};
use crate::dict::{Dict, Dictionary};
use crate::elias_fano::EliasFano;
use crate::postings::{List, ListStream, ListWriter, Lists};
use crate::staged::Staged;

mod merge;
mod titles;
mod words;
use crate::trie::Packed;
use crate::vectors::{CarriedCodes, Row, Vectors};
use crate::{fuzzy, text, Alias, AliasKind, Document, Error};
pub(crate) use merge::Part;
pub(crate) use titles::{weight_order, TitleCursor, Titles};
pub(crate) use words::{WordCursor, Words};

const MAGIC: &[u8; 8] = b"COMPLETR";
const VERSION: u64 = 21;
const DOCS_PER_BLOCK: usize = 128;
const ZSTD_LEVEL: i32 = 3;

/// Appended to every key so a key sorts after its extensions, as in marisa's record tries.
pub(crate) const TERMINATOR: u8 = 0xff;

/// A packed FST value holding its single posting directly.
const INLINE: u64 = 1 << 63;

/// Build-time parameters. All segments of one index must share them. Set them with the chainable
/// methods of the same names, e.g. `BuildOptions::default().vector_bits(2)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct BuildOptions {
    /// Words shorter than this are not indexed for infix or fuzzy matching.
    pub min_word_chars: u8,
    pub max_edit_distance: u8,
    /// Only this many leading chars of a word produce fuzzy delete variants.
    pub fuzzy_prefix_chars: u8,
    /// Bits per dimension for stored embeddings: 2, 3 or 4. Fewer bits are smaller and faster,
    /// with lower recall.
    pub vector_bits: u8,
    /// Store aliases in compact tries: about 25 % smaller keys, with lookups
    /// ~1.8x and prefix scans ~4x slower.
    pub compact_keys: bool,
    /// Threads for building: 1 (default) builds sections one after another with the lowest
    /// peak memory; 0 uses all cores, faster but holding every section's buffers at once.
    pub build_threads: usize,
    /// Which structure holds each key set. Chosen per access pattern; overridable for benchmarks only.
    #[doc(hidden)]
    pub layout: Layout,
}

crate::setters!(BuildOptions {
    min_word_chars: u8,
    max_edit_distance: u8,
    fuzzy_prefix_chars: u8,
    vector_bits: u8,
    compact_keys: bool,
    build_threads: usize,
});

impl BuildOptions {
    #[doc(hidden)]
    pub fn layout(mut self, layout: Layout) -> Self {
        self.layout = layout;
        self
    }
}

impl Default for BuildOptions {
    fn default() -> Self {
        Self {
            min_word_chars: 3,
            max_edit_distance: 2,
            fuzzy_prefix_chars: 7,
            vector_bits: 4,
            compact_keys: false,
            build_threads: 1,
            layout: Layout::default(),
        }
    }
}

/// The dictionary behind each key set of a segment.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layout {
    pub aliases: Dictionary,
}

impl Default for Layout {
    /// A trie for the scan-heavy aliases: faster to scan and smaller than an FST.
    fn default() -> Self {
        Self {
            aliases: Dictionary::Trie,
        }
    }
}

impl Layout {
    pub fn uniform(dictionary: Dictionary) -> Self {
        Self {
            aliases: dictionary,
        }
    }
}

impl BuildOptions {
    /// The layout segments are written with.
    pub(crate) fn effective_layout(&self) -> Layout {
        if self.compact_keys {
            Layout::uniform(Dictionary::CompactTrie)
        } else {
            self.layout
        }
    }

    /// Whether segments built with `self` and `other` can be searched together.
    pub(crate) fn compatible(&self, other: &Self) -> bool {
        (
            self.min_word_chars,
            self.max_edit_distance,
            self.fuzzy_prefix_chars,
        ) == (
            other.min_word_chars,
            other.max_edit_distance,
            other.fuzzy_prefix_chars,
        )
    }
}

pub(crate) fn term_key(s: &str) -> Vec<u8> {
    let mut key = Vec::with_capacity(s.len() + 1);
    key.extend_from_slice(s.as_bytes());
    key.push(TERMINATOR);
    key
}

/// A key's postings: one inline, or a delta coded list.
pub(crate) struct Postings<'a> {
    one: Option<u32>,
    many: List<'a>,
}

impl Iterator for Postings<'_> {
    type Item = u32;

    #[inline]
    fn next(&mut self) -> Option<u32> {
        self.one.take().or_else(|| self.many.next())
    }
}

/// An FST from key to postings. Values inline a single posting or hold its list's first bit.
pub(crate) struct Keyed {
    pub(crate) map: Dict,
    lists: Lists,
    bound: u32,
}

impl Keyed {
    /// Writes `arena`'s keys with their postings, one packed value per key.
    fn write_packed(w: &mut Writer, kind: Dictionary, mut arena: KeyArena) -> Result<(), Error> {
        let groups = arena.sorted_groups();
        let arena = &arena;
        let groups = groups.iter().map(|&(start, end)| {
            (
                arena.key(start),
                arena.entries[start..end].iter().map(|e| e.2),
            )
        });
        Self::write_groups(w, kind, groups)
    }

    /// Writes keys, in byte order and unique, with their sorted, unique postings.
    fn write_groups<'k, P: IntoIterator<Item = u32>>(
        w: &mut Writer,
        kind: Dictionary,
        groups: impl Iterator<Item = (&'k [u8], P)>,
    ) -> Result<(), Error> {
        let mut keys = Vec::new();
        let mut packed_values = Vec::new();
        let mut lists = ListWriter::default();
        let mut postings = Vec::new();
        for (key, group) in groups {
            keys.push(key);
            postings.clear();
            postings.extend(group);
            let value = match postings.as_slice() {
                [one] => INLINE | u64::from(*one),
                many => lists.push(many)?,
            };
            packed_values.push(value);
        }
        Dict::write(w, kind, &keys, &packed_values)?;
        lists.write(w);
        Ok(())
    }

    fn read(r: &mut Reader, kind: Dictionary, bound: u32, full: bool) -> Result<Self, Error> {
        Ok(Self {
            map: Dict::read(r, kind)?,
            lists: Lists::read(r, bound, full)?,
            bound,
        })
    }

    /// Postings for an FST value; out-of-range values from a corrupt file yield none.
    pub(crate) fn postings(&self, value: u64) -> Postings<'_> {
        if value & INLINE != 0 {
            let one = u32::try_from(value & !INLINE)
                .ok()
                .filter(|&p| p < self.bound);
            return Postings {
                one,
                many: List::empty(),
            };
        }
        Postings {
            one: None,
            many: self.lists.list(value, self.bound),
        }
    }

    pub(crate) fn get(&self, key: &[u8]) -> Option<Postings<'_>> {
        self.map.get(key).map(|value| self.postings(value))
    }
}

/// Delete variants hashed into buckets of word groups, each group the words sharing the chars the
/// variants come from; a bucket can also hold groups of other variants, which the caller discards.
pub(crate) struct Variants {
    buckets: u64,
    words: u64,
    /// A bit per word, set for the first of each group.
    starts: Column<u64>,
    /// `bucket * words + ordinal` of each group's first word, sorted.
    entries: EliasFano,
}

impl Variants {
    /// About eight groups per bucket at the default settings.
    fn buckets_for(groups: usize) -> u32 {
        (groups as u32).saturating_mul(3).max(1)
    }

    fn bucket(variant: &[u8], buckets: u64) -> u64 {
        let h = xxhash_rust::xxh3::xxh3_64(variant);
        ((u128::from(h) * u128::from(buckets)) >> 64) as u64
    }

    /// Entries held at once while writing.
    const CHUNK: u32 = 1 << 24;

    /// Writes the variants of the groups starting at `starts` (then the word count); `variants(group,
    /// f)` calls `f` with each variant of a group. Groups are counted per bucket, then the buckets
    /// are filled a range at a time, so at most `CHUNK` entries are held.
    fn write(
        w: &mut Writer,
        buckets: u32,
        starts: &[u32],
        variants: impl Fn(usize, &mut dyn FnMut(&[u8])) + Sync,
    ) {
        use std::sync::atomic::{AtomicU32, Ordering::Relaxed};
        let groups = starts.len() - 1;
        let words = u64::from(starts[groups]);
        let mut bits = vec![0u64; (words as usize).div_ceil(64)];
        for &s in &starts[..groups] {
            bits[s as usize / 64] |= 1 << (s % 64);
        }
        w.u64(u64::from(buckets));
        w.u64(words);
        w.column(&bits);
        drop(bits);

        let counts: Vec<AtomicU32> = (0..=buckets).map(|_| AtomicU32::new(0)).collect();
        (0..groups).into_par_iter().for_each(|group| {
            variants(group, &mut |v| {
                counts[Self::bucket(v, u64::from(buckets)) as usize + 1].fetch_add(1, Relaxed);
            })
        });
        let mut offsets: Vec<u32> = counts.into_iter().map(AtomicU32::into_inner).collect();
        for i in 1..offsets.len() {
            offsets[i] += offsets[i - 1];
        }
        let mut entries = EliasFano::writer(
            w,
            u64::from(buckets) * words,
            offsets[buckets as usize] as usize,
        );
        // One buffer serves every range, so the ranges reuse the same memory.
        let (mut slots, mut next): (Vec<u32>, Vec<u32>) = (Vec::new(), Vec::new());
        let atomic = |v: &mut [u32]| -> &[AtomicU32] {
            // SAFETY: `AtomicU32` has the layout of `u32`, and the slice is borrowed exclusively.
            unsafe { &*(v as *mut [u32] as *const [AtomicU32]) }
        };
        let mut lo = 0;
        while lo < buckets as usize {
            let base = offsets[lo];
            let hi = (offsets.partition_point(|&o| o <= base.saturating_add(Self::CHUNK)) - 1)
                .clamp(lo + 1, buckets as usize);
            let size = (offsets[hi] - base) as usize;
            if slots.len() < size {
                slots.resize(size, 0);
            }
            next.clear();
            next.extend(offsets[lo..hi].iter().map(|&o| o - base));
            {
                let (held, cursors) = (atomic(&mut slots[..size]), atomic(&mut next));
                (0..groups).into_par_iter().for_each(|group| {
                    variants(group, &mut |v| {
                        let bucket = Self::bucket(v, u64::from(buckets)) as usize;
                        if (lo..hi).contains(&bucket) {
                            let slot = cursors[bucket - lo].fetch_add(1, Relaxed);
                            held[slot as usize].store(group as u32, Relaxed);
                        }
                    })
                });
            }
            for bucket in lo..hi {
                let held = &mut slots
                    [(offsets[bucket] - base) as usize..(offsets[bucket + 1] - base) as usize];
                held.sort_unstable();
                for (i, &group) in held.iter().enumerate() {
                    if i == 0 || held[i - 1] != group {
                        entries.push(bucket as u64 * words + u64::from(starts[group as usize]));
                    }
                }
            }
            lo = hi;
        }
        entries.finish();
    }

    fn read(r: &mut Reader, word_bound: u32, full: bool) -> Result<Self, Error> {
        let (buckets, words) = (r.u64()?, r.u64()?);
        let starts: Column<u64> = r.column()?;
        let entries = EliasFano::read(r, full)?;
        let tail = words % 64;
        let valid = buckets > 0
            && buckets.checked_mul(words).is_some()
            && words == u64::from(word_bound)
            && starts.len() as u64 == words.div_ceil(64)
            && starts.as_slice().first().is_none_or(|w| w & 1 == 1)
            && (tail == 0 || starts.as_slice().last().is_none_or(|w| w >> tail == 0));
        valid
            .then_some(Self {
                buckets,
                words,
                starts,
                entries,
            })
            .ok_or_else(|| Error::Corrupt("invalid variants".into()))
    }

    /// The first ordinal of each group in `variant`'s bucket.
    pub(crate) fn get(&self, variant: &[u8]) -> impl Iterator<Item = u32> + '_ {
        let start = Self::bucket(variant, self.buckets) * self.words;
        self.entries
            .range(start, start + self.words)
            .map(move |v| (v - start) as u32)
    }

    /// Ordinals of the words in the group starting at `first`.
    #[inline]
    pub(crate) fn group_at(&self, first: u32) -> std::ops::Range<u32> {
        let bits = self.starts.as_slice();
        let next = first as usize + 1;
        let mut at = next / 64;
        let mut word = bits.get(at).map_or(0, |w| w & (u64::MAX << (next % 64)));
        while word == 0 {
            at += 1;
            match bits.get(at) {
                Some(&w) => word = w,
                None => return first..self.words as u32,
            }
        }
        first..(at * 64 + word.trailing_zeros() as usize) as u32
    }

    fn size(&self) -> usize {
        16 + self.starts.len() * 8 + self.entries.size()
    }
}

/// Document weights: 16-bit codes into their distinct values when there are few enough, else the
/// values.
struct Weights {
    values: Column<f32>,
    codes: Option<Column<u16>>,
}

impl Weights {
    const MAX_DISTINCT: usize = 1 << 16;

    fn write(w: &mut Writer, n: usize, weight: impl Fn(usize) -> f32) {
        let mut codes: FxHashMap<u32, u32> = FxHashMap::default();
        for i in 0..n {
            codes.insert(weight(i).to_bits(), 0);
            if codes.len() > Self::MAX_DISTINCT {
                break;
            }
        }
        if codes.len() > Self::MAX_DISTINCT || codes.len() * 2 >= n {
            w.u64(0);
            w.column_with(n, (0..n).map(weight));
            return;
        }
        let mut values: Vec<u32> = codes.keys().copied().collect();
        values.sort_unstable();
        for (code, &bits) in values.iter().enumerate() {
            codes.insert(bits, code as u32);
        }
        w.u64(1);
        w.column_with(values.len(), values.iter().map(|&b| f32::from_bits(b)));
        w.column_with(n, (0..n).map(|i| codes[&weight(i).to_bits()] as u16));
    }

    fn read(r: &mut Reader, n: usize, full: bool) -> Result<Self, Error> {
        let coded = r.u64()?;
        let values: Column<f32> = r.column()?;
        let codes = match coded {
            0 => None,
            1 => Some(r.column::<u16>()?),
            _ => return Err(Error::Corrupt("invalid weights".into())),
        };
        let valid = match &codes {
            None => values.len() == n,
            Some(codes) => {
                codes.len() == n
                    && (!full
                        || codes
                            .as_slice()
                            .iter()
                            .all(|&c| (c as usize) < values.len()))
            }
        } && (!full || values.as_slice().iter().all(|w| w.is_finite()));
        valid
            .then_some(Self { values, codes })
            .ok_or_else(|| Error::Corrupt("invalid weights".into()))
    }

    #[inline]
    fn get(&self, i: usize) -> f32 {
        let values = self.values.as_slice();
        match &self.codes {
            Some(codes) => values
                .get(codes.as_slice()[i] as usize)
                .copied()
                .unwrap_or(0.0),
            None => values[i],
        }
    }

    fn size(&self) -> usize {
        8 + self.values.len() * 4 + self.codes.as_ref().map_or(0, |c| c.len() * 2)
    }
}

/// The 16 bytes of a lowercase, hyphenated UUID, which formats back to the same text.
fn parse_uuid(s: &[u8]) -> Option<[u8; 16]> {
    if s.len() != 36 || [8, 13, 18, 23].iter().any(|&i| s[i] != b'-') {
        return None;
    }
    let digit = |c: u8| match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        _ => None,
    };
    let mut hex = s.iter().copied().filter(|&c| c != b'-');
    let mut out = [0u8; 16];
    for byte in &mut out {
        *byte = digit(hex.next()?)? << 4 | digit(hex.next()?)?;
    }
    Some(out)
}

fn format_uuid(bytes: &[u8], out: &mut Vec<u8>) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for (i, &b) in bytes.iter().enumerate() {
        if matches!(i, 4 | 6 | 8 | 10) {
            out.push(b'-');
        }
        out.extend([HEX[(b >> 4) as usize], HEX[(b & 15) as usize]]);
    }
}

/// Documents' string keys: 16 bytes each when every key is a lowercase UUID, else FSST.
enum KeyColumn {
    Uuids(Column<u8>),
    Fsst(Box<FsstColumn>),
}

impl KeyColumn {
    /// Whether no document has a key.
    fn is_empty(&self) -> bool {
        matches!(self, Self::Fsst(column) if column.data.as_ref().is_empty())
    }

    /// The `i`th key's 16 bytes, when keys are stored as UUIDs.
    fn uuid(&self, i: usize) -> Option<&[u8]> {
        match self {
            Self::Uuids(bytes) => bytes.as_slice().get(i * 16..i * 16 + 16),
            Self::Fsst(_) => None,
        }
    }

    /// As [`KeyColumn::write`] for keys that are all UUIDs, given as their 16 bytes.
    fn write_uuids<'a>(w: &mut Writer, len: usize, item: impl Fn(usize) -> &'a [u8]) {
        w.u64(1);
        let at = w.begin_run();
        for i in 0..len {
            w.raw(item(i));
        }
        w.end_run(at, len * 16);
    }

    /// Writes `len` keys, `item(i, out)` appending the `i`th to `out`; called twice per key.
    fn write(w: &mut Writer, len: usize, item: impl Fn(usize, &mut Vec<u8>)) -> Result<(), Error> {
        let mut buf = Vec::new();
        let uuids = len > 0
            && (0..len).all(|i| {
                buf.clear();
                item(i, &mut buf);
                parse_uuid(&buf).is_some()
            });
        if !uuids {
            w.u64(0);
            return FsstColumn::write(w, len, item);
        }
        w.u64(1);
        let at = w.begin_run();
        for i in 0..len {
            buf.clear();
            item(i, &mut buf);
            w.raw(&parse_uuid(&buf).expect("checked above"));
        }
        w.end_run(at, len * 16);
        Ok(())
    }

    fn read(r: &mut Reader, n: usize, full: bool) -> Result<Self, Error> {
        match r.u64()? {
            0 => Ok(Self::Fsst(Box::new(FsstColumn::read(r, n, full)?))),
            1 => {
                let bytes: Column<u8> = r.column()?;
                (bytes.len() == n * 16)
                    .then_some(Self::Uuids(bytes))
                    .ok_or_else(|| Error::Corrupt("invalid key column".into()))
            }
            _ => Err(Error::Corrupt("invalid key column".into())),
        }
    }

    fn get(&self, i: usize) -> String {
        match self {
            Self::Fsst(column) => column.get(i),
            Self::Uuids(bytes) => {
                let mut out = Vec::with_capacity(36);
                if let Some(raw) = bytes.as_slice().get(i * 16..i * 16 + 16) {
                    format_uuid(raw, &mut out);
                }
                String::from_utf8(out).expect("hex digits")
            }
        }
    }

    fn size(&self) -> usize {
        match self {
            Self::Fsst(column) => column.size(),
            Self::Uuids(bytes) => bytes.len(),
        }
    }
}

struct StrColumn {
    data: Bytes,
    offsets: Column<u32>,
    /// Whether the data is UTF-8, checked on first read so opening touches none of it.
    utf8: std::sync::OnceLock<bool>,
}

impl StrColumn {
    /// Writes `items`, then their offsets from a second pass, so the offsets are never held.
    fn write<'a>(
        w: &mut Writer,
        items: impl Iterator<Item = &'a str> + Clone,
    ) -> Result<(), Error> {
        let at = w.begin_run();
        let (mut count, mut total) = (0, 0);
        for item in items.clone() {
            w.raw(item.as_bytes());
            total += item.len();
            count += 1;
        }
        u32::try_from(total).map_err(|_| Error::input("text column over 4 GiB"))?;
        w.end_run(at, total);
        let ends = items.scan(0u32, |end, item| {
            *end += item.len() as u32;
            Some(*end)
        });
        w.column_with(count + 1, std::iter::once(0).chain(ends));
        Ok(())
    }

    fn read(r: &mut Reader, full: bool) -> Result<Self, Error> {
        let data = r.bytes()?;
        let offsets = r.column::<u32>()?;
        let o = offsets.as_slice();
        let valid = o.first() == Some(&0)
            && o.last().map(|&l| l as usize) == Some(data.as_ref().len())
            && (!full || o.windows(2).all(|w| w[0] <= w[1]));
        let column = Self {
            data,
            offsets,
            utf8: std::sync::OnceLock::new(),
        };
        (valid && (!full || column.is_utf8()))
            .then_some(column)
            .ok_or_else(|| Error::Corrupt("invalid text column".into()))
    }

    fn is_utf8(&self) -> bool {
        *self
            .utf8
            .get_or_init(|| std::str::from_utf8(self.data.as_ref()).is_ok())
    }

    /// The `i`th string; empty if its offsets are out of range or split a char.
    fn get(&self, i: usize) -> &str {
        if !self.is_utf8() {
            return "";
        }
        // SAFETY: the whole column was just found to be UTF-8.
        let text = unsafe { std::str::from_utf8_unchecked(self.data.as_ref()) };
        let o = self.offsets.as_slice();
        match (o.get(i), o.get(i + 1)) {
            (Some(&start), Some(&end)) => text.get(start as usize..end as usize).unwrap_or(""),
            _ => "",
        }
    }
}

/// Strings compressed with FSST (Boncz et al., VLDB 2020), each decompressed on its own. Columns
/// under `FSST_MIN_BYTES` are stored raw, where a symbol table would not pay for itself.
struct FsstColumn {
    data: Bytes,
    offsets: Blocked,
    symbols: Option<([fsst::Symbol; 255], [u8; 255])>,
}

const FSST_MIN_BYTES: usize = 4096;
const ESCAPE: u8 = 255;

impl FsstColumn {
    /// Writes `len` strings, `item(i, out)` appending the `i`th to `out`. Strings are read one at a
    /// time, to measure, to sample for training and to compress, so they are never all held.
    fn write(w: &mut Writer, len: usize, item: impl Fn(usize, &mut Vec<u8>)) -> Result<(), Error> {
        // Only whether the strings pass the thresholds below matters, so measuring stops there.
        let mut buf = Vec::new();
        let mut total = 0;
        for i in 0..len {
            if total > FSST_SAMPLE_BYTES.max(FSST_MIN_BYTES) {
                break;
            }
            buf.clear();
            item(i, &mut buf);
            total += buf.len();
        }
        let compressor = (total >= FSST_MIN_BYTES).then(|| {
            let sample = fsst_sample(len, total, &item);
            fsst::Compressor::train(&sample.iter().map(Vec::as_slice).collect())
        });
        let (symbols, lengths): (Vec<u64>, Vec<u8>) = match &compressor {
            Some(c) => {
                let parts = c.clone().into_parts();
                (
                    parts.symbols.iter().map(|s| s.to_u64()).collect(),
                    parts.lengths.to_vec(),
                )
            }
            None => (Vec::new(), Vec::new()),
        };
        w.column(&symbols);
        w.column(&lengths);
        // The compressed strings stream out; only their lengths are held.
        let at = w.begin_run();
        let mut offsets = BlockedWriter::default();
        offsets.push(0);
        let mut total = 0;
        for i in 0..len {
            buf.clear();
            item(i, &mut buf);
            let size = match &compressor {
                Some(c) => {
                    let packed = c.compress(&buf);
                    w.raw(&packed);
                    packed.len()
                }
                None => {
                    w.raw(&buf);
                    buf.len()
                }
            };
            total += size;
            offsets.push(total as u64);
        }
        w.end_run(at, total);
        offsets.finish(w);
        Ok(())
    }

    fn read(r: &mut Reader, n: usize, full: bool) -> Result<Self, Error> {
        let bad = || Error::Corrupt("invalid string column".into());
        let symbols: Column<u64> = r.column()?;
        let lengths: Column<u8> = r.column()?;
        let data = r.bytes()?;
        let offsets = Blocked::read(r, full)?;
        let valid = offsets.len() == n + 1
            && offsets.get(0) == 0
            && offsets.get(n) as usize == data.as_ref().len()
            && (!full || (1..=n).all(|i| offsets.get(i - 1) <= offsets.get(i)));
        let symbols = match (symbols.as_slice(), lengths.as_slice()) {
            ([], []) => None,
            (s, l)
                if s.len() == 255 && l.len() == 255 && l.iter().all(|&l| (1..=8).contains(&l)) =>
            {
                let table: [fsst::Symbol; 255] =
                    std::array::from_fn(|i| fsst::Symbol::from_slice(&s[i].to_le_bytes()));
                Some((table, l.try_into().map_err(|_| bad())?))
            }
            _ => return Err(bad()),
        };
        if symbols.is_none() {
            std::str::from_utf8(data.as_ref()).map_err(|_| bad())?;
        }
        valid
            .then_some(Self {
                data,
                offsets,
                symbols,
            })
            .ok_or_else(bad)
    }

    fn get(&self, i: usize) -> String {
        let mut out = Vec::new();
        self.append(i, &mut out);
        String::from_utf8(out)
            .unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned())
    }

    /// Appends the `i`th string's bytes to `out`; nothing for a truncated stream in a corrupt file.
    fn append(&self, i: usize, out: &mut Vec<u8>) {
        let range = self.offsets.get(i) as usize..self.offsets.get(i + 1) as usize;
        let bytes = self.data.as_ref().get(range).unwrap_or(&[]);
        let Some((symbols, lengths)) = &self.symbols else {
            return out.extend_from_slice(bytes);
        };
        // An escape code needs its literal byte; a truncated stream would read past it.
        let mut at = 0;
        while at < bytes.len() {
            at += if bytes[at] == ESCAPE { 2 } else { 1 };
        }
        if at > bytes.len() {
            return;
        }
        let decompressor = fsst::Decompressor::new(symbols, lengths);
        let capacity = decompressor.max_decompression_capacity(bytes);
        out.reserve(capacity);
        let written =
            decompressor.decompress_into(bytes, &mut out.spare_capacity_mut()[..capacity]);
        // SAFETY: `decompress_into` initialised the first `written` spare bytes.
        unsafe { out.set_len(out.len() + written) };
    }

    fn size(&self) -> usize {
        self.data.as_ref().len() + self.offsets.size() + self.symbols.map_or(0, |_| 255 * 9)
    }
}

/// Bytes of text FSST trains its symbol table on; just below its own sampling target, so it trains
/// on exactly this sample.
const FSST_SAMPLE_BYTES: usize = (1 << 14) - 1;

/// Lines of up to 512 bytes drawn as FSST draws its training sample, or every string when they
/// are fewer bytes than the sample.
fn fsst_sample(len: usize, total: usize, item: &impl Fn(usize, &mut Vec<u8>)) -> Vec<Vec<u8>> {
    let read = |i: usize| {
        let mut out = Vec::new();
        item(i, &mut out);
        out
    };
    if total <= FSST_SAMPLE_BYTES {
        return (0..len).map(read).collect();
    }
    let hash = |v: u64| v.wrapping_mul(2971215073) ^ v.wrapping_shr(15);
    let (mut rnd, mut size, mut sample) = (hash(4637947), 0, Vec::new());
    while size < FSST_SAMPLE_BYTES {
        rnd = hash(rnd);
        let start = rnd as usize % len;
        let Some(line) = (start..len)
            .chain(0..start)
            .map(read)
            .find(|l| !l.is_empty())
        else {
            break;
        };
        rnd = hash(rnd);
        let chunk = 512 * (rnd as usize % (1 + (line.len() - 1) / 512));
        let take = 512.min(line.len() - chunk).min(FSST_SAMPLE_BYTES - size);
        sample.push(line[chunk..chunk + take].to_vec());
        size += take;
    }
    sample
}

fn put_varint(buf: &mut Vec<u8>, mut v: usize) {
    while v >= 0x80 {
        buf.push(v as u8 | 0x80);
        v >>= 7;
    }
    buf.push(v as u8);
}

fn get_varint(buf: &[u8], pos: &mut usize) -> Option<usize> {
    let mut v = 0usize;
    for shift in (0..64).step_by(7) {
        let b = *buf.get(*pos)?;
        *pos += 1;
        v |= usize::from(b & 0x7f) << shift;
        if b < 0x80 {
            return Some(v);
        }
    }
    None
}

/// Appends a document's aliases and contexts as a document block stores them.
fn encode_stored(raw: &mut Vec<u8>, aliases: &[Alias], contexts: &[String]) {
    put_varint(raw, aliases.len());
    for alias in aliases {
        raw.push(alias.kind as u8);
        put_varint(raw, alias.text.len());
        raw.extend_from_slice(alias.text.as_bytes());
    }
    put_varint(raw, contexts.len());
    for context in contexts {
        put_varint(raw, context.len());
        raw.extend_from_slice(context.as_bytes());
    }
}

/// A stored document's aliases and contexts.
type StoredDoc = (Vec<Alias>, Vec<String>);

/// Aliases and contexts, zstd-compressed in blocks; only needed to rebuild documents.
struct DocStore {
    blocks: Bytes,
    offsets: Column<u64>,
}

impl DocStore {
    fn write(w: &mut Writer, docs: &Staged) -> Result<(), Error> {
        Self::write_blocks(w, docs.len(), |first, end, raw| {
            for doc in (first..end).map(|l| docs.get(l)) {
                encode_stored(raw, doc.aliases, doc.contexts);
            }
            Ok(())
        })
    }

    /// Writes `docs` documents in blocks; `fill(first, end, raw)` appends the stored parts of
    /// documents `first..end`.
    /// Writes the blocks one at a time, as `fill(first, end, raw)` encodes them, streaming each out.
    fn write_blocks(
        w: &mut Writer,
        docs: usize,
        mut fill: impl FnMut(usize, usize, &mut Vec<u8>) -> Result<(), Error>,
    ) -> Result<(), Error> {
        let at = w.begin_run();
        let (mut raw, mut offsets, mut total) = (Vec::new(), vec![0u64], 0usize);
        for first in (0..docs).step_by(DOCS_PER_BLOCK) {
            raw.clear();
            fill(first, docs.min(first + DOCS_PER_BLOCK), &mut raw)?;
            let packed = zstd::bulk::compress(&raw, ZSTD_LEVEL)?;
            w.raw(&(raw.len() as u32).to_le_bytes());
            w.raw(&packed);
            total += 4 + packed.len();
            offsets.push(total as u64);
        }
        w.end_run(at, total);
        w.column(&offsets);
        Ok(())
    }

    fn read(r: &mut Reader, docs: usize) -> Result<Self, Error> {
        let blocks = r.bytes()?;
        let offsets = r.column::<u64>()?;
        let o = offsets.as_slice();
        let valid = o.len() == docs.div_ceil(DOCS_PER_BLOCK) + 1
            && o.first() == Some(&0)
            && o.last().map(|&l| l as usize) == Some(blocks.as_ref().len())
            && o.windows(2)
                .all(|w| w[0].checked_add(4).is_some_and(|end| end <= w[1]));
        valid
            .then_some(Self { blocks, offsets })
            .ok_or_else(|| Error::Corrupt("invalid document store".into()))
    }

    /// `(aliases, contexts)` of every document in `block`.
    fn block(&self, block: usize) -> Result<Vec<StoredDoc>, Error> {
        let mut raw = Vec::new();
        self.decompress(block, &mut raw, &mut zstd::bulk::Decompressor::new()?)?;
        let docs = parse_block(&raw)?;
        Ok(docs
            .into_iter()
            .map(|(aliases, contexts)| {
                let aliases = aliases
                    .into_iter()
                    .map(|(kind, text)| Alias::new(text, kind))
                    .collect();
                (aliases, contexts.into_iter().map(str::to_owned).collect())
            })
            .collect())
    }

    fn decompress(
        &self,
        block: usize,
        raw: &mut Vec<u8>,
        dctx: &mut zstd::bulk::Decompressor,
    ) -> Result<(), Error> {
        let o = self.offsets.as_slice();
        let data = &self.blocks.as_ref()[o[block] as usize..o[block + 1] as usize];
        let raw_len = u32::from_le_bytes(data[..4].try_into().unwrap()) as usize;
        if raw_len > MAX_BLOCK_BYTES {
            return Err(Error::Corrupt("corrupt document block".into()));
        }
        raw.clear();
        raw.reserve(raw_len);
        if dctx.decompress_to_buffer(&data[4..], raw)? != raw_len {
            return Err(Error::Corrupt("corrupt document block".into()));
        }
        Ok(())
    }
}

const MAX_BLOCK_BYTES: usize = 1 << 28;

type BorrowedDoc<'a> = (Vec<(AliasKind, &'a str)>, Vec<&'a str>);

fn parse_block(raw: &[u8]) -> Result<Vec<BorrowedDoc<'_>>, Error> {
    let corrupt = || Error::Corrupt("corrupt document block".into());
    let text = |pos: &mut usize| -> Result<&str, Error> {
        let len = get_varint(raw, pos).ok_or_else(corrupt)?;
        let bytes = raw
            .get(*pos..pos.checked_add(len).ok_or_else(corrupt)?)
            .ok_or_else(corrupt)?;
        *pos += len;
        std::str::from_utf8(bytes).map_err(|_| corrupt())
    };
    let mut pos = 0;
    let mut out = Vec::with_capacity(DOCS_PER_BLOCK);
    while pos < raw.len() {
        let count = get_varint(raw, &mut pos).ok_or_else(corrupt)?;
        let mut aliases = Vec::with_capacity(count.min(raw.len() - pos));
        for _ in 0..count {
            let kind = if *raw.get(pos).ok_or_else(corrupt)? == 1 {
                AliasKind::Abbreviation
            } else {
                AliasKind::Synonym
            };
            pos += 1;
            aliases.push((kind, text(&mut pos)?));
        }
        let count = get_varint(raw, &mut pos).ok_or_else(corrupt)?;
        let mut contexts = Vec::with_capacity(count.min(raw.len() - pos));
        for _ in 0..count {
            contexts.push(text(&mut pos)?);
        }
        out.push((aliases, contexts));
    }
    Ok(out)
}

/// An immutable batch of documents plus the ids it deletes from older segments.
///
/// Documents are stored in id order; a document's position is its local id.
pub struct Segment {
    data: Bytes,
    config: BuildOptions,
    ids: Blocked,
    weights: Weights,
    /// The index's `max_score` were this segment searched alone, at the default popularity weight.
    max_score: f64,
    /// Per document, its text's length in chars `<< 1`, and whether it is a single word.
    shapes: Packed,
    deletes: Column<u64>,
    docs: DocStore,
    pub(crate) titles: Titles,
    pub(crate) words: Words,
    /// Postings are `local << 1 | kind`.
    pub(crate) aliases: Keyed,
    /// Delete variant to word ordinals.
    pub(crate) variants: Variants,
    pub(crate) vectors: Option<Vectors>,
    texts: FsstColumn,
    /// Empty for documents without a key.
    keys: KeyColumn,
    /// Context tag to locals.
    pub(crate) contexts: Keyed,
}

impl Segment {
    pub fn build(
        documents: impl IntoIterator<Item = Document>,
        deletes: impl IntoIterator<Item = u64>,
    ) -> Result<Self, Error> {
        Self::build_with(BuildOptions::default(), documents, deletes)
    }

    /// Builds a segment. A repeated id keeps its last document.
    pub fn build_with(
        config: BuildOptions,
        documents: impl IntoIterator<Item = Document>,
        deletes: impl IntoIterator<Item = u64>,
    ) -> Result<Self, Error> {
        Self::build_inner(config, documents, deletes, None)
    }

    /// Builds a segment; `codes` carries vectors over from other segments as `id -> (code, scale)`,
    /// with their dimension.
    pub(crate) fn build_inner(
        config: BuildOptions,
        documents: impl IntoIterator<Item = Document>,
        deletes: impl IntoIterator<Item = u64>,
        codes: Option<(usize, &CarriedCodes)>,
    ) -> Result<Self, Error> {
        let mut staged = Staged::default();
        for doc in documents {
            staged.add(doc)?;
        }
        Self::build_staged(config, staged, deletes.into_iter().collect(), codes, None)
    }

    /// Builds from staged documents on the configured pool, into memory or into the file at `path`.
    fn build_staged(
        config: BuildOptions,
        staged: Staged,
        deletes: Vec<u64>,
        codes: Option<(usize, &CarriedCodes)>,
        path: Option<&Path>,
    ) -> Result<Self, Error> {
        crate::vectors::in_pool(config.build_threads, move || {
            Self::build_on_pool(config, staged, deletes, codes, path)
        })
    }

    fn build_on_pool(
        config: BuildOptions,
        mut docs: Staged,
        mut deletes: Vec<u64>,
        codes: Option<(usize, &CarriedCodes)>,
        path: Option<&Path>,
    ) -> Result<Self, Error> {
        crate::vectors::validate_bits(config.vector_bits)?;
        let started = std::time::Instant::now();
        docs.finish()?;
        deletes.sort_unstable();
        deletes.dedup();

        let n = docs.len();
        let mut text_lens = Vec::with_capacity(n);
        let mut single_word = Vec::with_capacity(n);
        let mut title_keys = KeyArena::default();
        let mut alias_keys = KeyArena::default();
        let mut context_keys = KeyArena::default();
        // Per word: postings and occurrences.
        let mut words = WordTable::default();
        let mut lower = String::new();
        for (local, doc) in docs.iter().enumerate() {
            let local = local as u32;
            text::lower_into(doc.text, &mut lower);
            text_lens.push(u16::try_from(text::char_len(&lower)).unwrap_or(u16::MAX));
            single_word.push(text::words(&lower).count() == 1);
            for word in indexed_words(&lower, config.min_word_chars) {
                words.add(word, local);
            }
            title_keys.push(lower.as_bytes(), local)?;
            for alias in doc.aliases {
                alias_keys.push(
                    text::lower(&alias.text).as_bytes(),
                    local << 1 | alias.kind as u32,
                )?;
            }
            let mut contexts: Vec<&str> = doc.contexts.iter().map(String::as_str).collect();
            contexts.sort_unstable();
            contexts.dedup();
            for context in contexts {
                context_keys.push(context.as_bytes(), local)?;
            }
        }

        let words = words.sorted();
        if u32::try_from(words.len()).is_err() {
            return Err(Error::input("too many distinct words for one segment"));
        }

        let mut sink = match path {
            Some(path) => Sink::file(path)?,
            None => {
                // Reserved generously, so the buffer never copies itself while growing; untouched
                // pages cost nothing.
                let text_bytes: usize = docs
                    .iter()
                    .map(|d| d.text.len() + d.key.map_or(0, str::len))
                    .sum();
                Sink::Memory(Vec::with_capacity(
                    text_bytes * 3 + words.len() * 100 + n * 64 + (1 << 20),
                ))
            }
        };
        let layout = config.effective_layout();
        write_header(
            &mut sink,
            config,
            n,
            |i| docs.get(i).id,
            |i| docs.get(i).popularity,
            |i| (text_lens[i], single_word[i]),
            &deletes,
        )?;
        let dim = docs
            .iter()
            .find_map(|d| d.vector.map(<[f32]>::len))
            .or(codes.map(|(dim, _)| dim))
            .unwrap_or(0);
        // Sections are independent, each starting and ending 8-byte aligned, so writing them one by
        // one and building them concurrently produce the same bytes.
        let mut words = words;
        let postings = std::mem::take(&mut words.postings);
        let words_ref = &words;
        // Only the stored sections and the vectors need the documents; they hold them, and they go
        // once those are written, before the variants.
        let title_weights: Vec<f32> = (0..docs.len()).map(|i| docs.get(i).popularity).collect();
        let docs = Arc::new(docs);
        let (stored, vectored) = (docs.clone(), docs.clone());
        let sections: Vec<Section> = vec![
            Box::new(move |w| DocStore::write(w, &stored)),
            Box::new(move |w| {
                FsstColumn::write(w, docs.len(), |i, out| {
                    out.extend_from_slice(docs.get(i).text.as_bytes())
                })?;
                KeyColumn::write(w, docs.len(), |i, out| {
                    out.extend_from_slice(docs.get(i).key.unwrap_or("").as_bytes())
                })
            }),
            Box::new(move |w| {
                let mut rows: Vec<(u32, Row)> = Vec::new();
                for (local, doc) in vectored.iter().enumerate() {
                    let carried = codes.and_then(|(_, map)| map.get(&doc.id));
                    match (doc.vector, carried) {
                        (Some(v), _) => rows.push((local as u32, Row::Float(v))),
                        (None, Some((code, scale))) => {
                            rows.push((local as u32, Row::Code(code, *scale)))
                        }
                        (None, None) => {}
                    }
                }
                Vectors::write(w, dim, config.vector_bits, &rows)
            }),
            Box::new(move |w| {
                let order = Titles::order_of(title_keys)?;
                Titles::write(w, &order, |local| title_weights[local as usize]);
                Ok(())
            }),
            Box::new(move |w| Keyed::write_packed(w, layout.aliases, alias_keys)),
            Box::new(move |w| Keyed::write_packed(w, Dictionary::Fst, context_keys)),
            Box::new(move |w| words_ref.write(w, postings)),
            Box::new(move |w| words_ref.write_variants(w, config)),
        ];
        write_sections(&mut sink, config, sections)?;
        let bytes = sink.len();
        let segment = sink.finish()?;
        tracing::debug!(
            documents = n,
            bytes,
            ms = started.elapsed().as_millis() as u64,
            "built segment"
        );
        Ok(segment)
    }

    pub fn config(&self) -> BuildOptions {
        self.config
    }

    pub fn len(&self) -> usize {
        self.ids.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ids.len() == 0
    }

    /// Document ids in ascending order.
    pub fn ids(&self) -> impl ExactSizeIterator<Item = u64> + '_ {
        self.ids.iter()
    }

    pub fn id(&self, local: usize) -> u64 {
        self.ids.get(local)
    }

    pub(crate) fn local_of(&self, id: u64) -> Option<usize> {
        self.ids.binary_search(id).ok()
    }

    pub(crate) fn text_len(&self, local: usize) -> u16 {
        (self.shapes.get(local) >> 1) as u16
    }

    pub(crate) fn is_single_word(&self, local: usize) -> bool {
        self.shapes.get(local) & 1 == 1
    }

    /// Ids this segment hides in older segments, ascending.
    pub fn deletes(&self) -> &[u64] {
        self.deletes.as_slice()
    }

    /// Size of the encoded segment.
    pub fn size_bytes(&self) -> usize {
        self.data.as_ref().len()
    }

    /// Approximate bytes per section, for diagnostics.
    pub fn section_sizes(&self) -> Vec<(&'static str, usize)> {
        vec![
            (
                "columns",
                self.ids.size() + self.weights.size() + self.shapes.size() + self.deletes.len() * 8,
            ),
            (
                "documents (zstd)",
                self.docs.blocks.as_ref().len() + self.docs.offsets.len() * 8,
            ),
            ("texts (fsst)", self.texts.size()),
            ("keys", self.keys.size()),
            ("titles", self.titles.size()),
            ("words", self.words.size()),
            ("aliases keys", self.aliases.map.size()),
            ("aliases postings", self.aliases.lists.size()),
            ("variants", self.variants.size()),
            ("vectors", self.vectors.as_ref().map_or(0, Vectors::size)),
        ]
    }

    /// Dimension of this segment's embeddings, if it has any.
    pub fn vector_dim(&self) -> Option<usize> {
        self.vectors.as_ref().map(Vectors::dim)
    }

    pub fn document(&self, local: usize) -> Document {
        let (aliases, contexts) = self
            .stored_block(local / DOCS_PER_BLOCK)
            .swap_remove(local % DOCS_PER_BLOCK);
        self.assemble(local, aliases, contexts)
    }

    fn assemble(&self, local: usize, aliases: Vec<Alias>, contexts: Vec<String>) -> Document {
        Document {
            id: self.id(local),
            key: self.key(local),
            text: self.text(local),
            popularity: self.weight(local),
            aliases,
            contexts,
            vector: None,
        }
    }

    /// The original text of `local`.
    pub(crate) fn text(&self, local: usize) -> String {
        self.texts.get(local)
    }

    pub(crate) fn key(&self, local: usize) -> Option<String> {
        Some(self.keys.get(local)).filter(|k| !k.is_empty())
    }

    /// A block's documents; only a crafted segment with a valid checksum can fail here.
    fn stored_block(&self, block: usize) -> Vec<StoredDoc> {
        let expected = (self.len() - block * DOCS_PER_BLOCK).min(DOCS_PER_BLOCK);
        self.docs
            .block(block)
            .ok()
            .filter(|d| d.len() == expected)
            .expect("validated segment")
    }

    pub fn documents(&self) -> impl Iterator<Item = Document> + '_ {
        (0..self.len().div_ceil(DOCS_PER_BLOCK)).flat_map(move |block| {
            let first = block * DOCS_PER_BLOCK;
            let entries = self.stored_block(block);
            entries
                .into_iter()
                .enumerate()
                .map(move |(i, (aliases, contexts))| self.assemble(first + i, aliases, contexts))
        })
    }

    /// The indexed words of `local`'s text, repeats included.
    pub(crate) fn doc_words(&self, local: usize) -> Vec<String> {
        let lower = text::lower(&self.text(local));
        indexed_words(&lower, self.config.min_word_chars)
            .map(str::to_owned)
            .collect()
    }

    pub(crate) fn max_score(&self) -> f64 {
        self.max_score
    }

    pub(crate) fn weight(&self, local: usize) -> f32 {
        self.weights.get(local)
    }

    pub(crate) fn word_text(&self, ordinal: u32) -> &str {
        self.words.text(ordinal)
    }

    pub(crate) fn word_count(&self) -> u32 {
        self.words.len()
    }

    /// Occurrences of the word at `ordinal`; none for an ordinal out of range in a corrupt file.
    pub(crate) fn word_freq_at(&self, ordinal: u32) -> u32 {
        self.words.freq(ordinal)
    }

    /// Occurrences of `word` in this segment's texts, counting repeats within a text.
    #[cfg(test)]
    pub(crate) fn word_freq(&self, word: &str) -> u32 {
        self.words
            .ordinal(&term_key(word))
            .map_or(0, |ordinal| self.words.freq(ordinal))
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        self.data.as_ref().to_vec()
    }

    pub fn from_bytes(data: Vec<u8>) -> Result<Self, Error> {
        Self::decode(Bytes::from_vec(data), true)
    }

    /// Opens a segment file, memory-mapped and read in place.
    /// Maps a segment file, checking only its structure; [`Segment::verify`] checks the rest.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, Error> {
        let file = File::open(path)?;
        // SAFETY: segment files are immutable once written; truncating one while it is open is unsupported.
        let map = unsafe { memmap2::Mmap::map(&file)? };
        // Lookups jump around the file; read-ahead would only fill memory.
        #[cfg(unix)]
        let _ = map.advise(memmap2::Advice::Random);
        Self::decode(Bytes::new(Arc::new(map)), false)
    }

    /// Checks the checksum and every section, as [`Segment::from_bytes`] does.
    pub fn verify(&self) -> Result<(), Error> {
        Self::decode(self.data.clone(), true).map(drop)
    }

    /// Maps every page and builds the vector index in advance, so first queries pay for neither.
    pub fn warm(&self) {
        self.data.touch();
        if let Some(vectors) = &self.vectors {
            let _ = vectors.index();
        }
    }

    pub fn save(&self, path: impl AsRef<Path>) -> Result<(), Error> {
        let path = path.as_ref();
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, self.data.as_ref())?;
        std::fs::rename(tmp, path)?;
        Ok(())
    }

    /// With `full`, also checks the checksum and every value, which reads the whole segment.
    fn decode(data: Bytes, full: bool) -> Result<Self, Error> {
        let bytes = data.as_ref();
        let body = bytes
            .len()
            .checked_sub(8)
            .ok_or_else(|| Error::Corrupt("truncated segment".into()))?;
        if full && xxhash_rust::xxh3::xxh3_64(&bytes[..body]).to_le_bytes() != bytes[body..] {
            return Err(Error::Corrupt("segment checksum mismatch".into()));
        }
        let mut r = Reader::new(data.clone())?;
        if r.raw(MAGIC.len())? != MAGIC {
            return Err(Error::Corrupt("not a completr segment".into()));
        }
        let version = r.u64()?;
        if version != VERSION {
            return Err(Error::Corrupt(format!(
                "unsupported segment version {version}"
            )));
        }
        let (min_word_chars, max_edit_distance, fuzzy_prefix_chars, vector_bits) =
            (r.u8()?, r.u8()?, r.u8()?, r.u8()?);
        let mut code = || Dictionary::from_code(r.u8()?);
        let layout = Layout { aliases: code()? };
        let max_score = f64::from_bits(r.u64()?);
        let compact_keys = layout.aliases == Dictionary::CompactTrie;
        let config = BuildOptions {
            min_word_chars,
            max_edit_distance,
            fuzzy_prefix_chars,
            vector_bits,
            compact_keys,
            build_threads: 1,
            layout,
        };
        let ids = Blocked::read(&mut r, full)?;
        let n = ids.len();
        let local_bound =
            u32::try_from(n).map_err(|_| Error::Corrupt("too many documents".into()))?;
        let weights = Weights::read(&mut r, n, full)?;
        let shapes = Packed::read(&mut r)?;
        let deletes = r.column()?;
        let docs = DocStore::read(&mut r, n)?;
        let texts = FsstColumn::read(&mut r, n, full)?;
        let keys = KeyColumn::read(&mut r, n, full)?;
        let vectors = Vectors::read(&mut r, n, full)?;
        let titles = Titles::read(&mut r, n)?;
        let aliases = Keyed::read(&mut r, layout.aliases, local_bound.saturating_mul(2), full)?;
        let contexts = Keyed::read(&mut r, Dictionary::Fst, local_bound, full)?;
        let words = Words::read(&mut r, local_bound, full)?;
        let variants = Variants::read(&mut r, words.len(), full)?;
        r.align()?;
        r.u64()?;
        r.finish()?;
        let segment = Self {
            data,
            config,
            ids,
            weights,
            max_score,
            shapes,
            deletes,
            docs,
            titles,
            words,
            aliases,
            variants,
            vectors,
            texts,
            keys,
            contexts,
        };
        let valid = segment.shapes.len == n
            && (!full || (1..n).all(|i| segment.id(i - 1) < segment.id(i)))
            && (!full || segment.titles_sorted());
        valid
            .then_some(segment)
            .ok_or_else(|| Error::Corrupt("inconsistent segment".into()))
    }
}

/// The indexed words of a lowercased text, repeats included.
pub(crate) fn indexed_words(lower: &str, min_chars: u8) -> impl Iterator<Item = &str> {
    text::words(lower).filter(move |w| text::char_len(w) >= min_chars as usize)
}

/// Indexed words while documents are scanned: interned, each with its postings and occurrences.
#[derive(Default)]
struct WordTable {
    ids: FxHashMap<Box<str>, u32>,
    postings: Vec<Vec<u32>>,
    freqs: Vec<u32>,
}

impl WordTable {
    fn add(&mut self, word: &str, local: u32) {
        let id = match self.ids.get(word) {
            Some(&id) => id as usize,
            None => {
                self.ids.insert(word.into(), self.postings.len() as u32);
                self.postings.push(Vec::new());
                self.freqs.push(0);
                self.postings.len() - 1
            }
        };
        if self.postings[id].last() != Some(&local) {
            self.postings[id].push(local);
        }
        self.freqs[id] += 1;
    }

    /// The words in term-key order, in flat arrays.
    fn sorted(self) -> SortedWords {
        let mut words: Vec<(Box<str>, u32)> = self.ids.into_iter().collect();
        // Term-key order: a word sorts after the words it is a prefix of, as its key ends in 0xff.
        words.par_sort_unstable_by(|(a, _), (b, _)| {
            let (a, b) = (a.as_bytes(), b.as_bytes());
            let n = a.len().min(b.len());
            a[..n].cmp(&b[..n]).then(b.len().cmp(&a.len()))
        });
        let mut postings = self.postings;
        let mut out = SortedWords {
            keys: Vec::new(),
            key_ends: Vec::with_capacity(words.len()),
            postings: WordPostings {
                lists: ListWriter::default(),
                starts: Vec::with_capacity(words.len()),
                freqs: Vec::with_capacity(words.len()),
            },
        };
        for (word, id) in words {
            out.keys.extend_from_slice(word.as_bytes());
            out.keys.push(TERMINATOR);
            out.key_ends
                .push(offset(out.keys.len()).expect("words under 4 GiB"));
            let list = std::mem::take(&mut postings[id as usize]);
            let start = out
                .postings
                .lists
                .push(&list)
                .expect("ascending, as added by local");
            out.postings.starts.push(start);
            out.postings.freqs.push(self.freqs[id as usize]);
        }
        out
    }
}

/// An arena offset as stored, 32 bits.
fn offset(at: usize) -> Result<u32, Error> {
    u32::try_from(at).map_err(|_| Error::input("over 4 GiB of keys or postings in one segment"))
}

/// Words in term-key order: keys with their terminator in one arena, postings in another.
struct SortedWords {
    keys: Vec<u8>,
    key_ends: Vec<u32>,
    postings: WordPostings,
}

/// Each word's postings, delta coded as written, where its list starts, and its occurrences; taken
/// by the words section, so they are freed before the variants are built.
#[derive(Default)]
struct WordPostings {
    lists: ListWriter,
    starts: Vec<u64>,
    freqs: Vec<u32>,
}

impl SortedWords {
    /// The words section: texts, sampled keys, postings and occurrences.
    fn write(&self, w: &mut Writer, postings: WordPostings) -> Result<(), Error> {
        let keys = (0..self.len()).map(|o| self.key(o));
        Words::write(w, keys, &postings.starts, &postings.lists, &postings.freqs)
    }

    /// The variants section, computed only when written so earlier sections never hold it.
    fn write_variants(&self, w: &mut Writer, config: BuildOptions) -> Result<(), Error> {
        // Variants come from a word's first chars only, so words sharing them share their variants;
        // in key order such words are adjacent.
        let chars = config.fuzzy_prefix_chars as usize;
        let head = |o: usize| {
            let word = self.word(o);
            &word[..word
                .char_indices()
                .nth(chars)
                .map_or(word.len(), |(i, _)| i)]
        };
        let mut starts: Vec<u32> = (0..self.len())
            .filter(|&o| o == 0 || head(o) != head(o - 1))
            .map(|o| o as u32)
            .collect();
        let groups = starts.len();
        starts.push(offset(self.len())?);
        Variants::write(w, Variants::buckets_for(groups), &starts, |group, f| {
            fuzzy::for_each_delete_variant(
                self.word(starts[group] as usize),
                config.max_edit_distance,
                config.fuzzy_prefix_chars as usize,
                |v| f(v.as_bytes()),
            )
        });
        Ok(())
    }

    fn len(&self) -> usize {
        self.key_ends.len()
    }

    fn key(&self, ordinal: usize) -> &[u8] {
        let start = ordinal
            .checked_sub(1)
            .map_or(0, |o| self.key_ends[o] as usize);
        &self.keys[start..self.key_ends[ordinal] as usize]
    }

    fn word(&self, ordinal: usize) -> &str {
        let key = self.key(ordinal);
        // SAFETY: each key is a word's UTF-8 bytes followed by the terminator.
        unsafe { std::str::from_utf8_unchecked(&key[..key.len() - 1]) }
    }
}

/// Collects documents compactly, then builds one segment from them, in memory or into a file.
pub struct SegmentBuilder {
    config: BuildOptions,
    staged: Staged,
    deletes: Vec<u64>,
}

impl SegmentBuilder {
    pub fn new(config: BuildOptions) -> Self {
        Self {
            config,
            staged: Staged::default(),
            deletes: Vec::new(),
        }
    }

    /// Adds a document; of several with one id, the last added is kept.
    pub fn add(&mut self, document: Document) -> Result<(), Error> {
        self.staged.add(document)
    }

    /// Adds a document of a text alone, as [`Document::new`] would, copying `text` once.
    pub fn add_text(&mut self, id: u64, text: &str, popularity: f32) -> Result<(), Error> {
        self.staged.add_text(id, None, text, popularity)
    }

    /// Adds a document of a text alone, as [`Document::keyed`] would, copying `key` and `text` once.
    pub fn add_keyed_text(&mut self, key: &str, text: &str, popularity: f32) -> Result<(), Error> {
        self.staged
            .add_text(crate::key_id(key), Some(key), text, popularity)
    }

    /// Hides `id` in older segments.
    pub fn delete(&mut self, id: u64) {
        self.deletes.push(id);
    }

    pub fn build(self) -> Result<Segment, Error> {
        Segment::build_staged(self.config, self.staged, self.deletes, None, None)
    }

    /// Writes the segment to `path` one section at a time and maps it, so the encoded segment is
    /// never held in memory.
    pub fn write(self, path: impl AsRef<Path>) -> Result<Segment, Error> {
        Segment::build_staged(
            self.config,
            self.staged,
            self.deletes,
            None,
            Some(path.as_ref()),
        )
    }

    /// Documents added, duplicates included.
    pub fn len(&self) -> usize {
        self.staged.added()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// An estimate of the peak memory that building now would take, on the high side.
    pub fn memory_bytes(&self) -> usize {
        self.staged.build_memory()
    }
}

/// Writes documents into segment files in a directory, starting a new segment before the build of
/// the current one would need more than the memory budget, as tantivy flushes a segment when its
/// writer's budget fills. An index of the segments ranks exactly like one segment, and of several
/// documents with one id, the last added wins.
pub struct SegmentWriter {
    config: BuildOptions,
    dir: std::path::PathBuf,
    memory_budget: usize,
    builder: SegmentBuilder,
    segments: Vec<Segment>,
}

impl SegmentWriter {
    pub const DEFAULT_MEMORY_BUDGET: usize = 256 << 20;

    /// Writes into `dir`, created if missing, as `000000.seg`, `000001.seg` and so on.
    pub fn new(config: BuildOptions, dir: impl AsRef<Path>) -> Result<Self, Error> {
        std::fs::create_dir_all(dir.as_ref())?;
        Ok(Self {
            config,
            dir: dir.as_ref().to_owned(),
            memory_budget: Self::DEFAULT_MEMORY_BUDGET,
            builder: SegmentBuilder::new(config),
            segments: Vec::new(),
        })
    }

    /// Peak memory, in bytes, that building one segment may take.
    pub fn memory_budget(mut self, bytes: usize) -> Self {
        self.memory_budget = bytes;
        self
    }

    pub fn add(&mut self, document: Document) -> Result<(), Error> {
        self.make_room()?;
        self.builder.add(document)
    }

    /// As [`SegmentBuilder::add_text`].
    pub fn add_text(&mut self, id: u64, text: &str, popularity: f32) -> Result<(), Error> {
        self.make_room()?;
        self.builder.add_text(id, text, popularity)
    }

    /// As [`SegmentBuilder::add_keyed_text`].
    pub fn add_keyed_text(&mut self, key: &str, text: &str, popularity: f32) -> Result<(), Error> {
        self.make_room()?;
        self.builder.add_keyed_text(key, text, popularity)
    }

    /// Writes the current segment when building more would pass the budget.
    fn make_room(&mut self) -> Result<(), Error> {
        if !self.builder.is_empty() && self.builder.memory_bytes() >= self.memory_budget {
            self.flush()?;
        }
        Ok(())
    }

    /// Writes the documents added since the last flush as a segment.
    pub fn flush(&mut self) -> Result<(), Error> {
        if self.builder.is_empty() {
            return Ok(());
        }
        let builder = std::mem::replace(&mut self.builder, SegmentBuilder::new(self.config));
        let path = self.dir.join(format!("{:06}.seg", self.segments.len()));
        self.segments.push(builder.write(path)?);
        Ok(())
    }

    /// The segments written, in order.
    pub fn finish(mut self) -> Result<Vec<Segment>, Error> {
        self.flush()?;
        Ok(self.segments)
    }
}

/// Where a segment's bytes go as they are produced, followed by their checksum.
enum Sink {
    Memory(Vec<u8>),
    File {
        out: std::io::BufWriter<File>,
        tmp: std::path::PathBuf,
        path: std::path::PathBuf,
        len: usize,
    },
}

impl Sink {
    fn file(path: &Path) -> Result<Self, Error> {
        let tmp = path.with_extension("tmp");
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(&tmp)?;
        Ok(Self::File {
            out: std::io::BufWriter::with_capacity(1 << 20, file),
            tmp,
            path: path.to_owned(),
            len: 0,
        })
    }

    fn finish(self) -> Result<Segment, Error> {
        match self {
            Self::Memory(mut buf) => {
                let checksum = xxhash_rust::xxh3::xxh3_64(&buf);
                buf.extend_from_slice(&checksum.to_le_bytes());
                Segment::decode(Bytes::from_vec(buf), true)
            }
            Self::File { out, tmp, path, .. } => {
                let mut file = out
                    .into_inner()
                    .map_err(std::io::IntoInnerError::into_error)?;
                // Hashed once written, as sections patch their lengths in after streaming.
                // SAFETY: the file is ours alone until it is renamed into place.
                let map = unsafe { memmap2::Mmap::map(&file)? };
                let checksum = xxhash_rust::xxh3::xxh3_64(&map);
                drop(map);
                std::io::Write::write_all(&mut file, &checksum.to_le_bytes())?;
                file.sync_all()?;
                std::fs::rename(&tmp, &path)?;
                Segment::open(&path)
            }
        }
    }
}

impl crate::codec::Out for Sink {
    fn put(&mut self, bytes: &[u8]) -> Result<(), Error> {
        match self {
            Self::Memory(buf) => buf.extend_from_slice(bytes),
            Self::File { out, len, .. } => {
                std::io::Write::write_all(out, bytes)?;
                *len += bytes.len();
            }
        }
        Ok(())
    }

    fn patch(&mut self, at: usize, bytes: &[u8]) -> Result<(), Error> {
        use std::io::{Seek, SeekFrom, Write};
        match self {
            Self::Memory(buf) => buf[at..at + bytes.len()].copy_from_slice(bytes),
            Self::File { out, .. } => {
                out.seek(SeekFrom::Start(at as u64))?;
                out.write_all(bytes)?;
                out.seek(SeekFrom::End(0))?;
            }
        }
        Ok(())
    }

    fn len(&self) -> usize {
        match self {
            Self::Memory(buf) => buf.len(),
            Self::File { len, .. } => *len,
        }
    }
}

/// A section of a segment, written in order or built concurrently.
type Section<'s> = Box<dyn FnOnce(&mut Writer) -> Result<(), Error> + Send + 's>;

/// The header and the per-document columns.
/// `shape(local)` is a document's text length in chars and whether it is a single word.
fn write_header(
    sink: &mut Sink,
    config: BuildOptions,
    n: usize,
    id: impl Fn(usize) -> u64,
    weight: impl Fn(usize) -> f32 + Sync,
    shape: impl Fn(usize) -> (u16, bool) + Sync,
    deletes: &[u64],
) -> Result<(), Error> {
    let layout = config.effective_layout();
    let mut w = Writer::spilling(sink);
    w.raw(MAGIC);
    w.u64(VERSION);
    w.raw(&[
        config.min_word_chars,
        config.max_edit_distance,
        config.fuzzy_prefix_chars,
        config.vector_bits,
    ]);
    w.raw(&[layout.aliases.code()]);
    let factor = crate::index::POPULARITY_WEIGHT * 10.0;
    let max_score = crate::index::max_score_of(|| {
        (0..n).into_par_iter().map(|i| {
            let (len, single) = shape(i);
            crate::index::exact_score(len, single, weight(i), factor)
        })
    });
    w.u64(max_score.to_bits());
    Blocked::write_with(&mut w, n, || (0..n).map(&id));
    Weights::write(&mut w, n, weight);
    Packed::write_with(&mut w, n, || {
        (0..n).map(|i| {
            let (len, single) = shape(i);
            u64::from(len) << 1 | u64::from(single)
        })
    });
    w.column(deletes);
    w.finish()
}

/// Sections are independent, each starting and ending 8-byte aligned, so writing them one by one and
/// building them concurrently produce the same bytes.
fn write_sections(
    sink: &mut Sink,
    config: BuildOptions,
    sections: Vec<Section>,
) -> Result<(), Error> {
    if config.build_threads == 1 {
        for write in sections {
            let mut w = Writer::spilling(&mut *sink);
            write(&mut w)?;
            w.align();
            w.finish()?;
        }
    } else {
        let parts: Vec<Result<Vec<u8>, Error>> = sections.into_par_iter().map(section).collect();
        for part in parts {
            sink.put(&part?)?;
        }
    }
    Ok(())
}

/// One section written to its own buffer, ending aligned.
fn section(write: impl FnOnce(&mut Writer) -> Result<(), Error>) -> Result<Vec<u8>, Error> {
    let mut w = Writer::default();
    write(&mut w)?;
    w.align();
    Ok(w.buf)
}

/// Keys in one byte buffer with a value each: sorted and grouped without an allocation per key.
#[derive(Default)]
struct KeyArena {
    bytes: Vec<u8>,
    /// Start, length and value of each entry.
    entries: Vec<(u32, u32, u32)>,
}

impl KeyArena {
    /// Adds `text` terminated as a term key.
    fn push(&mut self, text: &[u8], value: u32) -> Result<(), Error> {
        let start = u32::try_from(self.bytes.len()).map_err(|_| Error::input("keys over 4 GiB"))?;
        self.bytes.extend_from_slice(text);
        self.bytes.push(TERMINATOR);
        self.entries.push((start, text.len() as u32 + 1, value));
        Ok(())
    }

    fn key(&self, entry: usize) -> &[u8] {
        let (start, len, _) = self.entries[entry];
        &self.bytes[start as usize..(start + len) as usize]
    }

    /// Sorts entries by key and value, drops duplicates, and returns each key's entry range.
    fn sorted_groups(&mut self) -> Vec<(usize, usize)> {
        let bytes = &self.bytes;
        let slice = |e: &(u32, u32, u32)| &bytes[e.0 as usize..(e.0 + e.1) as usize];
        self.entries
            .par_sort_unstable_by(|a, b| slice(a).cmp(slice(b)).then(a.2.cmp(&b.2)));
        self.entries
            .dedup_by(|a, b| a.2 == b.2 && slice(a) == slice(b));
        let mut groups = Vec::new();
        let mut start = 0;
        for i in 1..=self.entries.len() {
            if i == self.entries.len() || slice(&self.entries[i]) != slice(&self.entries[start]) {
                groups.push((start, i));
                start = i;
            }
        }
        groups
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Segment {
        let docs = vec![
            Document::new(7, "Machine Learning", 0.5).with_alias("ML", AliasKind::Abbreviation),
            Document::new(3, "Data Science", 0.0).with_alias("data analytics", AliasKind::Synonym),
            Document::new(7, "Machine learning", 0.75).with_alias("ML", AliasKind::Abbreviation),
        ];
        Segment::build(docs, [9, 9, 1]).unwrap()
    }

    #[test]
    fn keeps_last_duplicate_in_id_order() {
        let seg = sample();
        assert_eq!(seg.ids().collect::<Vec<_>>(), [3, 7]);
        assert_eq!(seg.document(1).text, "Machine learning");
        assert_eq!(seg.deletes(), [1, 9]);
        assert_eq!(seg.word_freq("machine"), 1);
        assert_eq!(seg.word_freq("ml"), 0);
    }

    #[test]
    fn written_segments_match_built_ones() {
        // Over a megabyte of text, so sections spill to the file in pieces.
        let docs: Vec<Document> = (0..60_000u64)
            .map(|i| {
                let text = format!("title {i} with words alpha{} beta{}", i % 97, i % 13);
                Document::new(i * 7 % 60_013, text, 0.5)
            })
            .collect();
        let built = Segment::build(docs.clone(), [3, 1]).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("w.seg");
        let mut builder = SegmentBuilder::new(BuildOptions::default());
        for doc in docs {
            builder.add(doc).unwrap();
        }
        builder.delete(1);
        builder.delete(3);
        let written = builder.write(&path).unwrap();
        assert!(built.size_bytes() > 1 << 20);
        assert_eq!(written.to_bytes(), built.to_bytes());
        written.verify().unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), built.to_bytes());
    }

    #[test]
    fn round_trips_through_bytes() {
        let seg = sample();
        let copy = Segment::from_bytes(seg.to_bytes()).unwrap();
        assert_eq!(
            copy.documents().collect::<Vec<_>>(),
            seg.documents().collect::<Vec<_>>()
        );
        assert_eq!(copy.deletes(), seg.deletes());
        assert_eq!(copy.titled(&term_key("data science")), [0]);
        assert!(Segment::from_bytes(b"nonsense".to_vec()).is_err());
        let mut truncated = seg.to_bytes();
        truncated.truncate(truncated.len() - 9);
        assert!(Segment::from_bytes(truncated).is_err());

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.seg");
        let mut bytes = seg.to_bytes();
        let last = bytes.len() - 1;
        bytes[last] ^= 1;
        std::fs::write(&path, &bytes).unwrap();
        assert!(Segment::from_bytes(bytes).is_err());
        assert!(Segment::open(&path).unwrap().verify().is_err());
        seg.save(&path).unwrap();
        Segment::open(&path).unwrap().verify().unwrap();
    }

    #[test]
    fn uuid_keys_round_trip() {
        for key in [
            "0f42ab32-22cd-4dcf-927b-a8d9a183d68b",
            "00000000-0000-0000-0000-000000000000",
        ] {
            let mut out = Vec::new();
            format_uuid(&parse_uuid(key.as_bytes()).unwrap(), &mut out);
            assert_eq!(out, key.as_bytes());
        }
        for key in [
            "0F42AB32-22CD-4DCF-927B-A8D9A183D68B",
            "0f42ab3222cd4dcf927ba8d9a183d68b",
            "x",
        ] {
            assert!(parse_uuid(key.as_bytes()).is_none());
        }
    }

    #[test]
    fn uuid_keyed_segments_keep_their_keys() {
        let keys = [
            "0f42ab32-22cd-4dcf-927b-a8d9a183d68b",
            "4dce8f93-45ee-4573-8558-8cd321256233",
        ];
        let docs: Vec<Document> = keys
            .iter()
            .map(|k| Document::keyed(*k, format!("title {k}"), 0.5))
            .collect();
        let seg = Segment::build(docs.clone(), []).unwrap();
        assert!(matches!(seg.keys, KeyColumn::Uuids(_)));
        let copy = Segment::from_bytes(seg.to_bytes()).unwrap();
        let mut got: Vec<Document> = copy.documents().collect();
        got.sort_by(|a, b| a.key.cmp(&b.key));
        assert_eq!(got, docs);
    }

    #[test]
    fn stored_max_score_matches_the_estimate() {
        let docs = (0..3000u64).map(|i| {
            Document::new(
                i,
                format!("title {i} words {}", i % 7),
                (i % 11) as f32 / 10.0,
            )
        });
        let index = crate::Index::new(
            vec![Arc::new(Segment::build(docs, []).unwrap())],
            crate::IndexOptions::default(),
        )
        .unwrap();
        assert_eq!(index.max_score, index.estimate_max_score());
    }

    #[test]
    fn packs_many_postings_and_spans_blocks() {
        let docs: Vec<Document> = (0..300)
            .map(|i| Document::new(i, format!("shared title {}", i % 3), 0.1))
            .collect();
        let seg = Segment::build(docs.clone(), []).unwrap();
        assert_eq!(seg.titled(&term_key("shared title 1")).len(), 100);
        assert_eq!(seg.documents().collect::<Vec<_>>(), docs);
        assert_eq!(seg.doc_words(0), ["shared", "title"]);
        assert_eq!(seg.doc_words(299), ["shared", "title"]);
    }
}
