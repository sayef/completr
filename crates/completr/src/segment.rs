use std::fs::File;
use std::path::Path;
use std::sync::Arc;

use rayon::prelude::*;
use rustc_hash::FxHashMap;

use crate::codec::{Bytes, Column, Reader, Writer};
use crate::dict::{Dict, Dictionary};
use crate::trie::Packed;
use crate::vectors::{CarriedCodes, Row, Vectors};
use crate::{fuzzy, text, Alias, AliasKind, Document, Error};

const MAGIC: &[u8; 8] = b"COMPLETR";
const VERSION: u64 = 10;
const DOCS_PER_BLOCK: usize = 128;
const ZSTD_LEVEL: i32 = 3;

/// Appended to every key so a key sorts after its extensions, as in marisa's record tries.
pub(crate) const TERMINATOR: u8 = 0xff;

/// A packed FST value holding its single posting directly.
const INLINE: u64 = 1 << 63;
const LEN_BITS: u32 = 24;

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
    /// Store titles, words and aliases in compact tries: about 25 % smaller keys, with lookups
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
    pub titles: Dictionary,
    pub words: Dictionary,
    pub aliases: Dictionary,
}

impl Default for Layout {
    /// Tries for the scan-heavy titles and aliases, an FST for words: the fastest and
    /// next-smallest layout in our benchmarks.
    fn default() -> Self {
        Self {
            titles: Dictionary::Trie,
            words: Dictionary::Fst,
            aliases: Dictionary::Trie,
        }
    }
}

impl Layout {
    pub fn uniform(dictionary: Dictionary) -> Self {
        Self {
            titles: dictionary,
            words: dictionary,
            aliases: dictionary,
        }
    }
}

impl BuildOptions {
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

/// Postings of one key: inline in the FST value, or a range of the postings column.
pub(crate) enum Postings<'a> {
    One([u32; 1]),
    Many(&'a [u32]),
}

impl Postings<'_> {
    pub(crate) fn as_slice(&self) -> &[u32] {
        match self {
            Self::One(p) => p,
            Self::Many(p) => p,
        }
    }
}

/// An FST from key to postings. Packed values inline a single posting or hold `start << 24 | len`;
/// ordinal values index an offsets column.
pub(crate) struct Keyed {
    pub(crate) map: Dict,
    offsets: Option<Column<u32>>,
    values: Column<u32>,
    bound: u32,
}

impl Keyed {
    /// Writes `arena`'s keys with their postings, one packed value per key.
    fn write_packed(w: &mut Writer, kind: Dictionary, mut arena: KeyArena) -> Result<(), Error> {
        let groups = arena.sorted_groups();
        let mut keys = Vec::with_capacity(groups.len());
        let mut packed_values = Vec::with_capacity(groups.len());
        let mut values = Vec::new();
        let mut postings = Vec::new();
        for &(start, end) in &groups {
            keys.push(arena.key(start));
            postings.clear();
            postings.extend(arena.entries[start..end].iter().map(|e| e.2));
            let value = match postings.as_slice() {
                [one] => INLINE | u64::from(*one),
                many => {
                    if many.len() >= 1 << LEN_BITS {
                        return Err(Error::input("too many postings for one key"));
                    }
                    let packed = (values.len() as u64) << LEN_BITS | many.len() as u64;
                    values.extend_from_slice(many);
                    packed
                }
            };
            packed_values.push(value);
        }
        Dict::write(w, kind, &keys, &packed_values)?;
        w.column(&values);
        Ok(())
    }

