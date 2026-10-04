//! A static LOUDS trie in the style of marisa-trie, read in place.
//!
//! Edges are path-compressed: a node's edge is its label byte plus an optional tail. Tails are
//! stored once, sharing storage when one is a suffix of another. Keys iterate in byte order.

use rayon::slice::ParallelSliceMut;

use crate::codec::{BitWriter, Column, Reader, Writer};
use crate::Error;

/// Every this many zeros of the LOUDS bits, the position is sampled for `select0`.
const SELECT_SAMPLE: usize = 64;

/// `SELECT_IN_BYTE[b][k]`: position of the `k`-th set bit of byte `b`.
const SELECT_IN_BYTE: [[u8; 8]; 256] = {
    let mut table = [[8u8; 8]; 256];
    let mut b = 0;
    while b < 256 {
        let (mut k, mut i) = (0, 0);
        while i < 8 {
            if b >> i & 1 == 1 {
                table[b][k] = i as u8;
                k += 1;
            }
            i += 1;
        }
        b += 1;
    }
    table
};

/// Position of the `k`-th set bit of `x`, which has more than `k` set bits.
pub(crate) fn select_in_word(x: u64, mut k: usize) -> usize {
    for byte in 0..8 {
        let b = (x >> (byte * 8)) as u8;
        let count = b.count_ones() as usize;
        if k < count {
            return byte * 8 + SELECT_IN_BYTE[b as usize][k] as usize;
        }
        k -= count;
    }
    64
}

fn bit(words: &[u64], i: usize) -> bool {
    words[i / 64] >> (i % 64) & 1 == 1
}

fn set(words: &mut Vec<u64>, i: usize) {
    if words.len() <= i / 64 {
        words.resize(i / 64 + 1, 0);
    }
    words[i / 64] |= 1 << (i % 64);
}

/// Ones before each word, for rank queries.
fn rank_directory(words: &[u64]) -> Vec<u32> {
    let mut out = Vec::with_capacity(words.len() + 1);
    let mut total = 0u32;
    for w in words {
        out.push(total);
        total += w.count_ones();
    }
    out.push(total);
    out
}

fn rank1(words: &[u64], directory: &[u32], i: usize) -> usize {
    let (word, offset) = (i / 64, i % 64);
    let below = if offset == 0 {
        0
    } else {
        (words[word] & ((1u64 << offset) - 1)).count_ones()
    };
    directory[word] as usize + below as usize
}

/// Per byte: the net `ones - zeros`, and the lowest running balance seen just before a `1`.
const EXCESS: [(i8, i8); 256] = {
    let mut table = [(0i8, i8::MAX); 256];
    let mut b = 0;
    while b < 256 {
        let (mut balance, mut low) = (0i8, i8::MAX);
        let mut i = 0;
        while i < 8 {
            if b >> i & 1 == 1 {
                if balance < low {
                    low = balance;
                }
                balance += 1;
            } else {
                balance -= 1;
            }
            i += 1;
        }
        table[b] = (balance, low);
        b += 1;
    }
    table
};

/// Checks the breadth-first order and samples zero positions, a byte at a time. Returns the
/// samples and the counts of ones and zeros, or `None` if a child precedes its parent.
fn scan_louds(words: &[u64], bits: usize) -> Option<(Vec<u32>, usize, usize)> {
    let mut samples = Vec::new();
    let (mut balance, mut zeros, mut ones) = (0i64, 0usize, 0usize);
    for (w, &word) in words.iter().enumerate() {
        let valid = (bits - w * 64).min(64);
        for byte in 0..valid.div_ceil(8) {
            let len = (valid - byte * 8).min(8);
            let mask = if len == 8 { 0xff } else { (1u32 << len) - 1 };
            let b = (word >> (byte * 8)) as u32 & mask;
            // Missing high bits count as zeros only within `len`.
            let (net, low) = EXCESS[b as usize];
            let net = i64::from(net) + (8 - len as i64);
            if low != i8::MAX && balance + i64::from(low) < 0 {
                return None;
            }
            let byte_ones = b.count_ones() as usize;
            let byte_zeros = len - byte_ones;
            let next_sample = zeros.div_ceil(SELECT_SAMPLE) * SELECT_SAMPLE;
            if byte_zeros > 0 && next_sample < zeros + byte_zeros {
                let mut seen = zeros;
                for i in 0..len {
                    if b >> i & 1 == 0 {
                        if seen % SELECT_SAMPLE == 0 {
                            samples.push((w * 64 + byte * 8 + i) as u32);
                        }
                        seen += 1;
                    }
                }
            }
            balance += net;
            ones += byte_ones;
            zeros += byte_zeros;
        }
    }
    Some((samples, ones, zeros))
}

