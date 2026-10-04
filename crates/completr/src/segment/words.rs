//! The word dictionary: words in term-key order, found by binary search over every `SAMPLE`-th key
//! and then the texts, as in tantivy's sstable dictionary.

use super::*;

/// Words between two sampled keys.
const SAMPLE: usize = 64;

pub(crate) struct Words {
    texts: StrColumn,
    sample_ends: Column<u32>,
    samples: Bytes,
    /// Where each word's list of postings starts.
    starts: Blocked,
    lists: Lists,
    freqs: Packed,
    bound: u32,
}

/// `word` with its terminator against `key`, byte by byte.
fn term_cmp(word: &[u8], key: &[u8]) -> std::cmp::Ordering {
    word.iter()
        .chain(std::iter::once(&TERMINATOR))
        .cmp(key.iter())
}

impl Words {
    /// Writes the words of `keys` (term keys in order) with their lists and occurrences.
    pub(super) fn write<'a>(
        w: &mut Writer,
        keys: impl Iterator<Item = &'a [u8]> + Clone,
        starts: &[u64],
        lists: &ListWriter,
        freqs: &[u32],
    ) -> Result<(), Error> {
        lists.write(w);
        Self::write_after_lists(
            w,
            keys,
            || starts.iter().copied(),
            freqs.len(),
            || freqs.iter().map(|&f| u64::from(f)),
        )
    }

    /// As [`Words::write`], the lists having been written already.
    pub(super) fn write_after_lists<'a, I: Iterator<Item = u64>, J: Iterator<Item = u64>>(
        w: &mut Writer,
        keys: impl Iterator<Item = &'a [u8]> + Clone,
        starts: impl Fn() -> I,
        count: usize,
        freqs: impl Fn() -> J,
    ) -> Result<(), Error> {
        let word = |key: &'a [u8]| {
            // SAFETY: each key is a word's UTF-8 bytes followed by the terminator.
            unsafe { std::str::from_utf8_unchecked(&key[..key.len() - 1]) }
        };
        StrColumn::write(w, keys.clone().map(word))?;
        let (mut sample_ends, mut samples) = (Vec::new(), Vec::new());
        for key in keys.step_by(SAMPLE) {
            samples.extend_from_slice(key);
            sample_ends.push(offset(samples.len())?);
        }
        w.column(&sample_ends);
        w.bytes(&samples);
        Blocked::write_with(w, count, starts);
        Packed::write_with(w, count, freqs);
        Ok(())
    }

    pub(super) fn read(r: &mut Reader, bound: u32, full: bool) -> Result<Self, Error> {
        let lists = Lists::read(r, bound, full)?;
        let words = Self {
            texts: StrColumn::read(r, full)?,
            sample_ends: r.column()?,
            samples: r.bytes()?,
            starts: Blocked::read(r, full)?,
            lists,
            freqs: Packed::read(r)?,
            bound,
        };
        let n = words.freqs.len;
        let ends = words.sample_ends.as_slice();
        let valid = words.texts.offsets.len() == n + 1
            && words.starts.len() == n
            && ends.len() == n.div_ceil(SAMPLE)
            && ends
                .last()
                .is_none_or(|&e| e as usize == words.samples.as_ref().len())
            && (!full || words.verify());
        valid
            .then_some(words)
            .ok_or_else(|| Error::Corrupt("invalid words".into()))
    }

    /// Whether the words ascend, their lists start in order, and the samples are their keys.
    fn verify(&self) -> bool {
        let n = self.len() as usize;
        (1..n).all(|o| {
            let previous = self.text(o as u32 - 1).as_bytes();
            term_cmp(self.text(o as u32).as_bytes(), &term_key_of(previous))
                == std::cmp::Ordering::Greater
                && self.starts.get(o - 1) < self.starts.get(o)
        }) && (0..self.sample_ends.len()).all(|s| {
            term_cmp(self.text((s * SAMPLE) as u32).as_bytes(), self.sample(s))
                == std::cmp::Ordering::Equal
        })
    }

    pub(crate) fn len(&self) -> u32 {
        self.freqs.len as u32
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub(crate) fn text_bytes(&self) -> usize {
        self.texts.data.as_ref().len()
    }

    pub(crate) fn text(&self, ordinal: u32) -> &str {
        self.texts.get(ordinal as usize)
    }

    /// Occurrences of the word at `ordinal`; none for an ordinal out of range in a corrupt file.
    pub(crate) fn freq(&self, ordinal: u32) -> u32 {
        if ordinal < self.len() {
            self.freqs.get(ordinal as usize) as u32
        } else {
            0
        }
    }

    pub(crate) fn postings(&self, ordinal: u32) -> Postings<'_> {
        let many = if (ordinal as usize) < self.starts.len() {
            self.lists
                .list(self.starts.get(ordinal as usize), self.bound)
        } else {
            List::empty()
        };
        Postings { one: None, many }
    }

    fn sample(&self, i: usize) -> &[u8] {
        let ends = self.sample_ends.as_slice();
        let start = i.checked_sub(1).map_or(0, |p| ends[p] as usize);
        self.samples
            .as_ref()
            .get(start..ends[i] as usize)
            .unwrap_or(&[])
    }

    /// The first ordinal whose term key is not below `bound`.
    fn lower_bound(&self, bound: &[u8]) -> u32 {
        let n = self.len() as usize;
        let (mut below, mut above) = (0, self.sample_ends.len());
        while below < above {
            let mid = below + (above - below) / 2;
            if self.sample(mid) < bound {
                below = mid + 1;
            } else {
                above = mid;
            }
        }
        let (mut lo, mut hi) = match below {
            0 => (0, 0),
            b => ((b - 1) * SAMPLE + 1, (b * SAMPLE).min(n)),
        };
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            if term_cmp(self.text(mid as u32).as_bytes(), bound) == std::cmp::Ordering::Less {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        lo as u32
    }

    /// The ordinal of the word with term key `key`.
    pub(crate) fn ordinal(&self, key: &[u8]) -> Option<u32> {
        let o = self.lower_bound(key);
        (o < self.len() && term_cmp(self.text(o).as_bytes(), key) == std::cmp::Ordering::Equal)
            .then_some(o)
    }

    /// Ordinals of the words whose term key starts with `prefix`.
    pub(crate) fn range(&self, prefix: &[u8]) -> std::ops::Range<u32> {
        let start = self.lower_bound(prefix);
        let mut successor = prefix.to_vec();
        while successor.last() == Some(&u8::MAX) {
            successor.pop();
        }
        let end = match successor.last_mut() {
            Some(last) => {
                *last += 1;
                self.lower_bound(&successor)
            }
            None => self.len(),
        };
        start..end.max(start)
    }

    pub(super) fn size(&self) -> usize {
        self.texts.data.as_ref().len()
            + self.texts.offsets.len() * 4
            + self.sample_ends.len() * 4
            + self.samples.as_ref().len()
            + self.starts.size()
            + self.lists.size()
            + self.freqs.size()
    }
}

fn term_key_of(word: &[u8]) -> Vec<u8> {
    let mut key = word.to_vec();
    key.push(TERMINATOR);
    key
}

/// Words whose term key starts with a prefix, in order.
pub(crate) struct WordCursor<'a> {
    words: &'a Words,
    at: u32,
    end: u32,
    key: Vec<u8>,
}

impl<'a> WordCursor<'a> {
    pub(crate) fn new(words: &'a Words, prefix: &[u8]) -> Self {
        let range = words.range(prefix);
        Self {
            words,
            at: range.start,
            end: range.end,
            key: Vec::new(),
        }
    }

    pub(crate) fn advance(&mut self) -> bool {
        if self.at >= self.end {
            return false;
        }
        self.key.clear();
        self.key
            .extend_from_slice(self.words.text(self.at).as_bytes());
        self.key.push(TERMINATOR);
        self.at += 1;
        true
    }

    pub(crate) fn key(&self) -> &[u8] {
        &self.key
    }

    /// The current word's ordinal.
    pub(crate) fn value(&self) -> u64 {
        u64::from(self.at - 1)
    }
}
