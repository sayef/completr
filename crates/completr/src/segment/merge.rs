//! Merging segments through their sorted structures, as tantivy's merger does: documents are mapped
//! to their new places, dictionaries are unioned in key order with remapped postings, and stored
//! parts are copied. The result is byte for byte the segment a build from the live documents gives.

use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::path::Path;

use rustc_hash::FxHashMap;

use super::*;

/// One segment to merge, and which of its documents are live.
pub(crate) struct Part<'a> {
    pub(crate) segment: &'a Segment,
    pub(crate) live: &'a [bool],
}

/// Unions `dicts` in key order, calling `f(key, hits)` with each key and the `(part, value)` pairs
/// holding it.
fn union(
    dicts: &[&Dict],
    mut f: impl FnMut(&[u8], &[(usize, u64)]) -> Result<(), Error>,
) -> Result<(), Error> {
    let mut cursors: Vec<(usize, crate::dict::Cursor)> = dicts
        .iter()
        .enumerate()
        .map(|(i, d)| (i, d.cursor(b"")))
        .filter_map(|(i, mut c)| c.advance().then_some((i, c)))
        .collect();
    let (mut key, mut hits) = (Vec::new(), Vec::new());
    let mut first = true;
    while !cursors.is_empty() {
        let next = cursors
            .iter()
            .map(|(_, c)| c.key())
            .min()
            .expect("a cursor");
        // Keys only come out unordered from a corrupt file.
        if !first && next <= key.as_slice() {
            return Err(Error::Corrupt("dictionary keys out of order".into()));
        }
        first = false;
        key.clear();
        key.extend_from_slice(next);
        hits.clear();
        hits.extend(
            cursors
                .iter()
                .filter(|(_, c)| c.key() == key.as_slice())
                .map(|(i, c)| (*i, c.value())),
        );
        f(&key, &hits)?;
        cursors.retain_mut(|(_, c)| c.key() != key.as_slice() || c.advance());
    }
    Ok(())
}

/// Keys with their postings, in key order.
#[derive(Default)]
struct Groups {
    keys: Vec<u8>,
    key_ends: Vec<usize>,
    values: Vec<u32>,
    value_ends: Vec<usize>,
}

impl Groups {
    fn push(&mut self, key: &[u8], values: &[u32]) {
        self.keys.extend_from_slice(key);
        self.key_ends.push(self.keys.len());
        self.values.extend_from_slice(values);
        self.value_ends.push(self.values.len());
    }

    fn iter(&self) -> impl Iterator<Item = (&[u8], std::iter::Copied<std::slice::Iter<'_, u32>>)> {
        (0..self.key_ends.len()).map(|i| {
            let k = i.checked_sub(1).map_or(0, |p| self.key_ends[p]);
            let v = i.checked_sub(1).map_or(0, |p| self.value_ends[p]);
            (
                &self.keys[k..self.key_ends[i]],
                self.values[v..self.value_ends[i]].iter().copied(),
            )
        })
    }
}

