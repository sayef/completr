//! Merging segments through their sorted structures, as tantivy's merger does: documents are mapped
//! to their new places, dictionaries are unioned in key order with remapped postings, and stored
//! parts are copied. The result is byte for byte the segment a build from the live documents gives.

use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::path::Path;

use rustc_hash::FxHashMap;

use super::*;

/// One segment to merge, and which of its documents are hidden.
pub(crate) struct Part<'a> {
    pub(crate) segment: &'a Segment,
    pub(crate) hidden: &'a [bool],
}

/// Keys in order, each with a value.
trait Keys {
    fn advance(&mut self) -> bool;
    fn key(&self) -> &[u8];
    fn value(&self) -> u64;
}

impl Keys for crate::dict::Cursor<'_> {
    fn advance(&mut self) -> bool {
        self.advance()
    }

    fn key(&self) -> &[u8] {
        self.key()
    }

    fn value(&self) -> u64 {
        self.value()
    }
}

impl Keys for WordCursor<'_> {
    fn advance(&mut self) -> bool {
        self.advance()
    }

    fn key(&self) -> &[u8] {
        self.key()
    }

    fn value(&self) -> u64 {
        self.value()
    }
}

/// Unions the parts' `cursors` in key order, calling `f(key, hits)` with each key and the
/// `(part, value)` pairs holding it.
fn union<C: Keys>(
    cursors: impl IntoIterator<Item = C>,
    mut f: impl FnMut(&[u8], &[(usize, u64)]) -> Result<(), Error>,
) -> Result<(), Error> {
    let mut cursors: Vec<C> = cursors.into_iter().collect();
    // The parts' next keys, smallest first, then by part.
    let mut heap: BinaryHeap<Reverse<(Vec<u8>, usize)>> = BinaryHeap::new();
    for (i, c) in cursors.iter_mut().enumerate() {
        if c.advance() {
            heap.push(Reverse((c.key().to_vec(), i)));
        }
    }
    let (mut key, mut hits, mut taken) = (Vec::new(), Vec::new(), Vec::new());
    let mut first = true;
    while let Some(Reverse((next, i))) = heap.pop() {
        // Keys only come out unordered from a corrupt file.
        if !first && next <= key {
            return Err(Error::Corrupt("dictionary keys out of order".into()));
        }
        first = false;
        key = next;
        taken.clear();
        taken.push(i);
        while heap.peek().is_some_and(|Reverse((k, _))| *k == key) {
            taken.push(heap.pop().expect("peeked").0 .1);
        }
        hits.clear();
        hits.extend(taken.iter().map(|&i| (i, cursors[i].value())));
        f(&key, &hits)?;
        for &i in &taken {
            if cursors[i].advance() {
                heap.push(Reverse((cursors[i].key().to_vec(), i)));
            }
        }
    }
    Ok(())
}

/// Keys with their postings, in key order.
#[derive(Default)]
struct Groups {
    keys: Vec<u8>,
    key_ends: Vec<u32>,
    values: Vec<u32>,
    value_ends: Vec<u32>,
}

impl Groups {
    fn push(&mut self, key: &[u8], values: &[u32]) -> Result<(), Error> {
        self.keys.extend_from_slice(key);
        self.key_ends.push(offset(self.keys.len())?);
        self.values.extend_from_slice(values);
        self.value_ends.push(offset(self.values.len())?);
        Ok(())
    }

    fn iter(&self) -> impl Iterator<Item = (&[u8], std::iter::Copied<std::slice::Iter<'_, u32>>)> {
        (0..self.key_ends.len()).map(|i| {
            let k = i.checked_sub(1).map_or(0, |p| self.key_ends[p] as usize);
            let v = i.checked_sub(1).map_or(0, |p| self.value_ends[p] as usize);
            (
                &self.keys[k..self.key_ends[i] as usize],
                self.values[v..self.value_ends[i] as usize].iter().copied(),
            )
        })
    }
}