/// Positions of every `SELECT_SAMPLE`-th set bit among the first `bits` bits.
fn ones_samples(words: &[u64], bits: usize) -> Vec<u32> {
    let mut samples = Vec::new();
    let mut seen = 0usize;
    for (w, &word) in words.iter().enumerate() {
        let valid = (bits.saturating_sub(w * 64)).min(64);
        let word = if valid == 64 {
            word
        } else {
            word & ((1u64 << valid) - 1)
        };
        let count = word.count_ones() as usize;
        let next = seen.div_ceil(SELECT_SAMPLE) * SELECT_SAMPLE;
        if next < seen + count {
            samples.push((w * 64 + select_in_word(word, next - seen)) as u32);
            let mut k = next + SELECT_SAMPLE;
            while k < seen + count {
                samples.push((w * 64 + select_in_word(word, k - seen)) as u32);
                k += SELECT_SAMPLE;
            }
        }
        seen += count;
    }
    samples
}

/// Position of the `k`-th set bit, using `samples` from `ones_samples`.
fn select1(words: &[u64], samples: &[u32], k: usize) -> usize {
    let mut pos = samples[k / SELECT_SAMPLE] as usize;
    let mut remaining = k % SELECT_SAMPLE;
    loop {
        let word = words[pos / 64] >> (pos % 64);
        let count = word.count_ones() as usize;
        if remaining < count {
            return pos + select_in_word(word, remaining);
        }
        remaining -= count;
        pos = (pos / 64 + 1) * 64;
    }
}

/// Bit packed unsigned integers of one width.
pub(crate) struct Packed {
    words: Column<u64>,
    width: u32,
    pub(crate) len: usize,
}

impl Packed {
    pub(crate) fn size(&self) -> usize {
        self.words.len() * 8 + 8
    }

    pub(crate) fn write<T: Copy + Into<u64>>(w: &mut Writer, values: &[T]) {
        Self::write_with(w, values.len(), || values.iter().map(|&v| v.into()));
    }

    /// Writes `len` values; `values` is called twice, for the width and then the bits.
    pub(crate) fn write_with<I: Iterator<Item = u64>>(
        w: &mut Writer,
        len: usize,
        values: impl Fn() -> I,
    ) {
        let width = values()
            .map(|v| 64 - v.leading_zeros())
            .max()
            .unwrap_or(0)
            .max(1);
        w.u64(u64::from(width) | (len as u64) << 8);
        let mut bits = BitWriter::new(w);
        for v in values() {
            if width == 64 {
                bits.push(v & u64::from(u32::MAX), 32);
                bits.push(v >> 32, 32);
            } else {
                bits.push(v, width);
            }
        }
        bits.finish();
    }

    pub(crate) fn read(r: &mut Reader) -> Result<Self, Error> {
        let header = r.u64()?;
        let (width, len) = ((header & 0xff) as u32, (header >> 8) as usize);
        let words = r.column()?;
        let bits = len
            .checked_mul(width as usize)
            .ok_or_else(|| Error::Corrupt("packed overflow".into()))?;
        if !(1..=64).contains(&width) || words.len() != bits.div_ceil(64) {
            return Err(Error::Corrupt("invalid packed array".into()));
        }
        Ok(Self { words, width, len })
    }

    pub(crate) fn get(&self, i: usize) -> u64 {
        let words = self.words.as_slice();
        let at = i * self.width as usize;
        let mask = if self.width == 64 {
            u64::MAX
        } else {
            (1u64 << self.width) - 1
        };
        let mut v = words[at / 64] >> (at % 64);
        if at % 64 + self.width as usize > 64 {
            v |= words[at / 64 + 1] << (64 - at % 64);
        }
        v & mask
    }
}

/// The trie. Node 0 is the root; nodes are numbered in breadth-first order.
pub(crate) struct Trie {
    /// `10` then, per node, one `1` per child and a `0`.
    louds: Column<u64>,
    louds_bits: usize,
    /// Positions of every `SELECT_SAMPLE`-th zero, built at load.
    select_samples: Vec<u32>,
    /// Label byte of node `i + 1`.
    labels: Column<u8>,
    terminal: Column<u64>,
    terminal_rank: Vec<u32>,
    has_tail: Column<u64>,
    tail_rank: Vec<u32>,
    /// Nodes without children, so descending can skip `select0`. A wrong bit only hides children.
    leaf: Column<u64>,
    tails: Tails,
    /// For restoring keys from ids (tail tries only): positions of every `SELECT_SAMPLE`-th one
    /// of the LOUDS bits and of the terminal bits.
    ones_samples: Vec<u32>,
    terminal_samples: Vec<u32>,
    keys: usize,
}

