//! The embedding tier of entity grounding (Design B; bobbin-tvn rev 2, and the
//! corpus mechanics bobbin-fjh reuses).
//!
//! A comment can reference tracked work with no id at all ("fixes the
//! frontier-recompute problem"). Before any generative model there is a
//! deterministic middle tier: **cosine similarity against an embedded
//! corpus**. Given a pinned embedding model and corpus snapshot the score is
//! reproducible — a falsifier in the catalog's own style — but it is still a
//! classifier, honestly: the tier is `embedding` (reproducible-but-
//! approximate), the placement is **advisory or escalate only, never a hard
//! deny**, and every verdict seals score / threshold / model / corpus
//! watermark, because a score means nothing outside the model and corpus that
//! produced it (the trust-chain rule, applied to embeddings).
//!
//! Yupana never runs an embedding model. The matrix is *projected* into the
//! hot plane (source: bobbin's beads index, or an equivalent embedding pass —
//! the design's step 4), and the query vector arrives embedded by the same
//! model. This module is the pure hot-plane half: hold the matrix, brute-force
//! cosine over a few thousand vectors (microseconds, no ANN infrastructure),
//! seal the verdict. The same discipline as the exact tier: a missing matrix
//! is **unevaluated (loud)**, never satisfied.

use serde::{Deserialize, Serialize};

/// One embedded corpus entry — a work item, a denied-edit verdict, any
/// governed entity similarity may ground against.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CorpusVector {
    /// The entity's stable identifier (`bobbin-bnq`, a verdict spool ref, …).
    pub id: String,
    /// A short human label carried into advisories.
    #[serde(default)]
    pub label: Option<String>,
    /// The embedding, in the matrix's declared model space.
    pub vector: Vec<f32>,
}

/// The projected vector matrix, sealed to the model and corpus snapshot that
/// produced it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VectorMatrix {
    /// The embedding model identity. A score is meaningless outside it, so a
    /// query vector from a different model MUST be refused, not scored.
    pub model: String,
    /// Corpus snapshot watermark (e.g. the beads-index export stamp). Rides
    /// every advisory so a score can be reproduced — or falsified — against
    /// the exact corpus that produced it.
    pub corpus_watermark: String,
    /// The embedded corpus.
    pub vectors: Vec<CorpusVector>,
}

/// One similarity match, sealed with everything needed to reproduce it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SimilarityMatch {
    /// The matched entity.
    pub id: String,
    /// Its label, when the corpus carried one.
    pub label: Option<String>,
    /// `cosine(query, id)` — the falsifiable number.
    pub score: f32,
    /// The threshold the match cleared.
    pub threshold: f32,
    /// The model both vectors were embedded under.
    pub model: String,
    /// The corpus snapshot the score was computed against.
    pub corpus_watermark: String,
}

/// Why a similarity query produced no verdict. Typed, so a caller can never
/// fold "no match" into "could not evaluate" — they are different answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unevaluated {
    /// No matrix is projected: the rule is unevaluated, LOUDLY — never
    /// "empty corpus, nothing is similar, pass".
    MatrixMissing,
    /// The query vector was embedded under a different model than the matrix;
    /// scoring across models would be a number with no meaning.
    ModelMismatch {
        /// The matrix's model.
        matrix: String,
        /// The query's claimed model.
        query: String,
    },
}

impl std::fmt::Display for Unevaluated {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Unevaluated::MatrixMissing => {
                write!(f, "no vector matrix is projected into the hot plane")
            }
            Unevaluated::ModelMismatch { matrix, query } => write!(
                f,
                "query embedded under `{query}` but the matrix is `{matrix}` — \
                 a cross-model score has no meaning"
            ),
        }
    }
}

/// Brute-force cosine of `query` (embedded under `query_model`) against the
/// matrix, returning matches at or above `threshold`, best first.
///
/// `matrix` is `None` when no matrix is projected — the caller surfaces
/// [`Unevaluated::MatrixMissing`] loudly. An EMPTY projected matrix is a real
/// answer (nothing in the corpus) and returns no matches, evaluated.
pub fn nearest(
    matrix: Option<&VectorMatrix>,
    query_model: &str,
    query: &[f32],
    threshold: f32,
    limit: usize,
) -> Result<Vec<SimilarityMatch>, Unevaluated> {
    let Some(matrix) = matrix else {
        return Err(Unevaluated::MatrixMissing);
    };
    if matrix.model != query_model {
        return Err(Unevaluated::ModelMismatch {
            matrix: matrix.model.clone(),
            query: query_model.to_string(),
        });
    }
    let mut matches: Vec<SimilarityMatch> = matrix
        .vectors
        .iter()
        .filter_map(|entry| {
            let score = cosine(query, &entry.vector)?;
            (score >= threshold).then(|| SimilarityMatch {
                id: entry.id.clone(),
                label: entry.label.clone(),
                score,
                threshold,
                model: matrix.model.clone(),
                corpus_watermark: matrix.corpus_watermark.clone(),
            })
        })
        .collect();
    matches.sort_by(|a, b| b.score.total_cmp(&a.score));
    matches.truncate(limit);
    Ok(matches)
}