/// The union of one key set across parts, postings remapped by `map(part, posting)` and dropped
/// when it gives `None`.
fn merge_keyed(
    parts: &[Part],
    keyed: impl Fn(&Segment) -> &Keyed,
    map: impl Fn(usize, u32) -> Option<u32>,
) -> Result<Groups, Error> {
    let dicts: Vec<&Dict> = parts.iter().map(|p| &keyed(p.segment).map).collect();
    let mut groups = Groups::default();
    let mut postings = Vec::new();
    union(&dicts, |key, hits| {
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
            groups.push(key, &postings);
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

        // New places by id: of live copies of one id, the last part's.
        let mut order: Vec<(u32, u32)> = Vec::new();
        let mut remap: Vec<Vec<u32>> = parts
            .iter()
            .map(|p| vec![u32::MAX; p.segment.len()])
            .collect();
        let mut next = vec![0usize; parts.len()];
        let advance = |part: usize, from: usize| {
            (from..parts[part].segment.len()).find(|&l| parts[part].live[l])
        };
        let mut heap: BinaryHeap<Reverse<(u64, Reverse<usize>)>> = BinaryHeap::new();
        for part in 0..parts.len() {
            if let Some(l) = advance(part, 0) {
                next[part] = l;
                heap.push(Reverse((parts[part].segment.ids()[l], Reverse(part))));
            }
        }
        while let Some(Reverse((id, Reverse(part)))) = heap.pop() {
            let local = next[part];
            if order
                .last()
                .is_none_or(|&(p, l)| parts[p as usize].segment.ids()[l as usize] != id)
            {
                remap[part][local] = order.len() as u32;
                order.push((part as u32, local as u32));
            }
            if let Some(l) = advance(part, local + 1) {
                next[part] = l;
                heap.push(Reverse((parts[part].segment.ids()[l], Reverse(part))));
            }
        }
        let n = order.len();
        if n >= (u32::MAX >> 1) as usize {
            return Err(Error::input("too many documents in one segment"));
        }
        let doc = |i: usize| {
            let (p, l) = order[i];
            (parts[p as usize].segment, l as usize)
        };

        // A posting out of range, possible only in a corrupt file, maps to nothing.
        let place = |p: usize, l: u32| remap[p].get(l as usize).copied().filter(|&r| r != u32::MAX);
        let titles = merge_keyed(parts, |s| &s.titles, place)?;
        let words = Self::merge_words(parts, &place)?;
        let aliases = merge_keyed(
            parts,
            |s| &s.aliases,
            |p, v| Some(place(p, v >> 1)? << 1 | v & 1),
        )?;
        let contexts = merge_keyed(parts, |s| &s.contexts, place)?;
        let dim = parts
            .iter()
            .find_map(|p| p.segment.vectors.as_ref().map(Vectors::dim))
            .unwrap_or(0);
        let rows: Vec<(u32, Row)> = (0..n)
            .filter_map(|i| {
                let (s, l) = doc(i);
                let (code, scale) = s.vectors.as_ref()?.row(l as u32)?;
                Some((i as u32, Row::Code(code, scale)))
            })
            .collect();

        let mut sink = match path {
            Some(path) => Sink::file(path)?,
            None => Sink::Memory(Vec::new()),
        };
        write_header(
            &mut sink,
            config,
            &(0..n)
                .map(|i| {
                    let (s, l) = doc(i);
                    s.ids()[l]
                })
                .collect::<Vec<_>>(),
            &(0..n)
                .map(|i| {
                    let (s, l) = doc(i);
                    s.weights()[l]
                })
                .collect::<Vec<_>>(),
            &(0..n)
                .map(|i| {
                    let (s, l) = doc(i);
                    s.text_lens.as_slice()[l]
                })
                .collect::<Vec<_>>(),
            &(0..n)
                .map(|i| {
                    let (s, l) = doc(i);
                    s.single_word.as_slice()[l]
                })
                .collect::<Vec<_>>(),
            &deletes,
        )?;
        let layout = config.effective_layout();
        let (words_ref, rows_ref) = (&words, &rows);
        let (titles_ref, aliases_ref, contexts_ref, order) = (&titles, &aliases, &contexts, &order);
        let sections: Vec<Section> = vec![
            Box::new(move |w| {
                DocStore::write_blocks(w, n, |first, end, raw| {
                    let mut cached: Option<(usize, usize, Vec<StoredDoc>)> = None;
                    for &(p, l) in &order[first..end] {
                        let (segment, block) =
                            (parts[p as usize].segment, l as usize / DOCS_PER_BLOCK);
                        if cached
                            .as_ref()
                            .is_none_or(|c| (c.0, c.1) != (p as usize, block))
                        {
                            cached = Some((p as usize, block, segment.stored_block(block)));
                        }
                        let (aliases, contexts) =
                            &cached.as_ref().expect("cached").2[l as usize % DOCS_PER_BLOCK];
                        encode_stored(raw, aliases, contexts);
                    }
                    Ok(())
                })
            }),
            Box::new(move |w| Keyed::write_groups(w, layout.titles, titles_ref.iter())),
            Box::new(move |w| words_ref.write(w, layout.words)),
            Box::new(move |w| Keyed::write_groups(w, layout.aliases, aliases_ref.iter())),
            Box::new(move |w| words_ref.write_variants(w, config)),
            Box::new(move |w| Vectors::write(w, dim, config.vector_bits, rows_ref)),
            Box::new(move |w| {
                FsstColumn::write(w, n, |i, out| {
                    let (p, l) = order[i];
                    parts[p as usize].segment.texts.append(l as usize, out)
                })?;
                FsstColumn::write(w, n, |i, out| {
                    let (p, l) = order[i];
                    out.extend_from_slice(
                        parts[p as usize]
                            .segment
                            .key(l as usize)
                            .unwrap_or_default()
                            .as_bytes(),
                    )
                })?;
                Keyed::write_groups(w, Dictionary::Fst, contexts_ref.iter())
            }),
        ];
        write_sections(&mut sink, config, sections)?;
        let bytes = sink.len();
        drop(rows);
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
    fn merge_words(
        parts: &[Part],
        place: &impl Fn(usize, u32) -> Option<u32>,
    ) -> Result<SortedWords, Error> {
        let hidden: Vec<FxHashMap<String, u32>> = parts
            .iter()
            .map(|p| {
                let mut counts = FxHashMap::default();
                for local in (0..p.segment.len()).filter(|&l| !p.live[l]) {
                    for word in p.segment.doc_words(local) {
                        *counts.entry(word).or_default() += 1;
                    }
                }
                counts
            })
            .collect();
        let dicts: Vec<&Dict> = parts.iter().map(|p| &p.segment.words.map).collect();
        let mut words = SortedWords {
            keys: Vec::new(),
            key_ends: Vec::new(),
            postings: Vec::new(),
            posting_ends: Vec::new(),
            freqs: Vec::new(),
        };
        let mut postings = Vec::new();
        union(&dicts, |key, hits| {
            postings.clear();
            let mut freq = 0u32;
            let word = std::str::from_utf8(&key[..key.len().saturating_sub(1)]).unwrap_or("");
            for &(part, ordinal) in hits {
                let segment = parts[part].segment;
                postings.extend(
                    segment
                        .words
                        .postings(ordinal)
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
            words.key_ends.push(words.keys.len());
            words.postings.extend_from_slice(&postings);
            words.posting_ends.push(words.postings.len());
            words.freqs.push(freq);
            Ok(())
        })?;
        if words.len() >> (32 - Variants::FINGERPRINT_BITS) != 0 {
            return Err(Error::input("too many distinct words for one segment"));
        }
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
                    if index.segment_live(i)[local as usize] {
                        let (code, scale) = vectors.row(local).unwrap();
                        codes.insert(seg.ids()[local as usize], (code.to_vec(), scale));
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
}