    /// Writes sorted, unique `keys` with ordinal values and their postings.
    fn write_ordinal<'a>(
        w: &mut Writer,
        kind: Dictionary,
        groups: impl IntoIterator<Item = (&'a [u8], &'a [u32])>,
    ) -> Result<(), Error> {
        let mut keys = Vec::new();
        let mut offsets = vec![0u32];
        let mut values = Vec::new();
        for (key, postings) in groups {
            keys.push(key);
            values.extend_from_slice(postings);
            offsets
                .push(u32::try_from(values.len()).map_err(|_| Error::input("too many postings"))?);
        }
        let ordinals: Vec<u64> = (0..keys.len() as u64).collect();
        Dict::write(w, kind, &keys, &ordinals)?;
        w.column(&offsets);
        w.column(&values);
        Ok(())
    }

    fn read(
        r: &mut Reader,
        kind: Dictionary,
        ordinal: bool,
        bound: u32,
        full: bool,
    ) -> Result<Self, Error> {
        let map = Dict::read(r, kind)?;
        let offsets = if ordinal {
            Some(r.column::<u32>()?)
        } else {
            None
        };
        let values = r.column::<u32>()?;
        let mut valid = !full || values.as_slice().iter().all(|&v| v < bound);
        if let Some(offsets) = &offsets {
            let o = offsets.as_slice();
            valid &= o.len() == map.len() + 1
                && o.first() == Some(&0)
                && o.last().map(|&l| l as usize) == Some(values.len())
                && (!full || o.windows(2).all(|w| w[0] <= w[1]));
        }
        valid
            .then_some(Self {
                map,
                offsets,
                values,
                bound,
            })
            .ok_or_else(|| Error::Corrupt("invalid postings".into()))
    }

    /// Postings for an FST value; out-of-range values from a corrupt file yield none.
    pub(crate) fn postings(&self, value: u64) -> Postings<'_> {
        let values = self.values.as_slice();
        match &self.offsets {
            Some(offsets) => {
                let o = offsets.as_slice();
                let i = value as usize;
                match (o.get(i), o.get(i + 1)) {
                    (Some(&start), Some(&end)) => {
                        Postings::Many(&values[start as usize..end as usize])
                    }
                    _ => Postings::Many(&[]),
                }
            }
            None if value & INLINE != 0 => match u32::try_from(value & !INLINE) {
                Ok(p) if p < self.bound => Postings::One([p]),
                _ => Postings::Many(&[]),
            },
            None => {
                let start = (value >> LEN_BITS) as usize;
                let len = (value & ((1 << LEN_BITS) - 1)) as usize;
                Postings::Many(values.get(start..start.saturating_add(len)).unwrap_or(&[]))
            }
        }
    }

    pub(crate) fn get(&self, key: &[u8]) -> Option<Postings<'_>> {
        self.map.get(key).map(|value| self.postings(value))
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    fn size(&self) -> usize {
        self.map.size() + self.offsets.as_ref().map_or(0, |o| o.len() * 4) + self.values.len() * 4
    }
}

/// Delete variants hashed into buckets of word ordinals, each with a fingerprint of its variant;
/// a bucket can still hold words of other variants, which the caller discards.
pub(crate) struct Variants {
    buckets: u64,
    offsets: Packed,
    words: Packed,
}

impl Variants {
    const FINGERPRINT_BITS: u32 = 6;

    /// About eight words per bucket at the default settings.
    fn buckets_for(words: usize) -> u64 {
        (words as u64 * 3).max(1)
    }

    /// The bucket in the high bits, and the fingerprint in the low ones.
    fn slot(variant: &[u8], buckets: u64) -> (u64, u64) {
        let h = xxhash_rust::xxh3::xxh3_64(variant);
        let bucket = ((u128::from(h) * u128::from(buckets)) >> 64) as u64;
        (bucket, h & ((1 << Self::FINGERPRINT_BITS) - 1))
    }

    /// An entry of `ordinal` under `variant`, sorting by bucket.
    fn entry(variant: &[u8], buckets: u64, ordinal: u32) -> u64 {
        let (bucket, fingerprint) = Self::slot(variant, buckets);
        bucket << 32 | u64::from(ordinal) << Self::FINGERPRINT_BITS | fingerprint
    }

    /// Writes `entries`, sorted and unique values from [`Variants::entry`].
    fn write(w: &mut Writer, buckets: u64, entries: &[u64]) {
        let mut offsets = vec![0u64; buckets as usize + 1];
        for &e in entries {
            offsets[(e >> 32) as usize + 1] += 1;
        }
        for i in 1..offsets.len() {
            offsets[i] += offsets[i - 1];
        }
        w.u64(buckets);
        Packed::write(w, &offsets);
        Packed::write(
            w,
            &entries.iter().map(|&e| e & 0xffff_ffff).collect::<Vec<_>>(),
        );
    }

