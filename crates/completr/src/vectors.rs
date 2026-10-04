//! Quantised embeddings per segment: a TurboQuant index (turbovec) searched under a live mask.
//!
//! TurboQuant is data-oblivious, so codes from different segments are compatible and compaction
//! concatenates them instead of re-encoding from floats.

use std::cell::OnceCell;
use std::sync::{Arc, Mutex, OnceLock};

use rustc_hash::FxHashMap;
use turbovec::TurboQuantIndex;

use crate::codec::{Column, Reader, Writer};
use crate::Error;

pub(crate) const MAX_DIM: usize = turbovec::MAX_DIM;

/// Vector codes carried from other segments into a merged one: `id -> (code, scale)`.
pub(crate) type CarriedCodes = FxHashMap<u64, (Vec<u8>, f32)>;

/// A document's embedding as given (floats) or as carried over from another segment (codes).
pub(crate) enum Row<'a> {
    Float(&'a [f32]),
    Code(&'a [u8], f32),
}

pub(crate) struct Vectors {
    codes: Column<u8>,
    scales: Column<f32>,
    /// Built from the codes on the first search, as it copies them.
    index: OnceLock<Result<TurboQuantIndex, String>>,
    dim: usize,
    bits: u8,
    /// Local id of each slot, ascending.
    slot_locals: Column<u32>,
}

pub(crate) fn validate_bits(bits: u8) -> Result<(), Error> {
    if (2..=4).contains(&bits) {
        Ok(())
    } else {
        Err(Error::input(format!(
            "vector bits must be 2, 3 or 4, not {bits}"
        )))
    }
}

pub(crate) fn validate_dim(dim: usize) -> Result<(), Error> {
    if dim > 0 && dim.is_multiple_of(8) && dim <= MAX_DIM {
        Ok(())
    } else {
        Err(Error::input(format!(
            "vector dimension must be a positive multiple of 8 up to {MAX_DIM}, not {dim}"
        )))
    }
}

fn code_bytes(dim: usize, bits: u8) -> usize {
    dim / 8 * bits as usize
}

impl Vectors {
    /// Writes the vector section; `rows` are `(local, row)` in ascending local order.
    pub(crate) fn write(
        w: &mut Writer,
        dim: usize,
        bits: u8,
        rows: &[(u32, Row)],
    ) -> Result<(), Error> {
        if rows.is_empty() {
            w.raw(&[0]);
            return Ok(());
        }
        validate_bits(bits)?;
        validate_dim(dim)?;
        let stride = code_bytes(dim, bits);
        let mut floats = Vec::new();
        for (local, row) in rows {
            match row {
                Row::Float(v) if v.len() != dim => {
                    return Err(Error::input(format!(
                        "vector of local {local} has {} dimensions, not {dim}",
                        v.len()
                    )));
                }
                Row::Float(v) if v.iter().any(|x| !x.is_finite()) => {
                    return Err(Error::input(format!(
                        "vector of local {local} is not finite"
                    )));
                }
                Row::Float(v) => floats.extend_from_slice(v),
                Row::Code(code, _) if code.len() != stride => {
                    return Err(Error::input("vector code of the wrong size"))
                }
                Row::Code(..) => {}
            }
        }
        let encoded = if floats.is_empty() {
            None
        } else {
            let mut index = TurboQuantIndex::new(dim, bits as usize)
                .map_err(|e| Error::input(e.to_string()))?;
            index.add(&floats);
            Some(index)
        };
        let mut codes = Vec::with_capacity(rows.len() * stride);
        let mut scales = Vec::with_capacity(rows.len());
        let mut next_float = 0;
        for (_, row) in rows {
            match row {
                Row::Float(_) => {
                    let index = encoded.as_ref().unwrap();
                    codes.extend_from_slice(
                        &index.packed_codes()[next_float * stride..(next_float + 1) * stride],
                    );
                    scales.push(index.scales()[next_float]);
                    next_float += 1;
                }
                Row::Code(code, scale) => {
                    codes.extend_from_slice(code);
                    scales.push(*scale);
                }
            }
        }
        w.raw(&[1, bits]);
        w.u64(dim as u64);
        w.column(&rows.iter().map(|(local, _)| *local).collect::<Vec<_>>());
        w.column(&codes);
        w.column(&scales);
        Ok(())
    }

    pub(crate) fn read(r: &mut Reader, docs: usize, full: bool) -> Result<Option<Self>, Error> {
        if r.u8()? == 0 {
            r.align()?;
            return Ok(None);
        }
        let bits = r.u8()?;
        let dim = usize::try_from(r.u64()?)
            .map_err(|_| Error::Corrupt("vector dimension overflow".into()))?;
        let slot_locals: Column<u32> = r.column()?;
        let codes: Column<u8> = r.column()?;
        let scales: Column<f32> = r.column()?;
        let bad = |what: &str| Error::Corrupt(format!("invalid vectors: {what}"));
        validate_bits(bits).map_err(|_| bad("bits"))?;
        validate_dim(dim).map_err(|_| bad("dimension"))?;
        let n = slot_locals.len();
        let locals = slot_locals.as_slice();
        if scales.len() != n
            || codes.len() != n * code_bytes(dim, bits)
            || locals.last().is_some_and(|&l| l as usize >= docs)
            || full
                && (locals.windows(2).any(|w| w[0] >= w[1])
                    || scales.as_slice().iter().any(|s| !s.is_finite()))
        {
            return Err(bad("sizes"));
        }
        Ok(Some(Self {
            codes,
            scales,
            index: OnceLock::new(),
            dim,
            bits,
            slot_locals,
        }))
    }

    pub(crate) fn dim(&self) -> usize {
        self.dim
    }

    pub(crate) fn bits(&self) -> u8 {
        self.bits
    }

    pub(crate) fn locals(&self) -> &[u32] {
        self.slot_locals.as_slice()
    }

    pub(crate) fn size(&self) -> usize {
        self.slot_locals.len() * (4 + 4 + code_bytes(self.dim, self.bits))
    }

    /// The stored code and scale of `local`, if it has a vector.
    pub(crate) fn row(&self, local: u32) -> Option<(&[u8], f32)> {
        let slot = self.locals().binary_search(&local).ok()?;
        let stride = code_bytes(self.dim, self.bits);
        Some((
            self.codes
                .as_slice()
                .get(slot * stride..(slot + 1) * stride)?,
            *self.scales.as_slice().get(slot)?,
        ))
    }

    /// Up to `k` `(local, score)` among slots allowed by `mask`, best first.
    pub(crate) fn search(
        &self,
        query: &[f32],
        k: usize,
        mask: &[bool],
        threads: usize,
    ) -> Result<Vec<(u32, f32)>, Error> {
        let results = in_pool(threads, || {
            self.index().and_then(|index| {
                index
                    .try_search_with_mask(query, k, Some(mask))
                    .map_err(|e| Error::input(e.to_string()))
            })
        })?;
        let locals = self.locals();
        Ok(results
            .scores_for_query(0)
            .iter()
            .zip(results.indices_for_query(0))
            .filter(|(_, &slot)| slot >= 0)
            .map(|(&score, &slot)| (locals[slot as usize], score))
            .collect())
    }

    /// The search index, built from the codes on first use.
    pub(crate) fn index(&self) -> Result<&TurboQuantIndex, Error> {
        self.index
            .get_or_init(|| {
                let index = TurboQuantIndex::from_parts(
                    Some(self.dim),
                    self.bits as usize,
                    self.slot_locals.len(),
                    self.codes.as_slice().to_vec(),
                    self.scales.as_slice().to_vec(),
                    Vec::new(),
                    Vec::new(),
                )
                .map_err(|e| e.to_string())?;
                index.prepare();
                Ok(index)
            })
            .as_ref()
            .map_err(|e| Error::Corrupt(format!("invalid vectors: {e}")))
    }
}

thread_local! {
    /// A one-thread pool on the calling thread itself, so a query runs inline without a hand-off.
    static INLINE: OnceCell<rayon::ThreadPool> = const { OnceCell::new() };
}

/// Runs `f` inline (`threads == 1`), on rayon's global pool (`0`), or on a shared pool of `threads`.
pub(crate) fn in_pool<R: Send>(threads: usize, f: impl FnOnce() -> R + Send) -> R {
    match threads {
        0 => f(),
        1 => INLINE.with(|cell| {
            let pool = cell.get_or_init(|| {
                rayon::ThreadPoolBuilder::new()
                    .num_threads(1)
                    .use_current_thread()
                    .build()
                    .expect("failed to create the inline vector search pool")
            });
            pool.install(f)
        }),
        n => {
            static POOLS: OnceLock<Mutex<FxHashMap<usize, Arc<rayon::ThreadPool>>>> =
                OnceLock::new();
            let pool = POOLS
                .get_or_init(Mutex::default)
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .entry(n)
                .or_insert_with(|| {
                    Arc::new(
                        rayon::ThreadPoolBuilder::new()
                            .num_threads(n)
                            .build()
                            .expect("failed to create a vector search pool"),
                    )
                })
                .clone();
            pool.install(f)
        }
    }
}