/// Cosine similarity, or `None` for dimension mismatch / zero vectors —
/// entries that cannot be scored are skipped, never scored as 0 (which would
/// silently rank them "maximally dissimilar", an answer nobody computed).
fn cosine(a: &[f32], b: &[f32]) -> Option<f32> {
    if a.len() != b.len() || a.is_empty() {
        return None;
    }
    let (mut dot, mut na, mut nb) = (0.0f32, 0.0f32, 0.0f32);
    for (x, y) in a.iter().zip(b) {
        dot += x * y;
        na += x * x;
        nb += y * y;
    }
    if na == 0.0 || nb == 0.0 {
        return None;
    }
    Some(dot / (na.sqrt() * nb.sqrt()))
}

#[cfg(test)]
// Test names shout the invariant they turn on (`UNEVALUATED`, `EMPTY`) — the
// repo's emphasis convention, allowed here and scoped to tests (yupana #83).
#[allow(non_snake_case)]
mod tests {
    use super::*;

    fn matrix() -> VectorMatrix {
        VectorMatrix {
            model: "test-embed-1".to_string(),
            corpus_watermark: "beads@2026-08-15T00:00:00Z".to_string(),
            vectors: vec![
                CorpusVector {
                    id: "bobbin-bnq".to_string(),
                    label: Some("frontier recompute".to_string()),
                    vector: vec![1.0, 0.0, 0.0],
                },
                CorpusVector {
                    id: "quipu-3aj".to_string(),
                    label: None,
                    vector: vec![0.0, 1.0, 0.0],
                },
            ],
        }
    }

    #[test]
    fn a_match_is_sealed_with_score_threshold_model_and_watermark() {
        let m = matrix();
        let found = nearest(Some(&m), "test-embed-1", &[0.9, 0.1, 0.0], 0.75, 5).unwrap();
        assert_eq!(found.len(), 1);
        let hit = &found[0];
        assert_eq!(hit.id, "bobbin-bnq");
        assert!(hit.score > 0.9);
        assert_eq!(hit.threshold, 0.75);
        assert_eq!(hit.model, "test-embed-1");
        assert_eq!(hit.corpus_watermark, "beads@2026-08-15T00:00:00Z");
    }

    #[test]
    fn a_missing_matrix_is_UNEVALUATED_loud_never_a_silent_pass() {
        let err = nearest(None, "test-embed-1", &[1.0], 0.75, 5).unwrap_err();
        assert_eq!(err, Unevaluated::MatrixMissing);
    }

    #[test]
    fn an_EMPTY_matrix_is_a_real_answer_distinct_from_a_missing_one() {
        let m = VectorMatrix {
            model: "test-embed-1".to_string(),
            corpus_watermark: "w".to_string(),
            vectors: Vec::new(),
        };
        let found = nearest(Some(&m), "test-embed-1", &[1.0], 0.75, 5).unwrap();
        assert!(found.is_empty(), "evaluated, nothing similar");
    }

    #[test]
    fn a_cross_model_query_is_REFUSED_not_scored() {
        let m = matrix();
        let err = nearest(Some(&m), "other-model", &[1.0, 0.0, 0.0], 0.75, 5).unwrap_err();
        assert!(matches!(err, Unevaluated::ModelMismatch { .. }), "{err}");
    }

    #[test]
    fn below_threshold_and_unscorable_entries_are_silent_not_zero() {
        let mut m = matrix();
        // A dimension-mismatched entry must be skipped, never scored.
        m.vectors.push(CorpusVector {
            id: "bad-dims".to_string(),
            label: None,
            vector: vec![1.0],
        });
        let found = nearest(Some(&m), "test-embed-1", &[0.0, 0.0, 1.0], 0.75, 5).unwrap();
        assert!(found.is_empty(), "orthogonal + unscorable ⇒ no matches");
    }

    #[test]
    fn matches_come_best_first_and_respect_the_limit() {
        let m = matrix();
        let found = nearest(Some(&m), "test-embed-1", &[0.8, 0.6, 0.0], 0.1, 1).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, "bobbin-bnq", "the nearer entry wins the limit");
    }
}
