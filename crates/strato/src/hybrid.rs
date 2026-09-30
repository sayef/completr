//! Hybrid search: lexical autocomplete and vector search fused into one ranking chosen by the
//! caller.

use std::cmp::Ordering;

use rustc_hash::FxHashMap;

use crate::{Error, Hit, Index, MatchKind};

/// How lexical and semantic results are combined.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Fusion {
    /// Reciprocal rank fusion: each list adds `1 / (k + rank)`, ranks from 1. Needs no score
    /// calibration; `k = 60` is the common choice.
    ReciprocalRank { k: f64 },
    /// `(1 - semantic_weight) * lexical + semantic_weight * max(semantic, 0)`. Lexical scores are
    /// in `[0, 1]`; semantic ones are inner products, cosine for normalised vectors.
    Weighted { semantic_weight: f64 },
    /// Lexical hits in their order, then semantic hits not found lexically.
    LexicalFirst,
}

impl Default for Fusion {
    fn default() -> Self {
        Self::ReciprocalRank { k: 60.0 }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct HybridOptions {
    pub fusion: Fusion,
    /// Candidates taken from each side before fusing; `None` uses `max(2 * limit, 20)`.
    pub candidates: Option<usize>,
}

impl HybridOptions {
    pub(crate) fn candidates(&self, limit: usize) -> usize {
        self.candidates.unwrap_or((2 * limit).max(20)).max(limit)
    }

    pub(crate) fn validate(&self) -> Result<(), Error> {
        match self.fusion {
            Fusion::ReciprocalRank { k } if !(k.is_finite() && k >= 0.0) => {
                Err(Error::input("rrf k must be finite and non-negative"))
            }
            Fusion::Weighted { semantic_weight } if !(0.0..=1.0).contains(&semantic_weight) => {
                Err(Error::input("semantic_weight must be within [0, 1]"))
            }
            _ => Ok(()),
        }
    }
}

/// A fused result. `kind` is the lexical match kind when the document matched lexically,
/// otherwise [`MatchKind::Semantic`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HybridHit {
    pub id: u64,
    pub score: f64,
    pub kind: MatchKind,
    pub lexical_score: Option<f64>,
    pub semantic_score: Option<f64>,
    /// Layer of the hit that decided `kind`, for engine searches.
    pub layer: usize,
}

/// Fuses ranked lexical and semantic lists of `(hit, layer)`, best first, ties by id.
pub(crate) fn fuse(
    lexical: &[(Hit, usize)],
    semantic: &[(Hit, usize)],
    limit: usize,
    fusion: Fusion,
) -> Vec<HybridHit> {
    struct Entry {
        hit: HybridHit,
        lexical_rank: Option<usize>,
        semantic_rank: Option<usize>,
    }
    let mut entries: Vec<Entry> = Vec::with_capacity(lexical.len() + semantic.len());
    let mut position: FxHashMap<u64, usize> = FxHashMap::default();
    for (rank, (hit, layer)) in lexical.iter().enumerate() {
        position.insert(hit.id, entries.len());
        entries.push(Entry {
            hit: HybridHit {
                id: hit.id,
                score: 0.0,
                kind: hit.kind,
                lexical_score: Some(hit.score),
                semantic_score: None,
                layer: *layer,
            },
            lexical_rank: Some(rank + 1),
            semantic_rank: None,
        });
    }
    for (rank, (hit, layer)) in semantic.iter().enumerate() {
        match position.get(&hit.id) {
            Some(&i) => {
                entries[i].hit.semantic_score = Some(hit.score);
                entries[i].semantic_rank = Some(rank + 1);
            }
            None => {
                position.insert(hit.id, entries.len());
                entries.push(Entry {
                    hit: HybridHit {
                        id: hit.id,
                        score: 0.0,
                        kind: MatchKind::Semantic,
                        lexical_score: None,
                        semantic_score: Some(hit.score),
                        layer: *layer,
                    },
                    lexical_rank: None,
                    semantic_rank: Some(rank + 1),
                });
            }
        }
    }
    for entry in &mut entries {
        entry.hit.score = match fusion {
            Fusion::ReciprocalRank { k } => {
                let part = |rank: Option<usize>| rank.map_or(0.0, |r| 1.0 / (k + r as f64));
                part(entry.lexical_rank) + part(entry.semantic_rank)
            }
            Fusion::Weighted { semantic_weight } => {
                (1.0 - semantic_weight) * entry.hit.lexical_score.unwrap_or(0.0)
                    + semantic_weight * entry.hit.semantic_score.unwrap_or(0.0).max(0.0)
            }
            // Lexical hits keep their order above every semantic-only hit.
            Fusion::LexicalFirst => match entry.lexical_rank {
                Some(r) => 2.0 + 1.0 / r as f64,
                None => entry.semantic_rank.map_or(0.0, |r| 1.0 / r as f64),
            },
        };
    }
    let mut hits: Vec<HybridHit> = entries.into_iter().map(|e| e.hit).collect();
    hits.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(Ordering::Equal)
            .then(a.id.cmp(&b.id))
    });
    hits.truncate(limit);
    hits
}