/// Where edge tails live: one shared byte blob, or the keys of a nested trie of reversed tails
/// (smaller, slower to read).
enum Tails {
    Flat {
        offsets: Packed,
        bytes: Column<u8>,
        ends: Column<u64>,
    },
    Nested {
        ids: Packed,
        trie: Box<Trie>,
    },
}

impl Trie {
    /// Writes a trie of `keys`, sorted and unique. A key's id is its node's terminal rank; the
    /// returned vector maps each input index to that id.
    #[cfg(test)]
    pub(crate) fn write(w: &mut Writer, keys: &[&[u8]]) -> Result<Vec<u32>, Error> {
        Self::write_with(w, keys, false)
    }

    /// With `nested`, tails are stored as keys of a second trie instead of a byte blob.
    pub(crate) fn write_with(
        w: &mut Writer,
        keys: &[&[u8]],
        nested: bool,
    ) -> Result<Vec<u32>, Error> {
        let mut louds = Vec::new();
        let mut louds_bits = 0usize;
        let mut push_bit = |bits: &mut Vec<u64>, one: bool| {
            if one {
                set(bits, louds_bits);
            } else if bits.len() <= louds_bits / 64 {
                bits.resize(louds_bits / 64 + 1, 0);
            }
            louds_bits += 1;
        };
        push_bit(&mut louds, true);
        push_bit(&mut louds, false);

        let mut labels = Vec::new();
        let (mut terminal, mut has_tail, mut leaf) = (Vec::new(), Vec::new(), Vec::new());
        let mut node_tails: Vec<&[u8]> = Vec::new();
        let mut ids = vec![0u32; keys.len()];
        let mut terminals = 0u32;
        // (first key, end key, depth) per node, breadth first.
        let mut queue = std::collections::VecDeque::from([(0u32, keys.len() as u32, 0u32)]);
        let mut node = 0usize;
        while let Some((lo, hi, depth)) = queue.pop_front() {
            let (lo, hi, depth) = (lo as usize, hi as usize, depth as usize);
            let mut start = lo;
            if start < hi && keys[start].len() == depth {
                set(&mut terminal, node);
                ids[start] = terminals;
                terminals += 1;
                start += 1;
            }
            if start == hi {
                set(&mut leaf, node);
            }
            let mut i = start;
            while i < hi {
                let label = keys[i][depth];
                let mut j = i + 1;
                while j < hi && keys[j][depth] == label {
                    j += 1;
                }
                let (first, last) = (keys[i], keys[j - 1]);
                let common = first.iter().zip(last).take_while(|(a, b)| a == b).count();
                push_bit(&mut louds, true);
                labels.push(label);
                let child = node + queue.len() + 1;
                if common > depth + 1 {
                    set(&mut has_tail, child);
                    node_tails.push(&first[depth + 1..common]);
                }
                queue.push_back((i as u32, j as u32, common as u32));
                i = j;
            }
            push_bit(&mut louds, false);
            node += 1;
        }
        let nodes = node;

        let bitvec = |words: &mut Vec<u64>, bits: usize| words.resize(bits.div_ceil(64), 0);
        bitvec(&mut terminal, nodes);
        bitvec(&mut has_tail, nodes);
        bitvec(&mut leaf, nodes);
        w.u64(nodes as u64);
        w.u64(u64::from(terminals));
        w.u64(louds_bits as u64);
        w.column(&louds);
        w.column(&labels);
        w.column(&terminal);
        w.column(&has_tail);
        w.column(&leaf);
        w.align();
        drop((louds, labels, terminal, has_tail, leaf));

        // Store each tail once; a tail that ends another shares its bytes.
        // Sorted by reversed bytes and walked backwards, each tail follows the tail it may end.
        let rev_cmp = |a: &[u8], b: &[u8]| a.iter().rev().cmp(b.iter().rev());
        // Node tails sorted by reversed bytes, grouped into unique tails in ascending order.
        let tail = |t: u32| node_tails[t as usize];
        let mut order: Vec<u32> = (0..node_tails.len() as u32).collect();
        order.par_sort_unstable_by(|&a, &b| rev_cmp(tail(a), tail(b)));
        let mut unique_of = vec![0u32; node_tails.len()];
        let mut unique: Vec<u32> = Vec::new();
        for &i in &order {
            if unique.last().is_none_or(|&u| tail(u) != tail(i)) {
                unique.push(i);
            }
            unique_of[i as usize] = unique.len() as u32 - 1;
        }
        drop(order);
        if nested {
            let reversed: Vec<Vec<u8>> = unique
                .iter()
                .map(|&u| tail(u).iter().rev().copied().collect())
                .collect();
            let refs: Vec<&[u8]> = reversed.iter().map(Vec::as_slice).collect();
            let mut inner = Writer::default();
            let ids = Self::write_with(&mut inner, &refs, false)?;
            let tail_ids: Vec<u64> = unique_of
                .iter()
                .map(|&u| u64::from(ids[u as usize]))
                .collect();
            w.raw(&[1]);
            Packed::write(w, &tail_ids);
            w.align();
            w.raw(&inner.buf);
        } else {
            // Walked from the largest reversed tail down, each tail is a suffix of the previous one
            // when it ends that tail.
            let mut blob: Vec<u8> = Vec::new();
            let mut ends = Vec::new();
            let mut offset_of = vec![0u32; unique.len()];
            let mut last: Option<(&[u8], usize)> = None;
            for (u, &i) in unique.iter().enumerate().rev() {
                let tail = tail(i);
                let offset = match last {
                    Some((prev, at)) if prev.ends_with(tail) => at + prev.len() - tail.len(),
                    _ => {
                        let at = blob.len();
                        blob.extend_from_slice(tail);
                        set(&mut ends, blob.len() - 1);
                        last = Some((tail, at));
                        at
                    }
                };
                offset_of[u] =
                    u32::try_from(offset).map_err(|_| Error::input("trie tails over 4 GiB"))?;
            }
            let tail_offsets: Vec<u32> = unique_of.iter().map(|&u| offset_of[u as usize]).collect();
            ends.resize(blob.len().div_ceil(64), 0);
            w.raw(&[0]);
            Packed::write(w, &tail_offsets);
            w.column(&blob);
            w.column(&ends);
        }
        w.align();
        Ok(ids)
    }

