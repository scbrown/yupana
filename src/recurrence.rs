//! Denied-edit recurrence advisory (bobbin-fjh) — the verdict spool as a
//! similarity corpus.
//!
//! Application #5 of similarity-as-grounding (quipu
//! `docs/design/semantic-grounded-edit-policies.md`): the spool already
//! retains refused-edit verdicts; embed them, and at the edit seam surface the
//! nearest prior denial as STAGE-1 advisory context — "a similar edit was
//! denied under policy X" — before the stage-2 policy evaluation that may
//! refuse. The agent learns from refusals it never saw. The ordering is
//! load-bearing twice: an agent that sees the context self-corrects before
//! tripping policy, and when a refusal does land it arrives explained.
//!
//! **Similarity never denies** — only the exact policy tier does. This module
//! produces advisory text or nothing; it has no path to an `Outcome::Deny`.
//!
//! ## The embedding, honestly
//!
//! Yupana runs no ML model. The embedding is deterministic token feature-
//! hashing (FNV-1a into a fixed 256-dim space) — a pinned, reproducible
//! lexical model, named in every advisory as [`MODEL`]. That is exactly what
//! the trust-chain rule requires: the score means nothing outside the model
//! and corpus that produced it, so both ride the advisory, with the corpus
//! watermark derived from the spool file itself. A richer model can replace
//! it later under a new `MODEL` id; scores across the two never mix
//! ([`crate::grounding::similarity`] refuses cross-model queries).
//!
//! ## Budget
//!
//! The hook is a short-lived process under a ~100 ms deadline, so the corpus
//! read is bounded: only the LAST [`TAIL_BYTES`] of the spool are scanned
//! (recent denials are the ones worth advising about), and the cosine pass is
//! brute force over at most a few hundred vectors — microseconds.

use std::io::{Read, Seek};
use std::path::Path;

use crate::grounding::similarity::{nearest, CorpusVector, VectorMatrix};

/// The pinned embedding-model identity carried in every advisory.
pub const MODEL: &str = "token-hash-fnv1a-256d-v1";

/// Advisory threshold: cosine of token-hash vectors this high means the edits
/// share most of their vocabulary — a near-repeat, not a theme.
pub const THRESHOLD: f32 = 0.6;

/// How much of the spool tail is scanned per edit.
const TAIL_BYTES: u64 = 256 * 1024;

const DIMS: usize = 256;