/// A part's new place of each local: `None` for one hidden or superseded.
struct Places {
    len: usize,
    /// A bit per local kept, with the kept count before each word; `None` when all are kept.
    kept: Option<(Vec<u64>, Vec<u32>)>,
    places: Blocked,
}

impl Places {
    fn new(len: usize, kept: Vec<u64>, places: Blocked) -> Self {
        let kept = (places.len() < len).then(|| {
            let ranks = kept
                .iter()
                .scan(0u32, |rank, w| {
                    let r = *rank;
                    *rank += w.count_ones();
                    Some(r)
                })
                .collect();
            (kept, ranks)
        });
        Self { len, kept, places }
    }

    fn get(&self, local: usize) -> Option<u32> {
        if local >= self.len {
            return None;
        }
        let rank = match &self.kept {
            None => local,
            Some((bits, ranks)) => {
                let word = bits[local / 64];
                if word >> (local % 64) & 1 == 0 {
                    return None;
                }
                let below = word & ((1u64 << (local % 64)) - 1);
                (ranks[local / 64] + below.count_ones()) as usize
            }
        };
        Some(self.places.get(rank) as u32)
    }
}

/// The union of one key set across parts, postings remapped by `map(part, posting)` and dropped
/// when it gives `None`.
fn merge_keyed(
    parts: &[Part],
    keyed: impl Fn(&Segment) -> &Keyed,
    map: impl Fn(usize, u32) -> Option<u32>,
) -> Result<Groups, Error> {
    let mut groups = Groups::default();
    let mut postings = Vec::new();
    let cursors = parts.iter().map(|p| keyed(p.segment).map.cursor(b""));
    union(cursors, |key, hits| {
        postings.clear();
        for &(part, value) in hits {
            postings.extend(
                keyed(parts[part].segment)
                    .postings(value)
                    .filter_map(|p| map(part, p)),
            );
        }
        postings.sort_unstable();
        postings.dedup();
        if !postings.is_empty() {
            groups.push(key, &postings)?;
        }
        Ok(())
    })?;
    Ok(groups)
}

impl Segment {
    /// The live documents of `parts` as one segment with `deletes`, written to `path` or kept in
    /// memory: the same bytes as building from those documents, without re-reading their texts
    /// for words, keys or variants.
    pub(crate) fn merge(
        config: BuildOptions,
        parts: &[Part],
        deletes: Vec<u64>,
        path: Option<&Path>,
    ) -> Result<Segment, Error> {
        crate::vectors::in_pool(config.build_threads, move || {
            Self::merge_on_pool(config, parts, deletes, path)
        })
    }