    pub(crate) fn read(r: &mut Reader) -> Result<Self, Error> {
        let bad = |what: &str| Error::Corrupt(format!("invalid trie: {what}"));
        let nodes = usize::try_from(r.u64()?).map_err(|_| bad("size"))?;
        let keys = usize::try_from(r.u64()?).map_err(|_| bad("size"))?;
        let louds_bits = usize::try_from(r.u64()?).map_err(|_| bad("size"))?;
        let louds: Column<u64> = r.column()?;
        let labels: Column<u8> = r.column()?;
        let terminal: Column<u64> = r.column()?;
        let has_tail: Column<u64> = r.column()?;
        let leaf: Column<u64> = r.column()?;
        r.align()?;
        let tails = match r.u8()? {
            0 => {
                let offsets = Packed::read(r)?;
                let bytes: Column<u8> = r.column()?;
                let ends: Column<u64> = r.column()?;
                let e = ends.as_slice();
                let stray = !bytes.len().is_multiple_of(64)
                    && e.last().is_some_and(|w| w >> (bytes.len() % 64) != 0);
                let valid = ends.len() == bytes.len().div_ceil(64)
                    && !stray
                    && (bytes.len() == 0 || bit(e, bytes.len() - 1));
                if !valid {
                    return Err(bad("tail blob"));
                }
                Tails::Flat {
                    offsets,
                    bytes,
                    ends,
                }
            }
            1 => {
                let ids = Packed::read(r)?;
                r.align()?;
                Tails::Nested {
                    ids,
                    trie: Box::new(Self::read_nested(r)?),
                }
            }
            _ => return Err(bad("tail kind")),
        };
        r.align()?;

        let words = louds.as_slice();
        if nodes == 0
            || Some(louds_bits) != nodes.checked_mul(2).and_then(|d| d.checked_add(1))
            || words.len() != louds_bits.div_ceil(64)
            || labels.len() != nodes - 1
            || terminal.len() != nodes.div_ceil(64)
            || has_tail.len() != nodes.div_ceil(64)
            || leaf.len() != nodes.div_ceil(64)
            || (!louds_bits.is_multiple_of(64) && words[words.len() - 1] >> (louds_bits % 64) != 0)
        {
            return Err(bad("sizes"));
        }
        // Children must come after their parent in breadth-first order, which rules out cycles:
        // before every `1`, ones seen must be at least zeros seen.
        let (select_samples, ones, zeros) =
            scan_louds(words, louds_bits).ok_or_else(|| bad("order"))?;
        if ones != nodes || zeros != nodes + 1 || !bit(words, 0) || bit(words, 1) {
            return Err(bad("shape"));
        }
        let terminal_rank = rank_directory(terminal.as_slice());
        let tail_rank = rank_directory(has_tail.as_slice());
        let tail_count = match &tails {
            Tails::Flat { offsets, .. } => offsets.len,
            Tails::Nested { ids, .. } => ids.len,
        };
        // Tail references are bounds-checked when read, not here, to keep loading O(words).
        if terminal_rank[terminal_rank.len() - 1] as usize != keys
            || tail_rank[tail_rank.len() - 1] as usize != tail_count
        {
            return Err(bad("tails or terminals"));
        }
        Ok(Self {
            louds,
            louds_bits,
            select_samples,
            labels,
            terminal,
            terminal_rank,
            has_tail,
            tail_rank,
            leaf,
            tails,
            ones_samples: Vec::new(),
            terminal_samples: Vec::new(),
            keys,
        })
    }