    fn read(r: &mut Reader, word_bound: u32, full: bool) -> Result<Self, Error> {
        let buckets = r.u64()?;
        let offsets = Packed::read(r)?;
        let words = Packed::read(r)?;
        let valid = offsets.len as u64 == buckets.saturating_add(1)
            && (!full
                || (0..words.len)
                    .all(|i| words.get(i) >> Self::FINGERPRINT_BITS < u64::from(word_bound))
                    && (1..offsets.len).all(|i| offsets.get(i - 1) <= offsets.get(i))
                    && offsets.get(offsets.len - 1) == words.len as u64);
        valid
            .then_some(Self {
                buckets,
                offsets,
                words,
            })
            .ok_or_else(|| Error::Corrupt("invalid variants".into()))
    }

    /// Ordinals of the words in `variant`'s bucket with its fingerprint.
    pub(crate) fn get(&self, variant: &[u8]) -> impl Iterator<Item = u32> + '_ {
        let (bucket, fingerprint) = Self::slot(variant, self.buckets);
        let start = self.offsets.get(bucket as usize) as usize;
        let end = (self.offsets.get(bucket as usize + 1) as usize).min(self.words.len);
        (start.min(end)..end)
            .map(|i| self.words.get(i))
            .filter(move |&v| v & ((1 << Self::FINGERPRINT_BITS) - 1) == fingerprint)
            .map(|v| (v >> Self::FINGERPRINT_BITS) as u32)
    }

    fn size(&self) -> usize {
        8 + self.offsets.size() + self.words.size()
    }
}

struct StrColumn {
    data: Bytes,
    offsets: Column<u32>,
}

impl StrColumn {
    fn write<'a>(w: &mut Writer, items: impl IntoIterator<Item = &'a str>) -> Result<(), Error> {
        let mut data = Vec::new();
        let mut offsets = vec![0u32];
        for item in items {
            data.extend_from_slice(item.as_bytes());
            offsets.push(
                u32::try_from(data.len()).map_err(|_| Error::input("text column over 4 GiB"))?,
            );
        }
        w.bytes(&data);
        w.column(&offsets);
        Ok(())
    }

    fn read(r: &mut Reader) -> Result<Self, Error> {
        let data = r.bytes()?;
        let offsets = r.column::<u32>()?;
        let text = std::str::from_utf8(data.as_ref())
            .map_err(|_| Error::Corrupt("invalid utf-8".into()))?;
        let o = offsets.as_slice();
        let valid = o.first() == Some(&0)
            && o.last().map(|&l| l as usize) == Some(text.len())
            && o.windows(2)
                .all(|w| w[0] <= w[1] && text.is_char_boundary(w[1] as usize));
        valid
            .then_some(Self { data, offsets })
            .ok_or_else(|| Error::Corrupt("invalid text column".into()))
    }

    fn get(&self, i: usize) -> &str {
        let o = self.offsets.as_slice();
        // SAFETY: validated as UTF-8 with offsets on char boundaries when read.
        unsafe {
            std::str::from_utf8_unchecked(&self.data.as_ref()[o[i] as usize..o[i + 1] as usize])
        }
    }
}

/// Strings compressed with FSST (Boncz et al., VLDB 2020), each decompressed on its own. Columns
/// under `FSST_MIN_BYTES` are stored raw, where a symbol table would not pay for itself.
struct FsstColumn {
    data: Bytes,
    offsets: Column<u32>,
    symbols: Option<([fsst::Symbol; 255], [u8; 255])>,
}

const FSST_MIN_BYTES: usize = 4096;
const ESCAPE: u8 = 255;