    fn merge_on_pool(
        config: BuildOptions,
        parts: &[Part],
        mut deletes: Vec<u64>,
        path: Option<&Path>,
    ) -> Result<Segment, Error> {
        let started = std::time::Instant::now();
        deletes.sort_unstable();
        deletes.dedup();

        // New places by id: of live copies of one id, the last part's. Each holds its part's base
        // plus its local.
        let bases: Vec<u32> = parts
            .iter()
            .scan(0u32, |base, p| {
                let start = *base;
                *base += p.segment.len() as u32;
                Some(start)
            })
            .collect();
        let split = |g: u32| {
            let p = bases.partition_point(|&b| b <= g) - 1;
            (p, (g - bases[p]) as usize)
        };
        let mut order: Vec<u32> = Vec::with_capacity(parts.iter().map(|p| p.segment.len()).sum());
        let mut kept: Vec<Vec<u64>> = parts
            .iter()
            .map(|p| vec![0; p.segment.len().div_ceil(64)])
            .collect();
        let mut next = vec![0usize; parts.len()];
        let advance = |part: usize, from: usize| {
            (from..parts[part].segment.len()).find(|&l| !parts[part].hidden[l])
        };
        let mut heap: BinaryHeap<Reverse<(u64, Reverse<usize>)>> = BinaryHeap::new();
        for part in 0..parts.len() {
            if let Some(l) = advance(part, 0) {
                next[part] = l;
                heap.push(Reverse((parts[part].segment.id(l), Reverse(part))));
            }
        }
        let mut last = None;
        while let Some(Reverse((id, Reverse(part)))) = heap.pop() {
            let local = next[part];
            if last != Some(id) {
                kept[part][local / 64] |= 1 << (local % 64);
                order.push(bases[part] + local as u32);
                last = Some(id);
            }
            if let Some(l) = advance(part, local + 1) {
                next[part] = l;
                heap.push(Reverse((parts[part].segment.id(l), Reverse(part))));
            }
        }
        let n = order.len();
        if n >= (u32::MAX >> 1) as usize {
            return Err(Error::input("too many documents in one segment"));
        }
        // A part's kept documents take ascending places, close to a line, so they pack in a few
        // bits each.
        let columns = crate::blocked::columns_with(parts.len(), || {
            order
                .iter()
                .enumerate()
                .map(|(i, &g)| (split(g).0, i as u64))
        });
        let places: Vec<Places> = (kept.into_iter().zip(columns).zip(parts))
            .map(|((kept, places), part)| Places::new(part.segment.len(), kept, places))
            .collect();
        // A posting out of range, possible only in a corrupt file, maps to nothing.
        let place = |p: usize, l: u32| places[p].get(l as usize);
        let mut sink = match path {
            Some(path) => Sink::file(path)?,
            None => Sink::Memory(Vec::new()),
        };
        // One section at a time, each using the pool, so no section waits whole in memory.
        let streaming = BuildOptions {
            build_threads: 1,
            ..config
        };
        {
            let doc = |i: usize| {
                let (p, l) = split(order[i]);
                (parts[p].segment, l)
            };
            write_header(
                &mut sink,
                config,
                n,
                |i| {
                    let (s, l) = doc(i);
                    s.id(l)
                },
                |i| {
                    let (s, l) = doc(i);
                    s.weight(l)
                },
                |i| {
                    let (s, l) = doc(i);
                    (s.text_len(l), s.is_single_word(l))
                },
                &deletes,
            )?;
            let dim = parts
                .iter()
                .find_map(|p| p.segment.vectors.as_ref().map(Vectors::dim))
                .unwrap_or(0);
            let order = &order;
            // The sections in document order come first, so the order goes before the words merge.
            let sections: Vec<Section> = vec![
                Box::new(move |w| {
                    // A part's documents come in its own order, so one decoded block per part
                    // serves all of that block's documents however the parts interleave.
                    let mut cached: Vec<Option<(usize, Vec<StoredDoc>)>> = vec![None; parts.len()];
                    DocStore::write_blocks(w, n, |first, end, raw| {
                        for &g in &order[first..end] {
                            let (p, l) = split(g);
                            let (segment, block) = (parts[p].segment, l / DOCS_PER_BLOCK);
                            if cached[p].as_ref().is_none_or(|c| c.0 != block) {
                                cached[p] = Some((block, segment.stored_block(block)));
                            }
                            let (aliases, contexts) =
                                &cached[p].as_ref().expect("cached").1[l % DOCS_PER_BLOCK];
                            encode_stored(raw, aliases, contexts);
                        }
                        Ok(())
                    })
                }),
                Box::new(move |w| {
                    FsstColumn::write(w, n, |i, out| {
                        let (s, l) = doc(i);
                        s.texts.append(l, out)
                    })?;
                    // Keys carried as they are stored, when every part stores them alike.
                    if parts.iter().all(|p| p.segment.keys.is_empty()) {
                        return KeyColumn::write(w, n, |_, _| {});
                    }
                    if n > 0 && parts.iter().all(|p| p.segment.keys.uuid(0).is_some()) {
                        KeyColumn::write_uuids(w, n, |i| {
                            let (s, l) = doc(i);
                            s.keys.uuid(l).unwrap_or(&[0; 16])
                        });
                        return Ok(());
                    }
                    KeyColumn::write(w, n, |i, out| {
                        let (s, l) = doc(i);
                        out.extend_from_slice(s.key(l).unwrap_or_default().as_bytes())
                    })
                }),
                Box::new(move |w| {
                    let rows: Vec<(u32, Row)> = (0..n)
                        .filter_map(|i| {
                            let (s, l) = doc(i);
                            let (code, scale) = s.vectors.as_ref()?.row(l as u32)?;
                            Some((i as u32, Row::Code(code, scale)))
                        })
                        .collect();
                    Vectors::write(w, dim, config.vector_bits, &rows)
                }),
            ];
            write_sections(&mut sink, streaming, sections)?;
        }
        drop(order);
        let layout = config.effective_layout();
        let sections: Vec<Section> = vec![
            Box::new(|w| Titles::merge(w, parts, n, place)),
            Box::new(move |w| {
                let aliases = merge_keyed(
                    parts,
                    |s| &s.aliases,
                    |p, v| Some(place(p, v >> 1)? << 1 | v & 1),
                )?;
                Keyed::write_groups(w, layout.aliases, aliases.iter())
            }),
            Box::new(move |w| {
                let contexts = merge_keyed(parts, |s| &s.contexts, place)?;
                Keyed::write_groups(w, Dictionary::Fst, contexts.iter())
            }),
        ];
        write_sections(&mut sink, streaming, sections)?;
        // The words, their postings written out as they are merged, then their variants, once the
        // places are gone.
        {
            let mut w = Writer::spilling(&mut sink);
            let words = Self::merge_words(&mut w, parts, &place)?;
            w.align();
            drop(places);
            words.write_variants(&mut w, config)?;
            w.align();
            w.finish()?;
        }
        let bytes = sink.len();
        let segment = sink.finish()?;
        tracing::debug!(
            documents = n,
            parts = parts.len(),
            bytes,
            ms = started.elapsed().as_millis() as u64,
            "merged segments"
        );
        Ok(segment)
    }

