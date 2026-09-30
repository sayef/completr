//! Sorted key dictionaries mapping bytes to a `u64`: an FST, or a LOUDS trie.

use crate::codec::{Bytes, Column, Reader, Writer};
use crate::trie::{Packed, Trie, TrieCursor};
use crate::Error;

/// A key set's structure; strato picks one per key set (see `Layout`).
#[doc(hidden)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Dictionary {
    /// `fst` crate: a minimal automaton sharing prefixes and suffixes.
    #[default]
    Fst,
    /// A LOUDS trie with shared tails, in the style of marisa-trie: smaller keys.
    Trie,
    /// The trie with its tails stored in a second trie: smaller still, slower to read.
    CompactTrie,
}

impl Dictionary {
    pub(crate) fn code(self) -> u8 {
        match self {
            Self::Fst => 0,
            Self::Trie => 1,
            Self::CompactTrie => 2,
        }
    }

    pub(crate) fn from_code(code: u8) -> Result<Self, Error> {
        match code {
            0 => Ok(Self::Fst),
            1 => Ok(Self::Trie),
            2 => Ok(Self::CompactTrie),
            other => Err(Error::Format(format!("unknown dictionary {other}"))),
        }
    }
}

const INLINE_U64: u64 = 1 << 63;

pub(crate) enum Dict {
    Fst(fst::Map<Bytes>),
    /// A bit-packed value per key id: `posting << 1 | 1` inline, or `index << 1` into `wide`.
    Trie {
        trie: Box<Trie>,
        values: Packed,
        wide: Column<u64>,
    },
}

impl Dict {
    /// Writes `entries`, which must be sorted by key with unique keys.
    pub(crate) fn write(
        w: &mut Writer,
        kind: Dictionary,
        keys: &[&[u8]],
        values: &[u64],
    ) -> Result<(), Error> {
        match kind {
            Dictionary::Fst => {
                let mut builder = fst::MapBuilder::memory();
                for (key, value) in keys.iter().zip(values) {
                    builder.insert(key, *value)?;
                }
                w.bytes(&builder.into_inner()?);
            }
            Dictionary::Trie | Dictionary::CompactTrie => {
                let ids = Trie::write_with(w, keys, kind == Dictionary::CompactTrie)?;
                let mut packed = vec![0u64; keys.len()];
                let mut wide = Vec::new();
                for (value, id) in values.iter().zip(ids) {
                    packed[id as usize] = if value & INLINE_U64 != 0 {
                        (value & !INLINE_U64) << 1 | 1
                    } else {
                        wide.push(*value);
                        ((wide.len() - 1) as u64) << 1
                    };
                }
                Packed::write(w, &packed);
                w.column(&wide);
            }
        }
        Ok(())
    }

    pub(crate) fn read(r: &mut Reader, kind: Dictionary) -> Result<Self, Error> {
        match kind {
            Dictionary::Fst => Ok(Self::Fst(fst::Map::new(r.bytes()?)?)),
            Dictionary::Trie | Dictionary::CompactTrie => {
                let trie = Box::new(Trie::read(r)?);
                let values = Packed::read(r)?;
                let wide: Column<u64> = r.column()?;
                // Wide indexes are bounds-checked when decoded.
                let valid = values.len == trie.len();
                valid
                    .then_some(Self::Trie { trie, values, wide })
                    .ok_or_else(|| Error::Format("invalid trie values".into()))
            }
        }
    }

    fn decode(values: &Packed, wide: &Column<u64>, id: usize) -> u64 {
        let v = values.get(id);
        if v & 1 == 1 {
            INLINE_U64 | v >> 1
        } else {
            wide.as_slice().get((v >> 1) as usize).copied().unwrap_or(0)
        }
    }

    pub(crate) fn get(&self, key: &[u8]) -> Option<u64> {
        match self {
            Self::Fst(map) => map.get(key),
            Self::Trie { trie, values, wide } => {
                trie.get(key).map(|id| Self::decode(values, wide, id))
            }
        }
    }