impl FsstColumn {
    fn write<'a>(w: &mut Writer, items: impl IntoIterator<Item = &'a str>) -> Result<(), Error> {
        let items: Vec<&[u8]> = items.into_iter().map(str::as_bytes).collect();
        let raw: usize = items.iter().map(|i| i.len()).sum();
        let compressor = (raw >= FSST_MIN_BYTES).then(|| fsst::Compressor::train(&items));
        let mut data = Vec::new();
        let mut offsets = vec![0u32];
        for item in &items {
            match &compressor {
                Some(c) => data.extend_from_slice(&c.compress(item)),
                None => data.extend_from_slice(item),
            }
            offsets.push(
                u32::try_from(data.len()).map_err(|_| Error::input("text column over 4 GiB"))?,
            );
        }
        let (symbols, lengths): (Vec<u64>, Vec<u8>) = match compressor {
            Some(c) => {
                let parts = c.into_parts();
                (
                    parts.symbols.iter().map(|s| s.to_u64()).collect(),
                    parts.lengths.to_vec(),
                )
            }
            None => (Vec::new(), Vec::new()),
        };
        w.column(&symbols);
        w.column(&lengths);
        w.bytes(&data);
        w.column(&offsets);
        Ok(())
    }

    fn read(r: &mut Reader, n: usize) -> Result<Self, Error> {
        let bad = || Error::Corrupt("invalid string column".into());
        let symbols: Column<u64> = r.column()?;
        let lengths: Column<u8> = r.column()?;
        let data = r.bytes()?;
        let offsets = r.column::<u32>()?;
        let o = offsets.as_slice();
        let valid = o.len() == n + 1
            && o.first() == Some(&0)
            && o.last().map(|&l| l as usize) == Some(data.as_ref().len())
            && o.windows(2).all(|w| w[0] <= w[1]);
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
        let o = self.offsets.as_slice();
        let bytes = &self.data.as_ref()[o[i] as usize..o[i + 1] as usize];
        match &self.symbols {
            Some((symbols, lengths)) => {
                // An escape code needs its literal byte; a truncated stream would read past it.
                let mut i = 0;
                while i < bytes.len() {
                    i += if bytes[i] == ESCAPE { 2 } else { 1 };
                }
                if i > bytes.len() {
                    return String::new();
                }
                let decoded = fsst::Decompressor::new(symbols, lengths).decompress(bytes);
                String::from_utf8(decoded)
                    .unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned())
            }
            None => String::from_utf8_lossy(bytes).into_owned(),
        }
    }

    fn size(&self) -> usize {
        self.data.as_ref().len() + self.offsets.len() * 4 + self.symbols.map_or(0, |_| 255 * 9)
    }
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

/// A stored document's aliases and contexts.
type StoredDoc = (Vec<Alias>, Vec<String>);

/// Aliases and contexts, zstd-compressed in blocks; only needed to rebuild documents.
struct DocStore {
    blocks: Bytes,
    offsets: Column<u64>,
}