    /// The union of the parts' words, with postings remapped and occurrences in hidden documents
    /// taken out; words left without a live document are dropped.
    /// Writes the merged words section to `w`, the postings as they come, and returns the words.
    fn merge_words(
        w: &mut Writer,
        parts: &[Part],
        place: &impl Fn(usize, u32) -> Option<u32>,
    ) -> Result<SortedWords, Error> {
        let hidden: Vec<FxHashMap<String, u32>> = parts
            .iter()
            .map(|p| {
                let mut counts = FxHashMap::default();
                for local in (0..p.segment.len()).filter(|&l| p.hidden[l]) {
                    for word in p.segment.doc_words(local) {
                        *counts.entry(word).or_default() += 1;
                    }
                }
                counts
            })
            .collect();
        let cursors = parts.iter().map(|p| WordCursor::new(&p.segment.words, b""));
        // The parts' words together bound the merged ones; reserved once, so the buffers never copy
        // themselves while growing, and untouched pages cost nothing.
        let count: usize = parts.iter().map(|p| p.segment.words.len() as usize).sum();
        let text: usize = parts.iter().map(|p| p.segment.words.text_bytes()).sum();
        let mut words = SortedWords {
            keys: Vec::with_capacity(text + count),
            key_ends: Vec::with_capacity(count),
            postings: WordPostings::default(),
        };
        let (mut starts, mut freqs) = (BlockedWriter::default(), BlockedWriter::default());
        let mut lists = ListStream::new(w);
        let mut postings = Vec::new();
        union(cursors, |key, hits| {
            postings.clear();
            let mut freq = 0u32;
            // A word that is not UTF-8, possible only in a corrupt file, is dropped.
            let Ok(word) = std::str::from_utf8(&key[..key.len().saturating_sub(1)]) else {
                return Ok(());
            };
            for &(part, ordinal) in hits {
                let segment = parts[part].segment;
                postings.extend(
                    segment
                        .words
                        .postings(ordinal as u32)
                        .filter_map(|l| place(part, l)),
                );
                let occurrences = segment.word_freq_at(ordinal as u32);
                freq += occurrences.saturating_sub(hidden[part].get(word).copied().unwrap_or(0));
            }
            if postings.is_empty() {
                return Ok(());
            }
            postings.sort_unstable();
            words.keys.extend_from_slice(key);
            words.key_ends.push(offset(words.keys.len())?);
            starts.push(lists.push(&postings)?);
            freqs.push(u64::from(freq));
            Ok(())
        })?;
        if u32::try_from(words.len()).is_err() {
            return Err(Error::input("too many distinct words for one segment"));
        }
        let w = lists.finish();
        let (starts, freqs) = (starts.into_blocked(), freqs.into_blocked());
        Words::write_after_lists(
            w,
            (0..words.len()).map(|o| words.key(o)),
            || starts.iter(),
            freqs.len(),
            || freqs.iter(),
        )?;
        Ok(words)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::vectors::CarriedCodes;
    use crate::{Document, Index, IndexOptions, Segment};

    fn doc(i: u64, text: String) -> Document {
        let mut d = if i.is_multiple_of(11) {
            Document::keyed(format!("key-{i}"), text, (i % 50) as f32 / 50.0)
        } else {
            Document::new(i, text, (i % 50) as f32 / 50.0)
        };
        if i.is_multiple_of(7) {
            d = d
                .with_synonym(format!("synonym {i}"))
                .with_abbreviation(format!("ab{i}"));
        }
        if i.is_multiple_of(5) {
            d = d
                .with_context(format!("ctx{}", i % 3))
                .with_context("shared");
        }
        if i.is_multiple_of(3) {
            d = d.with_vector((0..8).map(|k| ((i + k) % 13) as f32 - 6.0).collect());
        }
        d
    }

    /// Compaction as it was: a build from the live documents, vector codes carried over.
    fn rebuilt(index: &Index) -> Segment {
        let mut codes = CarriedCodes::default();
        for (i, seg) in index.segments().iter().enumerate() {
            if let Some(vectors) = &seg.vectors {
                for &local in vectors.locals() {
                    if !index.segment_hidden(i)[local as usize] {
                        let (code, scale) = vectors.row(local).unwrap();
                        codes.insert(seg.id(local as usize), (code.to_vec(), scale));
                    }
                }
            }
        }
        let config = index.segments()[0].config();
        Segment::build_inner(config, index.documents(), Vec::new(), Some((8, &codes))).unwrap()
    }

    #[test]
    fn merging_matches_rebuilding() {
        let base: Vec<Document> = (0..3_000)
            .map(|i| doc(i, format!("title {i} alpha{} beta{}", i % 37, i % 11)))
            .collect();
        let update: Vec<Document> = (100..400)
            .map(|i| doc(i, format!("updated {i} gamma{}", i % 5)))
            .collect();
        let more: Vec<Document> = (2_900..3_200)
            .map(|i| doc(i, format!("newer {i} alpha{} delta", i % 37)))
            .collect();
        let segments = vec![
            Arc::new(Segment::build(base, []).unwrap()),
            Arc::new(Segment::build(update, [5, 7, 2_950]).unwrap()),
            Arc::new(Segment::build(more, [9]).unwrap()),
        ];
        let index = Index::new(segments, IndexOptions::default()).unwrap();
        let merged = index.compact().unwrap();
        assert_eq!(merged.to_bytes(), rebuilt(&index).to_bytes());
        merged.verify().unwrap();
    }

    #[test]
    fn merging_uuid_keys_and_no_keys_matches_rebuilding() {
        let uuid = |i: u64| {
            Document::keyed(
                format!("00000000-0000-4000-8000-{i:012x}"),
                format!("title {i}"),
                0.5,
            )
        };
        let plain = |i: u64| Document::new(i, format!("title {i}"), 0.5);
        for make in [uuid, plain] {
            let segments = vec![
                Arc::new(Segment::build((0..500).map(make), []).unwrap()),
                Arc::new(Segment::build((400..700).map(make), []).unwrap()),
            ];
            let index = Index::new(segments, IndexOptions::default()).unwrap();
            let merged = index.compact().unwrap();
            assert_eq!(merged.to_bytes(), rebuilt(&index).to_bytes());
        }
    }
}