    pub(crate) fn len(&self) -> usize {
        match self {
            Self::Fst(map) => map.len(),
            Self::Trie { trie, .. } => trie.len(),
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub(crate) fn size(&self) -> usize {
        match self {
            Self::Fst(map) => map.as_fst().as_bytes().len(),
            Self::Trie { trie, values, wide } => trie.size() + values.size() + wide.len() * 8,
        }
    }

    /// Keys starting with `prefix`, in byte order.
    pub(crate) fn cursor(&self, prefix: &[u8]) -> Cursor<'_> {
        use fst::IntoStreamer;
        let inner = match self {
            Self::Fst(map) => Inner::Fst(map.range().ge(prefix).into_stream()),
            Self::Trie { trie, values, wide } => Inner::Trie(trie.cursor(prefix), values, wide),
        };
        Cursor {
            inner,
            prefix: prefix.to_vec(),
            key: Vec::new(),
            value: 0,
        }
    }
}

pub(crate) struct Cursor<'a> {
    inner: Inner<'a>,
    prefix: Vec<u8>,
    key: Vec<u8>,
    value: u64,
}

enum Inner<'a> {
    Fst(fst::map::Stream<'a>),
    Trie(TrieCursor<'a>, &'a Packed, &'a Column<u64>),
    Done,
}

impl Cursor<'_> {
    /// Moves to the next key; `false` once past the prefix.
    pub(crate) fn advance(&mut self) -> bool {
        use fst::Streamer;
        let found = match &mut self.inner {
            Inner::Fst(stream) => match stream.next() {
                Some((key, value)) if key.starts_with(&self.prefix) => {
                    self.key.clear();
                    self.key.extend_from_slice(key);
                    self.value = value;
                    true
                }
                _ => false,
            },
            Inner::Trie(cursor, values, wide) => match cursor.advance() {
                Some(id) => {
                    self.value = Dict::decode(values, wide, id);
                    true
                }
                None => false,
            },
            Inner::Done => false,
        };
        if !found {
            self.inner = Inner::Done;
        }
        found
    }

    pub(crate) fn key(&self) -> &[u8] {
        match &self.inner {
            Inner::Trie(cursor, ..) => cursor.key(),
            _ => &self.key,
        }
    }

    pub(crate) fn value(&self) -> u64 {
        self.value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds() -> Vec<Dictionary> {
        vec![Dictionary::Fst, Dictionary::Trie, Dictionary::CompactTrie]
    }

    #[test]
    fn lookups_and_cursors_agree_across_backends() {
        let keys: [&[u8]; 7] = [
            b"data\xff",
            b"database\xff",
            b"datum",
            "\u{e4}rzte\u{ff}".as_bytes(),
            "\u{e4}rzt".as_bytes(),
            b"zoo",
            b"",
        ];
        let mut entries: Vec<(Vec<u8>, u64)> = keys
            .iter()
            .enumerate()
            .map(|(i, k)| {
                (
                    k.to_vec(),
                    if i % 2 == 0 {
                        (1 << 63) | i as u64
                    } else {
                        (i as u64) << 24 | 3
                    },
                )
            })
            .collect();
        entries.sort();
        entries.dedup_by(|a, b| a.0 == b.0);
        for kind in kinds() {
            let mut w = Writer::default();
            let keys: Vec<&[u8]> = entries.iter().map(|(k, _)| k.as_slice()).collect();
            let values: Vec<u64> = entries.iter().map(|(_, v)| *v).collect();
            Dict::write(&mut w, kind, &keys, &values).unwrap();
            let dict = Dict::read(&mut Reader::new(Bytes::from_vec(w.buf)).unwrap(), kind).unwrap();
            assert_eq!(dict.len(), entries.len());
            for (key, value) in &entries {
                assert_eq!(dict.get(key), Some(*value), "{kind:?}");
            }
            assert_eq!(dict.get(b"dat"), None);
            for prefix in [&b""[..], b"d", b"dat", b"data", "ä".as_bytes(), b"q"] {
                let mut cursor = dict.cursor(prefix);
                let mut got = Vec::new();
                while cursor.advance() {
                    got.push((cursor.key().to_vec(), cursor.value()));
                }
                let want: Vec<_> = entries
                    .iter()
                    .filter(|(k, _)| k.starts_with(prefix))
                    .cloned()
                    .collect();
                assert_eq!(got, want, "{kind:?} {prefix:?}");
            }
        }
    }
}