impl DocStore {
    fn write(w: &mut Writer, docs: &[Document]) -> Result<(), Error> {
        let compressed: Vec<Result<Vec<u8>, Error>> = docs
            .par_chunks(DOCS_PER_BLOCK)
            .map(|chunk| {
                let mut raw = Vec::new();
                for doc in chunk {
                    put_varint(&mut raw, doc.aliases.len());
                    for alias in &doc.aliases {
                        raw.push(alias.kind as u8);
                        put_varint(&mut raw, alias.text.len());
                        raw.extend_from_slice(alias.text.as_bytes());
                    }
                    put_varint(&mut raw, doc.contexts.len());
                    for context in &doc.contexts {
                        put_varint(&mut raw, context.len());
                        raw.extend_from_slice(context.as_bytes());
                    }
                }
                let mut block = (raw.len() as u32).to_le_bytes().to_vec();
                block.extend_from_slice(&zstd::bulk::compress(&raw, ZSTD_LEVEL)?);
                Ok(block)
            })
            .collect();
        let mut blocks = Vec::new();
        let mut offsets = vec![0u64];
        for block in compressed {
            blocks.extend_from_slice(&block?);
            offsets.push(blocks.len() as u64);
        }
        w.bytes(&blocks);
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
    ids: Column<u64>,
    weights: Column<f32>,
    text_lens: Column<u16>,
    single_word: Column<u8>,
    deletes: Column<u64>,
    docs: DocStore,
    pub(crate) titles: Keyed,
    pub(crate) words: Keyed,
    word_freqs: Column<u32>,
    /// Per document, the ordinals of its indexed words, repeats included.
    doc_word_offsets: Column<u32>,
    doc_word_ords: Column<u32>,
    word_texts: StrColumn,
    /// Postings are `local << 1 | kind`.
    pub(crate) aliases: Keyed,
    /// Delete variant to word ordinals.
    pub(crate) variants: Variants,
    pub(crate) vectors: Option<Vectors>,
    texts: FsstColumn,
    /// Empty for documents without a key.
    keys: FsstColumn,
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

    /// Builds with `config.build_threads`; `codes` as for [`Segment::build_inner`].
    pub(crate) fn build_pooled(
        config: BuildOptions,
        documents: Vec<Document>,
        deletes: Vec<u64>,
        codes: Option<(usize, &CarriedCodes)>,
    ) -> Result<Self, Error> {
        crate::vectors::in_pool(config.build_threads, move || {
            Self::build_inner_on_pool(config, documents, deletes, codes)
        })
    }

    /// Builds a segment; `codes` carries vectors over from other segments as `id -> (code, scale)`,
    /// with their dimension.
    pub(crate) fn build_inner(
        config: BuildOptions,
        documents: impl IntoIterator<Item = Document>,
        deletes: impl IntoIterator<Item = u64>,
        codes: Option<(usize, &CarriedCodes)>,
    ) -> Result<Self, Error> {
        Self::build_pooled(
            config,
            documents.into_iter().collect(),
            deletes.into_iter().collect(),
            codes,
        )
    }

    fn build_inner_on_pool(
        config: BuildOptions,
        documents: Vec<Document>,
        deletes: Vec<u64>,
        codes: Option<(usize, &CarriedCodes)>,
    ) -> Result<Self, Error> {
        crate::vectors::validate_bits(config.vector_bits)?;
        let started = std::time::Instant::now();
        let mut docs: Vec<Document> = documents.into_iter().collect();
        if let Some(doc) = docs.iter().find(|d| !d.popularity.is_finite()) {
            return Err(Error::input(format!(
                "document {} has a non-finite popularity",
                doc.id
            )));
        }
        if docs.iter().any(|d| d.key.as_deref() == Some("")) {
            return Err(Error::input("document keys must not be empty"));
        }
        if docs.len() >= (u32::MAX >> 1) as usize {
            return Err(Error::input("too many documents in one segment"));
        }
        docs.reverse();
        docs.sort_by_key(|d| d.id);
        if let Some(pair) = docs
            .windows(2)
            .find(|w| w[0].id == w[1].id && w[0].key != w[1].key)
        {
            return Err(Error::input(format!(
                "keys {:?} and {:?} map to the same id {}",
                pair[0].key, pair[1].key, pair[0].id
            )));
        }
        docs.dedup_by_key(|d| d.id);
        let mut deletes: Vec<u64> = deletes.into_iter().collect();
        deletes.sort_unstable();
        deletes.dedup();

        let n = docs.len();
        let mut text_lens = Vec::with_capacity(n);
        let mut single_word = Vec::with_capacity(n);
        let mut title_keys = KeyArena::default();
        let mut alias_keys = KeyArena::default();
        let mut context_keys = KeyArena::default();
        // Per word: postings, occurrences, and a provisional id for the forward index.
        let mut word_map: FxHashMap<String, (Vec<u32>, u32, u32)> = FxHashMap::default();
        let mut doc_word_offsets = vec![0u32];
        let mut doc_word_ids: Vec<u32> = Vec::new();
        for (local, doc) in docs.iter().enumerate() {
            let local = local as u32;
            let lower = text::lower(&doc.text);
            text_lens.push(u16::try_from(text::char_len(&lower)).unwrap_or(u16::MAX));
            single_word.push(u8::from(text::words(&lower).count() == 1));
            for word in
                text::words(&lower).filter(|w| text::char_len(w) >= config.min_word_chars as usize)
            {
                let next_id = word_map.len() as u32;
                let (postings, freq, id) = word_map
                    .entry(word.to_owned())
                    .or_insert_with(|| (Vec::new(), 0, next_id));
                if postings.last() != Some(&local) {
                    postings.push(local);
                }
                *freq += 1;
                doc_word_ids.push(*id);
            }
            doc_word_offsets.push(
                u32::try_from(doc_word_ids.len()).map_err(|_| Error::input("too many words"))?,
            );
            title_keys.push(lower.as_bytes(), local)?;
            for alias in &doc.aliases {
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

        // Term key, word, postings and occurrences.
        type Word = (Vec<u8>, String, Vec<u32>, u32);
        let mut remap = vec![0u32; word_map.len()];
        let mut words: Vec<Word> = Vec::with_capacity(word_map.len());
        let mut provisional: Vec<u32> = Vec::with_capacity(word_map.len());
        for (w, (postings, freq, id)) in word_map {
            words.push((term_key(&w), w, postings, freq));
            provisional.push(id);
        }
        let mut order: Vec<usize> = (0..words.len()).collect();
        order.sort_unstable_by(|&a, &b| words[a].0.cmp(&words[b].0));
        for (ordinal, &i) in order.iter().enumerate() {
            remap[provisional[i] as usize] = ordinal as u32;
        }
        let mut sorted_words = Vec::with_capacity(words.len());
        let mut slots: Vec<Option<Word>> = words.into_iter().map(Some).collect();
        for &i in &order {
            sorted_words.push(slots[i].take().expect("each word once"));
        }
        let words = sorted_words;
        let doc_word_ords: Vec<u32> = doc_word_ids.iter().map(|&id| remap[id as usize]).collect();
        if words.len() >> (32 - Variants::FINGERPRINT_BITS) != 0 {
            return Err(Error::input("too many distinct words for one segment"));
        }
        let buckets = Variants::buckets_for(words.len());
        let mut variant_entries: Vec<u64> = words
            .par_iter()
            .enumerate()
            .flat_map_iter(|(ordinal, (_, word, _, _))| {
                fuzzy::delete_variants(
                    word,
                    config.max_edit_distance,
                    config.fuzzy_prefix_chars as usize,
                )
                .into_iter()
                .map(move |v| Variants::entry(v.as_bytes(), buckets, ordinal as u32))
            })
            .collect();
        variant_entries.par_sort_unstable();
        variant_entries.dedup();

        let mut w = Writer::default();
        w.raw(MAGIC);
        w.u64(VERSION);
        let layout = if config.compact_keys {
            Layout::uniform(Dictionary::CompactTrie)
        } else {
            config.layout
        };
        w.raw(&[
            config.min_word_chars,
            config.max_edit_distance,
            config.fuzzy_prefix_chars,
            config.vector_bits,
        ]);
        w.raw(&[
            layout.titles.code(),
            layout.words.code(),
            layout.aliases.code(),
        ]);
        w.column(&docs.iter().map(|d| d.id).collect::<Vec<_>>());
        w.column(&docs.iter().map(|d| d.popularity).collect::<Vec<_>>());
        w.column(&text_lens);
        w.column(&single_word);
        w.column(&deletes);
        let dim = docs
            .iter()
            .find_map(|d| d.vector.as_ref().map(Vec::len))
            .or(codes.map(|(dim, _)| dim))
            .unwrap_or(0);
        let mut rows: Vec<(u32, Row)> = Vec::new();
        for (local, doc) in docs.iter().enumerate() {
            let carried = codes.and_then(|(_, map)| map.get(&doc.id));
            match (&doc.vector, carried) {
                (Some(v), _) => rows.push((local as u32, Row::Float(v))),
                (None, Some((code, scale))) => rows.push((local as u32, Row::Code(code, *scale))),
                (None, None) => {}
            }
        }
        // Sections are independent, each starting and ending 8-byte aligned, so writing them in
        // place one by one and building them concurrently produce the same bytes.
        type Section<'s> = Box<dyn FnOnce(&mut Writer) -> Result<(), Error> + Send + 's>;
        let (docs_ref, words_ref, rows_ref) = (&docs, &words, &rows);
        let (doc_word_offsets_ref, doc_word_ords_ref) = (&doc_word_offsets[..], &doc_word_ords[..]);
        let sections: Vec<Section> = vec![
            Box::new(move |w| DocStore::write(w, docs_ref)),
            Box::new(move |w| Keyed::write_packed(w, layout.titles, title_keys)),
            Box::new(move |w| {
                Keyed::write_ordinal(
                    w,
                    layout.words,
                    words_ref.iter().map(|w| (w.0.as_slice(), w.2.as_slice())),
                )?;
                w.column(&words_ref.iter().map(|w| w.3).collect::<Vec<_>>());
                StrColumn::write(w, words_ref.iter().map(|w| w.1.as_str()))?;
                w.column(doc_word_offsets_ref);
                w.column(doc_word_ords_ref);
                Ok(())
            }),
            Box::new(move |w| Keyed::write_packed(w, layout.aliases, alias_keys)),
            Box::new(move |w| {
                Variants::write(w, buckets, &variant_entries);
                Ok(())
            }),
            Box::new(move |w| Vectors::write(w, dim, config.vector_bits, rows_ref)),
            Box::new(move |w| {
                FsstColumn::write(w, docs_ref.iter().map(|d| d.text.as_str()))?;
                FsstColumn::write(w, docs_ref.iter().map(|d| d.key.as_deref().unwrap_or("")))?;
                Keyed::write_packed(w, Dictionary::Fst, context_keys)
            }),
        ];
        if config.build_threads == 1 {
            for write in sections {
                w.align();
                write(&mut w)?;
                w.align();
            }
        } else {
            let parts: Vec<Result<Vec<u8>, Error>> =
                sections.into_par_iter().map(section).collect();
            for part in parts {
                w.align();
                w.raw(&part?);
            }
        }
        w.align();
        w.u64(xxhash_rust::xxh3::xxh3_64(&w.buf));
        tracing::debug!(
            documents = n,
            bytes = w.buf.len(),
            ms = started.elapsed().as_millis() as u64,
            "built segment"
        );
        Self::decode(Bytes::from_vec(w.buf), true)
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
    pub fn ids(&self) -> &[u64] {
        self.ids.as_slice()
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
            ("columns", self.ids.len() * 15 + self.deletes.len() * 8),
            (
                "documents (zstd)",
                self.docs.blocks.as_ref().len() + self.docs.offsets.len() * 8,
            ),
            ("texts (fsst)", self.texts.size() + self.keys.size()),
            ("titles", self.titles.size()),
            (
                "words",
                self.words.size() + self.word_freqs.len() * 4 + self.word_texts.data.as_ref().len(),
            ),
            ("aliases keys", self.aliases.map.size()),
            ("aliases postings", self.aliases.values.len() * 4),
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
            id: self.ids()[local],
            key: self.key(local),
            text: self.text(local),
            popularity: self.weights()[local],
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

    /// Ordinals of the indexed words of `local`, repeats included.
    pub(crate) fn doc_words(&self, local: usize) -> &[u32] {
        let o = self.doc_word_offsets.as_slice();
        let range = o[local] as usize..o[local + 1] as usize;
        self.doc_word_ords.as_slice().get(range).unwrap_or(&[])
    }

    pub(crate) fn weights(&self) -> &[f32] {
        self.weights.as_slice()
    }

    pub(crate) fn text_lens(&self) -> &[u16] {
        self.text_lens.as_slice()
    }

    pub(crate) fn single_word(&self) -> &[u8] {
        self.single_word.as_slice()
    }

    pub(crate) fn word_text(&self, ordinal: u32) -> &str {
        self.word_texts.get(ordinal as usize)
    }

    pub(crate) fn word_count(&self) -> u32 {
        self.word_freqs.len() as u32
    }

    pub(crate) fn word_freq_at(&self, ordinal: u32) -> u32 {
        self.word_freqs.as_slice()[ordinal as usize]
    }

    /// Occurrences of `word` in this segment's texts, counting repeats within a text.
    #[cfg(test)]
    pub(crate) fn word_freq(&self, word: &str) -> u32 {
        self.words
            .map
            .get(&term_key(word))
            .map_or(0, |ordinal| self.word_freqs.as_slice()[ordinal as usize])
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

    /// Maps every page of the segment in advance, so first queries do not pay page faults.
    pub fn warm(&self) {
        self.data.touch();
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
        let layout = Layout {
            titles: code()?,
            words: code()?,
            aliases: code()?,
        };
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
        let ids = r.column::<u64>()?;
        let n = ids.len();
        let local_bound =
            u32::try_from(n).map_err(|_| Error::Corrupt("too many documents".into()))?;
        let weights = r.column()?;
        let text_lens = r.column()?;
        let single_word = r.column()?;
        let deletes = r.column()?;
        let docs = DocStore::read(&mut r, n)?;
        let titles = Keyed::read(&mut r, layout.titles, false, local_bound, full)?;
        let words = Keyed::read(&mut r, layout.words, true, local_bound, full)?;
        let word_freqs: Column<u32> = r.column()?;
        let word_texts = StrColumn::read(&mut r)?;
        let doc_word_offsets: Column<u32> = r.column()?;
        let doc_word_ords: Column<u32> = r.column()?;
        let (o, ords) = (doc_word_offsets.as_slice(), doc_word_ords.as_slice());
        let forward_ok = o.len() == n + 1
            && o.first() == Some(&0)
            && o.last().map(|&l| l as usize) == Some(ords.len())
            && (!full
                || o.windows(2).all(|w| w[0] <= w[1])
                    && ords.iter().all(|&w| (w as usize) < word_freqs.len()));
        if !forward_ok {
            return Err(Error::Corrupt("invalid forward index".into()));
        }
        let aliases = Keyed::read(
            &mut r,
            layout.aliases,
            false,
            local_bound.saturating_mul(2),
            full,
        )?;
        let word_bound =
            u32::try_from(word_freqs.len()).map_err(|_| Error::Corrupt("too many words".into()))?;
        let variants = Variants::read(&mut r, word_bound, full)?;
        let vectors = Vectors::read(&mut r, n)?;
        let texts = FsstColumn::read(&mut r, n)?;
        let keys = FsstColumn::read(&mut r, n)?;
        let contexts = Keyed::read(&mut r, Dictionary::Fst, false, local_bound, full)?;
        r.align()?;
        r.u64()?;
        r.finish()?;
        let segment = Self {
            data,
            config,
            ids,
            weights,
            text_lens,
            single_word,
            deletes,
            docs,
            titles,
            words,
            word_freqs,
            word_texts,
            doc_word_offsets,
            doc_word_ords,
            aliases,
            variants,
            vectors,
            texts,
            keys,
            contexts,
        };
        let valid = segment.weights.len() == n
            && segment.text_lens.len() == n
            && segment.single_word.len() == n
            && (!full
                || segment.weights().iter().all(|w| w.is_finite())
                    && segment.ids().windows(2).all(|w| w[0] < w[1]))
            && segment.word_freqs.len() == segment.words.map.len()
            && segment.word_texts.offsets.len() == segment.words.map.len() + 1;
        valid
            .then_some(segment)
            .ok_or_else(|| Error::Corrupt("inconsistent segment".into()))
    }
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
        assert_eq!(seg.ids(), [3, 7]);
        assert_eq!(seg.document(1).text, "Machine learning");
        assert_eq!(seg.deletes(), [1, 9]);
        assert_eq!(seg.word_freq("machine"), 1);
        assert_eq!(seg.word_freq("ml"), 0);
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
        assert_eq!(
            copy.titles
                .get(&term_key("data science"))
                .unwrap()
                .as_slice(),
            [0]
        );
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
    fn packs_many_postings_and_spans_blocks() {
        let docs: Vec<Document> = (0..300)
            .map(|i| Document::new(i, format!("shared title {}", i % 3), 0.1))
            .collect();
        let seg = Segment::build(docs.clone(), []).unwrap();
        assert_eq!(
            seg.titles
                .get(&term_key("shared title 1"))
                .unwrap()
                .as_slice()
                .len(),
            100
        );
        assert_eq!(seg.documents().collect::<Vec<_>>(), docs);
        let word = |local: usize| -> Vec<&str> {
            seg.doc_words(local)
                .iter()
                .map(|&o| seg.word_text(o))
                .collect()
        };
        assert_eq!(word(0), ["shared", "title"]);
        assert_eq!(word(299), ["shared", "title"]);
    }
}
