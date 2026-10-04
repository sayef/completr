//! Documents collected for one segment build: texts in one arena, a fixed-size entry per document.

use std::cmp::Reverse;

use crate::{Alias, Document, Error};

/// The rarely used parts of a document, boxed so a plain document costs one pointer.
struct Extra {
    aliases: Vec<Alias>,
    contexts: Vec<String>,
    vector: Option<Vec<f32>>,
}

/// A document's text, then its key, in the arena.
struct Entry {
    id: u64,
    start: u32,
    len: u32,
    key_len: u32,
    popularity: f32,
    extra: Option<Box<Extra>>,
}

/// One staged document, borrowed.
pub(crate) struct DocRef<'a> {
    pub(crate) id: u64,
    pub(crate) text: &'a str,
    pub(crate) popularity: f32,
    pub(crate) key: Option<&'a str>,
    pub(crate) aliases: &'a [Alias],
    pub(crate) contexts: &'a [String],
    pub(crate) vector: Option<&'a [f32]>,
}

#[derive(Default)]
pub(crate) struct Staged {
    text: Vec<u8>,
    entries: Vec<Entry>,
    /// Bytes of keys, aliases and contexts, and vector elements, for the memory estimate.
    extra_bytes: usize,
    key_bytes: usize,
    vector_floats: usize,
    /// Entries by id once finished, the last added of each id.
    order: Vec<u32>,
}

impl Staged {
    pub(crate) fn add(&mut self, doc: Document) -> Result<(), Error> {
        let plain = doc.aliases.is_empty() && doc.contexts.is_empty() && doc.vector.is_none();
        let extra_bytes = doc.aliases.iter().map(|a| a.text.len() + 8).sum::<usize>()
            + doc.contexts.iter().map(|c| c.len() + 8).sum::<usize>();
        self.vector_floats += doc.vector.as_ref().map_or(0, Vec::len);
        let extra = (!plain).then(|| {
            Box::new(Extra {
                aliases: doc.aliases,
                contexts: doc.contexts,
                vector: doc.vector,
            })
        });
        self.extra_bytes += extra_bytes;
        self.push(doc.id, doc.key.as_deref(), &doc.text, doc.popularity, extra)
    }

    /// Adds a document of a text alone, copied from `text` and `key`.
    pub(crate) fn add_text(
        &mut self,
        id: u64,
        key: Option<&str>,
        text: &str,
        popularity: f32,
    ) -> Result<(), Error> {
        self.push(id, key, text, popularity, None)
    }

    fn push(
        &mut self,
        id: u64,
        key: Option<&str>,
        text: &str,
        popularity: f32,
        extra: Option<Box<Extra>>,
    ) -> Result<(), Error> {
        if !popularity.is_finite() {
            return Err(Error::input(format!(
                "document {id} has a non-finite popularity"
            )));
        }
        if key == Some("") {
            return Err(Error::input("document keys must not be empty"));
        }
        if self.entries.len() >= (u32::MAX >> 1) as usize {
            return Err(Error::input("too many documents in one segment"));
        }
        let key = key.unwrap_or("");
        let start = self.text.len();
        if start + text.len() + key.len() > u32::MAX as usize {
            return Err(Error::input("texts over 4 GiB in one segment"));
        }
        self.text.extend_from_slice(text.as_bytes());
        self.text.extend_from_slice(key.as_bytes());
        self.extra_bytes += key.len();
        self.key_bytes += key.len();
        self.entries.push(Entry {
            id,
            start: start as u32,
            len: text.len() as u32,
            key_len: key.len() as u32,
            popularity,
            extra,
        });
        Ok(())
    }

    fn key_of(&self, e: &Entry) -> Option<&str> {
        let at = (e.start + e.len) as usize;
        let bytes = &self.text[at..at + e.key_len as usize];
        // SAFETY: each key was copied whole from a `str`.
        (!bytes.is_empty()).then(|| unsafe { std::str::from_utf8_unchecked(bytes) })
    }

    /// Documents added so far, duplicates included.
    pub(crate) fn added(&self) -> usize {
        self.entries.len()
    }

    /// A conservative estimate of the peak memory of building these documents, measured on short
    /// names, titles and paragraphs: the build needs about 6 bytes per text byte and 120 per document.
    pub(crate) fn build_memory(&self) -> usize {
        // Keys share the arena with texts but count among the extra bytes.
        (self.text.len() - self.key_bytes) * 15 / 2
            + self.entries.len() * 150
            + self.extra_bytes * 8
            + self.vector_floats * 12
    }

    /// Orders the documents by id, keeping the last added of each id; ids whose copies carry
    /// different keys are an error.
    pub(crate) fn finish(&mut self) -> Result<(), Error> {
        let mut order: Vec<u32> = (0..self.entries.len() as u32).collect();
        let entries = &self.entries;
        order.sort_unstable_by_key(|&i| (entries[i as usize].id, Reverse(i)));
        let key = |i: u32| self.key_of(&entries[i as usize]);
        if let Some(pair) = order.windows(2).find(|w| {
            entries[w[0] as usize].id == entries[w[1] as usize].id && key(w[0]) != key(w[1])
        }) {
            return Err(Error::input(format!(
                "keys {:?} and {:?} map to the same id {}",
                key(pair[0]),
                key(pair[1]),
                entries[pair[0] as usize].id
            )));
        }
        order.dedup_by_key(|i| entries[*i as usize].id);
        self.order = order;
        Ok(())
    }

    /// Documents after [`Staged::finish`].
    pub(crate) fn len(&self) -> usize {
        self.order.len()
    }

    /// The `local`th document by id, after [`Staged::finish`].
    pub(crate) fn get(&self, local: usize) -> DocRef<'_> {
        let e = &self.entries[self.order[local] as usize];
        let bytes = &self.text[e.start as usize..(e.start + e.len) as usize];
        let extra = e.extra.as_deref();
        DocRef {
            id: e.id,
            // SAFETY: each text was copied whole from a `String`.
            text: unsafe { std::str::from_utf8_unchecked(bytes) },
            popularity: e.popularity,
            key: self.key_of(e),
            aliases: extra.map_or(&[], |x| &x.aliases),
            contexts: extra.map_or(&[], |x| &x.contexts),
            vector: extra.and_then(|x| x.vector.as_deref()),
        }
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = DocRef<'_>> + '_ {
        (0..self.len()).map(|local| self.get(local))
    }
}