    /// A trie used for tails: also indexed for restoring keys from ids.
    fn read_nested(r: &mut Reader) -> Result<Self, Error> {
        let mut trie = Self::read(r)?;
        trie.ones_samples = ones_samples(trie.louds.as_slice(), trie.louds_bits);
        trie.terminal_samples = ones_samples(trie.terminal.as_slice(), trie.terminal.len() * 64);
        Ok(trie)
    }

    pub(crate) fn len(&self) -> usize {
        self.keys
    }

    pub(crate) fn size(&self) -> usize {
        self.louds.len() * 8
            + self.labels.len()
            + (self.terminal.len() + self.has_tail.len() + self.leaf.len()) * 8
            + match &self.tails {
                Tails::Flat {
                    offsets,
                    bytes,
                    ends,
                } => offsets.size() + bytes.len() + ends.len() * 8,
                Tails::Nested { ids, trie } => ids.size() + trie.size(),
            }
            + (self.select_samples.len() + self.terminal_rank.len() + self.tail_rank.len()) * 4
            + (self.ones_samples.len() + self.terminal_samples.len()) * 4
    }

    /// Position of the `z`-th zero of the LOUDS bits.
    #[inline]
    fn select0(&self, z: usize) -> usize {
        let words = self.louds.as_slice();
        let mut pos = self.select_samples[z / SELECT_SAMPLE] as usize;
        let mut remaining = z % SELECT_SAMPLE;
        loop {
            let word = pos / 64;
            let zeros = !words[word] >> (pos % 64);
            let zeros = if word == words.len() - 1 && !self.louds_bits.is_multiple_of(64) {
                zeros & ((1u64 << (self.louds_bits % 64 - pos % 64)) - 1)
            } else {
                zeros
            };
            let count = zeros.count_ones() as usize;
            if remaining < count {
                return pos + select_in_word(zeros, remaining);
            }
            remaining -= count;
            pos = (word + 1) * 64;
        }
    }

    /// First child id and child count of `node`: the run of ones after its zero.
    #[inline]
    fn children(&self, node: usize) -> (usize, usize) {
        let words = self.louds.as_slice();
        let start = self.select0(node) + 1;
        let mut end = start;
        loop {
            let run = (words[end / 64] >> (end % 64)).trailing_ones() as usize;
            end += run;
            // Continue only if the run reached the end of the word.
            if run == 0 || !end.is_multiple_of(64) {
                break;
            }
        }
        (start - node - 1, end - start)
    }

    fn label(&self, node: usize) -> u8 {
        self.labels.as_slice()[node - 1]
    }

    /// The tail of `node`; `scratch` holds it when it has to be restored from a nested trie.
    #[inline]
    fn tail<'a>(&'a self, node: usize, scratch: &'a mut Vec<u8>) -> &'a [u8] {
        let has = self.has_tail.as_slice();
        if !bit(has, node) {
            return &[];
        }
        self.tail_at(rank1(has, &self.tail_rank, node), scratch)
    }

    /// The `index`-th tail, in node order.
    #[inline]
    fn tail_at<'a>(&'a self, index: usize, scratch: &'a mut Vec<u8>) -> &'a [u8] {
        match &self.tails {
            Tails::Flat {
                offsets,
                bytes,
                ends,
            } => {
                let start = offsets.get(index) as usize;
                if start >= bytes.len() {
                    return &[];
                }
                let ends = ends.as_slice();
                let mut i = start;
                loop {
                    let word = ends[i / 64] >> (i % 64);
                    if word != 0 {
                        let end = i + word.trailing_zeros() as usize;
                        return &bytes.as_slice()[start..=end];
                    }
                    i = (i / 64 + 1) * 64;
                }
            }
            Tails::Nested { ids, trie } => {
                scratch.clear();
                trie.restore_reversed(ids.get(index) as usize, scratch);
                scratch
            }
        }
    }