impl Index {
    /// Lexical autocomplete on `text` fused with vector search on `vector`.
    pub fn hybrid_search(
        &self,
        text: &str,
        vector: &[f32],
        limit: usize,
        options: HybridOptions,
    ) -> Result<Vec<HybridHit>, Error> {
        options.validate()?;
        let n = options.candidates(limit);
        let lexical: Vec<(Hit, usize)> = self
            .autocomplete(text, n)
            .into_iter()
            .map(|h| (h, 0))
            .collect();
        let semantic: Vec<(Hit, usize)> = self
            .vector_search(vector, n)?
            .into_iter()
            .map(|h| (h, 0))
            .collect();
        Ok(fuse(&lexical, &semantic, limit, options.fusion))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(id: u64, score: f64, kind: MatchKind) -> (Hit, usize) {
        (Hit { id, score, kind }, 0)
    }

    #[test]
    fn fusions_combine_as_documented() {
        let lexical = [
            hit(1, 0.9, MatchKind::Prefix),
            hit(2, 0.5, MatchKind::Infix),
        ];
        let semantic = [
            hit(3, 0.95, MatchKind::Semantic),
            hit(2, 0.8, MatchKind::Semantic),
        ];

        let rrf = fuse(&lexical, &semantic, 10, Fusion::ReciprocalRank { k: 60.0 });
        assert_eq!(rrf.iter().map(|h| h.id).collect::<Vec<_>>(), [2, 1, 3]);
        assert_eq!(
            (rrf[0].kind, rrf[0].lexical_score, rrf[0].semantic_score),
            (MatchKind::Infix, Some(0.5), Some(0.8))
        );
        assert_eq!(rrf[2].kind, MatchKind::Semantic);

        let weighted = fuse(
            &lexical,
            &semantic,
            10,
            Fusion::Weighted {
                semantic_weight: 0.5,
            },
        );
        assert_eq!(weighted.iter().map(|h| h.id).collect::<Vec<_>>(), [2, 3, 1]);
        assert!((weighted[0].score - 0.65).abs() < 1e-12);

        let lexical_first = fuse(&lexical, &semantic, 10, Fusion::LexicalFirst);
        assert_eq!(
            lexical_first.iter().map(|h| h.id).collect::<Vec<_>>(),
            [1, 2, 3]
        );
        assert_eq!(fuse(&lexical, &semantic, 1, Fusion::LexicalFirst).len(), 1);
    }

    #[test]
    fn ties_break_by_id() {
        let lexical = [hit(9, 0.5, MatchKind::Prefix)];
        let semantic = [hit(4, 0.5, MatchKind::Semantic)];
        let fused = fuse(&lexical, &semantic, 10, Fusion::ReciprocalRank { k: 60.0 });
        assert_eq!(fused.iter().map(|h| h.id).collect::<Vec<_>>(), [4, 9]);
    }
}