/// Deterministic token feature-hash embedding under [`MODEL`]. Pure; the same
/// text always embeds identically, which is what makes an advisory's score
/// falsifiable: re-embed and recompute to disprove.
#[must_use]
pub fn embed(text: &str) -> Vec<f32> {
    let mut v = vec![0.0f32; DIMS];
    for token in text
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_'))
        .filter(|t| !t.is_empty())
    {
        // FNV-1a over the lowercased token.
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        for b in token.bytes().map(|b| b.to_ascii_lowercase()) {
            hash ^= u64::from(b);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        v[(hash % DIMS as u64) as usize] += 1.0;
    }
    v
}

/// One denied edit read back from the spool tail.
struct Denial {
    policy: String,
    target: String,
    excerpt: String,
}

/// The stage-1 advisory for `introduced`, or `None` when no prior denial is
/// near enough (or no spool exists — silence is correct there: an advisory
/// plane with no corpus has nothing to say, and unlike a grounded RULE nothing
/// governed goes unevaluated by its absence).
#[must_use]
pub fn advisory(spool: &Path, introduced: &str) -> Option<String> {
    let (denials, watermark) = read_denials(spool)?;
    if denials.is_empty() {
        return None;
    }
    let matrix = VectorMatrix {
        model: MODEL.to_string(),
        corpus_watermark: watermark,
        vectors: denials
            .iter()
            .enumerate()
            .map(|(i, d)| CorpusVector {
                id: i.to_string(),
                label: None,
                vector: embed(&d.excerpt),
            })
            .collect(),
    };
    let best = nearest(Some(&matrix), MODEL, &embed(introduced), THRESHOLD, 1)
        .ok()?
        .into_iter()
        .next()?;
    let denial = &denials[best.id.parse::<usize>().ok()?];
    Some(format!(
        "yupana advisory (similarity, never a block): a similar edit was \
         previously DENIED under policy `{}` (target `{}`). \
         cosine = {:.2} ≥ threshold {:.2}, model {}, corpus {}. If this edit \
         repeats that one, expect the same refusal; if it is different, \
         proceed — only the exact policy tier can deny.",
        denial.policy, denial.target, best.score, best.threshold, best.model,
        best.corpus_watermark,
    ))
}

/// Read the denied-edit corpus from the spool's last [`TAIL_BYTES`], plus the
/// corpus watermark (file length + mtime — enough to name the snapshot a
/// score was computed against). `None` when the spool cannot be read at all.
fn read_denials(spool: &Path) -> Option<(Vec<Denial>, String)> {
    let meta = std::fs::metadata(spool).ok()?;
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs());
    let watermark = format!("spool@{}:{}", mtime, meta.len());

    let mut f = std::fs::File::open(spool).ok()?;
    let start = meta.len().saturating_sub(TAIL_BYTES);
    f.seek(std::io::SeekFrom::Start(start)).ok()?;
    let mut text = String::new();
    f.take(TAIL_BYTES).read_to_string(&mut text).ok()?;
    // A mid-file start lands mid-line; drop the torn head.
    let text = if start > 0 {
        text.split_once('\n').map_or("", |(_, rest)| rest)
    } else {
        &text
    };

    let denials = text
        .lines()
        .filter_map(|line| {
            let v: serde_json::Value = serde_json::from_str(line).ok()?;
            Some(Denial {
                // Only lines that carry a denied excerpt are corpus; satisfied
                // verdicts spool without one and are not denials to learn from.
                excerpt: v.get("denied_excerpt")?.as_str()?.to_string(),
                policy: v.get("predicate_id")?.as_str()?.to_string(),
                target: v.get("target_ref")?.as_str()?.to_string(),
            })
        })
        .collect();
    Some((denials, watermark))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spool_with(lines: &[serde_json::Value]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let body: String = lines.iter().map(|l| format!("{l}\n")).collect();
        std::fs::write(dir.path().join("verdicts.jsonl"), body).unwrap();
        dir
    }

    fn denial_line(policy: &str, excerpt: &str) -> serde_json::Value {
        serde_json::json!({
            "ts": 1, "predicate_id": policy, "target_ref": "src/a.rs",
            "turtle": "…", "denied_excerpt": excerpt,
        })
    }

    #[test]
    fn embedding_is_deterministic_and_case_insensitive_on_tokens() {
        assert_eq!(embed("Frontier Recompute"), embed("frontier recompute"));
        assert_ne!(embed("frontier recompute"), embed("entirely other words"));
    }

    #[test]
    fn a_near_repeat_of_a_denied_edit_surfaces_the_prior_denial_sealed() {
        let dir = spool_with(&[denial_line(
            "no-ticket-in-comment",
            "// internal hostname db1.gastown.local in a comment",
        )]);
        let advisory = advisory(
            &dir.path().join("verdicts.jsonl"),
            "// the internal hostname db1.gastown.local again in a comment",
        )
        .expect("a near-repeat must advise");
        assert!(advisory.contains("no-ticket-in-comment"), "{advisory}");
        assert!(advisory.contains("cosine"), "{advisory}");
        assert!(advisory.contains(MODEL), "{advisory}");
        assert!(advisory.contains("spool@"), "{advisory}");
        assert!(
            advisory.contains("never a block"),
            "the advisory must say similarity cannot deny: {advisory}"
        );
    }

    #[test]
    fn an_unrelated_edit_gets_no_advisory() {
        let dir = spool_with(&[denial_line("no-ticket-in-comment", "// hostname leak")]);
        assert_eq!(
            advisory(
                &dir.path().join("verdicts.jsonl"),
                "fn entirely_unrelated() { math(); }",
            ),
            None
        );
    }

    #[test]
    fn no_spool_and_satisfied_only_spools_are_silent() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(advisory(&dir.path().join("absent.jsonl"), "text"), None);

        // Satisfied verdicts carry no excerpt and are not a denial corpus.
        let dir = spool_with(&[serde_json::json!({
            "ts": 1, "predicate_id": "p", "target_ref": "t", "turtle": "…",
        })]);
        assert_eq!(advisory(&dir.path().join("verdicts.jsonl"), "text"), None);
    }
}