    /// Appends the key with id `id`, reversed. Nested tries store reversed tails, so this is the tail.
    fn restore_reversed(&self, id: usize, out: &mut Vec<u8>) {
        if id >= self.keys {
            return;
        }
        let mut node = select1(self.terminal.as_slice(), &self.terminal_samples, id);
        let mut scratch = Vec::new();
        while node != 0 {
            let tail = self.tail(node, &mut scratch);
            out.extend(tail.iter().rev());
            out.push(self.label(node));
            // The node's `1` sits after `pos - node` zeros; its parent owns the last of them.
            let pos = select1(self.louds.as_slice(), &self.ones_samples, node);
            node = pos - node - 1;
        }
    }

    fn key_id(&self, node: usize) -> Option<usize> {
        let terminal = self.terminal.as_slice();
        bit(terminal, node).then(|| rank1(terminal, &self.terminal_rank, node))
    }

    fn is_leaf(&self, node: usize) -> bool {
        bit(self.leaf.as_slice(), node)
    }

    /// The child of `node` whose label is `byte`.
    fn child(&self, node: usize, byte: u8) -> Option<usize> {
        if self.is_leaf(node) {
            return None;
        }
        let (first, count) = self.children(node);
        let labels = &self.labels.as_slice()[first - 1..first - 1 + count];
        labels.binary_search(&byte).ok().map(|i| first + i)
    }

    pub(crate) fn get(&self, key: &[u8]) -> Option<usize> {
        let (mut node, mut pos) = (0usize, 0usize);
        let mut scratch = Vec::new();
        while pos < key.len() {
            node = self.child(node, key[pos])?;
            pos += 1;
            let tail = self.tail(node, &mut scratch);
            if !key[pos..].starts_with(tail) {
                return None;
            }
            pos += tail.len();
        }
        self.key_id(node)
    }

    /// Keys starting with `prefix` in byte order, with their ids.
    pub(crate) fn cursor(&self, prefix: &[u8]) -> TrieCursor<'_> {
        let mut cursor = TrieCursor {
            trie: self,
            stack: Vec::new(),
            key: Vec::new(),
            pending: None,
            scratch: Vec::new(),
        };
        let (mut node, mut pos) = (0usize, 0usize);
        let mut scratch = Vec::new();
        while pos < prefix.len() {
            let Some(child) = self.child(node, prefix[pos]) else {
                return cursor;
            };
            let tail = self.tail(child, &mut scratch);
            let rest = &prefix[pos + 1..];
            let matched = rest.len().min(tail.len());
            if rest[..matched] != tail[..matched] {
                return cursor;
            }
            cursor.key.extend_from_slice(&prefix[pos..pos + 1]);
            cursor.key.extend_from_slice(tail);
            node = child;
            pos += 1 + tail.len();
        }
        cursor.enter(node);
        cursor
    }
}

pub(crate) struct TrieCursor<'a> {
    trie: &'a Trie,
    stack: Vec<Frame>,
    key: Vec<u8>,
    pending: Option<usize>,
    scratch: Vec<u8>,
}

/// An open node: its remaining children, the key length at it, and the running tail and key
/// ranks of its next child, so siblings need no rank queries.
struct Frame {
    next: usize,
    end: usize,
    len: usize,
    tail: usize,
    term: usize,
}

impl TrieCursor<'_> {
    fn open(&mut self, node: usize) {
        if !self.trie.is_leaf(node) {
            let (first, count) = self.trie.children(node);
            let trie = self.trie;
            self.stack.push(Frame {
                next: first,
                end: first + count,
                len: self.key.len(),
                tail: rank1(trie.has_tail.as_slice(), &trie.tail_rank, first),
                term: rank1(trie.terminal.as_slice(), &trie.terminal_rank, first),
            });
        }
    }

    fn enter(&mut self, node: usize) {
        self.pending = self.trie.key_id(node);
        self.open(node);
    }

    /// The next key and its id, in byte order.
    #[cfg(test)]
    pub(crate) fn next(&mut self) -> Option<(&[u8], usize)> {
        let id = self.advance()?;
        Some((&self.key, id))
    }

    /// The current key, after `advance` returned an id.
    pub(crate) fn key(&self) -> &[u8] {
        &self.key
    }

    /// Moves to the next key in byte order and returns its id.
    pub(crate) fn advance(&mut self) -> Option<usize> {
        let trie = self.trie;
        loop {
            if let Some(id) = self.pending.take() {
                return Some(id);
            }
            let frame = self.stack.last_mut()?;
            if frame.next == frame.end {
                self.stack.pop();
                continue;
            }
            let child = frame.next;
            frame.next += 1;
            let tail = if bit(trie.has_tail.as_slice(), child) {
                frame.tail += 1;
                trie.tail_at(frame.tail - 1, &mut self.scratch)
            } else {
                &[]
            };
            let id = bit(trie.terminal.as_slice(), child).then(|| {
                frame.term += 1;
                frame.term - 1
            });
            let base = frame.len;
            self.key.truncate(base);
            self.key.push(trie.label(child));
            self.key.extend_from_slice(tail);
            self.pending = id;
            self.open(child);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::Bytes;

    fn build(keys: &[&[u8]], nested: bool) -> (Trie, Vec<u32>) {
        let mut w = Writer::default();
        let ids = Trie::write_with(&mut w, keys, nested).unwrap();
        (
            Trie::read(&mut Reader::new(Bytes::from_vec(w.buf)).unwrap()).unwrap(),
            ids,
        )
    }

    fn check(keys: &[Vec<u8>]) {
        check_mode(keys, false);
        check_mode(keys, true);
    }

    fn check_mode(keys: &[Vec<u8>], nested: bool) {
        let mut sorted: Vec<&[u8]> = keys.iter().map(|k| k.as_slice()).collect();
        sorted.sort_unstable();
        sorted.dedup();
        let (trie, ids) = build(&sorted, nested);
        assert_eq!(trie.len(), sorted.len());
        let mut unique_ids = ids.clone();
        unique_ids.sort_unstable();
        unique_ids.dedup();
        assert_eq!(unique_ids.len(), sorted.len());
        for (i, key) in sorted.iter().enumerate() {
            assert_eq!(trie.get(key), Some(ids[i] as usize), "{key:?}");
            let mut shorter = key.to_vec();
            if shorter.pop().is_some() && sorted.binary_search(&shorter.as_slice()).is_err() {
                assert_eq!(trie.get(&shorter), None);
            }
        }
        let mut prefixes: Vec<Vec<u8>> = vec![Vec::new(), b"zzz".to_vec(), vec![0xff]];
        for key in &sorted {
            for n in 0..=key.len().min(4) {
                prefixes.push(key[..n].to_vec());
            }
            prefixes.push([&key[..], b"x"].concat());
        }
        for prefix in prefixes {
            let mut cursor = trie.cursor(&prefix);
            let mut got = Vec::new();
            while let Some((key, id)) = cursor.next() {
                got.push((key.to_vec(), id));
            }
            let want: Vec<(Vec<u8>, usize)> = sorted
                .iter()
                .enumerate()
                .filter(|(_, k)| k.starts_with(&prefix))
                .map(|(i, k)| (k.to_vec(), ids[i] as usize))
                .collect();
            assert_eq!(got, want, "prefix {prefix:?}");
        }
    }

    #[test]
    fn matches_a_sorted_set() {
        let words = [
            "",
            "a",
            "ab",
            "abc",
            "abd",
            "b",
            "data",
            "database",
            "datum",
            "data\u{ff}",
            "ärzte",
            "ärzt",
            "zoo",
        ];
        check(
            &words
                .iter()
                .map(|w| w.as_bytes().to_vec())
                .collect::<Vec<_>>(),
        );
        check(&[b"\xff".to_vec(), b"a\xff".to_vec(), b"a".to_vec()]);
        check(&[b"only".to_vec()]);
        check(&[]);
    }

    #[test]
    fn matches_generated_keys() {
        let mut state = 7u64;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let alphabet = b"abcde\xff";
        let keys: Vec<Vec<u8>> = (0..3000)
            .map(|_| {
                (0..next() % 12)
                    .map(|_| alphabet[(next() % alphabet.len() as u64) as usize])
                    .collect()
            })
            .collect();
        check(&keys);
    }

    /// `COMPLETR_KEYS=<dir> cargo test --release -p completr bench_trie -- --ignored --nocapture`, with
    /// key sets exported as length-prefixed files.
    #[test]
    #[ignore]
    fn bench_trie() {
        use fst::{IntoStreamer, Streamer};
        use std::time::Instant;

        let dir = std::env::var("COMPLETR_KEYS").unwrap();
        let load = |name: &str| -> Vec<Vec<u8>> {
            let data = std::fs::read(format!("{dir}/{name}.keys")).unwrap();
            let mut out = Vec::new();
            let mut pos = 0;
            while pos < data.len() {
                let len = u32::from_le_bytes(data[pos..pos + 4].try_into().unwrap()) as usize;
                out.push(data[pos + 4..pos + 4 + len].to_vec());
                pos += 4 + len;
            }
            out
        };
        let prefixes = load("prefixes");
        for name in ["titles", "aliases", "variants"] {
            let keys = load(name);
            let refs: Vec<&[u8]> = keys.iter().map(|k| k.as_slice()).collect();
            for nested in [false, true] {
                let t = Instant::now();
                let mut w = Writer::default();
                Trie::write_with(&mut w, &refs, nested).unwrap();
                let build = t.elapsed();
                let trie = Trie::read(&mut Reader::new(Bytes::from_vec(w.buf)).unwrap()).unwrap();
                let t = Instant::now();
                let mut hits = 0;
                for _ in 0..3 {
                    for k in refs.iter().step_by(7) {
                        hits += trie.get(k).is_some() as usize;
                    }
                }
                let lookup = t.elapsed().as_nanos() as f64 / hits as f64;
                let t = Instant::now();
                let mut scanned = 0;
                if name != "variants" {
                    for p in &prefixes {
                        let mut cursor = trie.cursor(p);
                        while cursor.next().is_some() {
                            scanned += 1;
                        }
                    }
                }
                let scan = t.elapsed();
                println!(
                "{name:<9} completr trie{} {:6.2} MB  build {:5.0} ms  lookup {lookup:5.0} ns  scan {scanned:7} keys {:6.1} ms",
                if nested { "+" } else { " " },
                trie.size() as f64 / 1_048_576.0,
                build.as_secs_f64() * 1e3,
                scan.as_secs_f64() * 1e3
            );
            }

            let t = Instant::now();
            let map =
                fst::Map::from_iter(refs.iter().enumerate().map(|(i, k)| (*k, i as u64))).unwrap();
            let build = t.elapsed();
            let t = Instant::now();
            let mut hits = 0;
            for _ in 0..3 {
                for k in refs.iter().step_by(7) {
                    hits += map.get(k).is_some() as usize;
                }
            }
            let lookup = t.elapsed().as_nanos() as f64 / hits as f64;
            let t = Instant::now();
            let mut scanned = 0;
            if name != "variants" {
                for p in &prefixes {
                    let mut stream = map.range().ge(p).into_stream();
                    while let Some((k, _)) = stream.next() {
                        if !k.starts_with(p) {
                            break;
                        }
                        scanned += 1;
                    }
                }
            }
            let scan = t.elapsed();
            println!(
                "{name:<9} fst            {:6.2} MB  build {:5.0} ms  lookup {lookup:5.0} ns  scan {scanned:7} keys {:6.1} ms",
                map.as_fst().as_bytes().len() as f64 / 1_048_576.0,
                build.as_secs_f64() * 1e3,
                scan.as_secs_f64() * 1e3
            );
        }
    }

    /// Loops prefix scans for profiling: `COMPLETR_KEYS=<dir> cargo test --release -p completr
    /// profile_scans -- --ignored`.
    #[test]
    #[ignore]
    fn profile_scans() {
        let dir = std::env::var("COMPLETR_KEYS").unwrap();
        let load = |name: &str| -> Vec<Vec<u8>> {
            let data = std::fs::read(format!("{dir}/{name}.keys")).unwrap();
            let (mut out, mut pos) = (Vec::new(), 0);
            while pos < data.len() {
                let len = u32::from_le_bytes(data[pos..pos + 4].try_into().unwrap()) as usize;
                out.push(data[pos + 4..pos + 4 + len].to_vec());
                pos += 4 + len;
            }
            out
        };
        let (prefixes, keys) = (load("prefixes"), load("aliases"));
        let refs: Vec<&[u8]> = keys.iter().map(|k| k.as_slice()).collect();
        let mut w = Writer::default();
        Trie::write(&mut w, &refs).unwrap();
        let trie = Trie::read(&mut Reader::new(Bytes::from_vec(w.buf)).unwrap()).unwrap();
        let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while std::time::Instant::now() < until {
            for p in &prefixes {
                let mut cursor = trie.cursor(p);
                while cursor.next().is_some() {}
            }
        }
    }

    #[test]
    fn rejects_corrupt_input() {
        let keys: Vec<&[u8]> = vec![b"alpha", b"beta", b"gamma"];
        let mut w = Writer::default();
        Trie::write(&mut w, &keys).unwrap();
        for cut in [8, 24, w.buf.len() - 8] {
            let mut data = w.buf.clone();
            data.truncate(cut);
            assert!(Trie::read(&mut Reader::new(Bytes::from_vec(data)).unwrap()).is_err());
        }
    }
}
