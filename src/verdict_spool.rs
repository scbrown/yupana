//! The verdict spool — signed verdicts, buffered off the edit path.
//!
//! [`crate::verdict`] can sign a governed decision into a `VerdictShape`-
//! conformant `aegis:Verdict` and POST it to quipu. It had **no caller**: the
//! moment a constraint actually fires — the one fact an auditor most wants —
//! never became a governed record. This module is the missing half, and the
//! reason it is a spool rather than a direct call is latency.
//!
//! ## Why not just promote
//!
//! The guard runs inside `PreToolUse`, on the critical path of every edit, under
//! a `deadline_ms` that defaults to **100 ms**. A `/knot` round-trip is not that,
//! and the projection path already carries a comment recording what an
//! unbounded quipu call does here: a transiently wedged quipu held the guard for
//! the full two minutes a caller was willing to wait. Promotion on the edit path
//! would make every agent's edit latency a function of quipu's availability, to
//! record a fact that is not needed until somebody audits.
//!
//! So the guard signs (microseconds) and appends (one small write); a separate
//! drain ships them. The verdict is durable at the moment of the decision, which
//! is what matters — a verdict promoted later is still bound to the evidence
//! hash it was signed over, so nothing about the delay weakens it.
//!
//! ## Fail-silence, and one deliberate exception
//!
//! Writing a verdict must never change a guard outcome, so every error here is
//! swallowed exactly like [`crate::metrics`]. The exception is key custody:
//! `verdict::load_or_generate` MINTS a keypair when none exists, and a key
//! materialising as a side effect of an agent's edit is not something that
//! should happen quietly. On the hook path this module only signs with a key
//! that is **already there** — no key means no verdict, and the count of
//! unsigned decisions is spooled so the gap is visible rather than inferred from
//! an empty verdict file. `yupana verifier` is how an operator creates the key,
//! deliberately.

use std::path::{Path, PathBuf};

use ring::signature::Ed25519KeyPair;

use crate::trace::{ConstraintEvaluation, Outcome};
use crate::types::Freshness;

/// Ceiling before the spool rotates to `<name>.old` (one slot, replace) — the
/// same discipline and the same reason as the metrics spool: unbounded growth
/// on a full host is how a bookkeeping feature causes the outage it exists to
/// help diagnose.
const ROTATE_BYTES: u64 = 64 * 1024 * 1024;

/// Where the verdict spool lives: `$YUPANA_VERDICT_PATH`, else
/// `$XDG_STATE_HOME/yupana/verdicts.jsonl`, else
/// `~/.local/state/yupana/verdicts.jsonl`.
///
/// Pure, so the precedence is testable without touching the process
/// environment — parallel tests race on env vars, and this crate denies
/// `unsafe_code`, which `set_var` now requires.
#[must_use]
pub fn resolve_path(
    explicit: Option<&str>,
    xdg_state: Option<&str>,
    home: Option<&str>,
) -> Option<PathBuf> {
    if let Some(p) = explicit {
        return Some(PathBuf::from(p));
    }
    if let Some(x) = xdg_state {
        return Some(PathBuf::from(x).join("yupana").join("verdicts.jsonl"));
    }
    home.map(|h| {
        PathBuf::from(h)
            .join(".local")
            .join("state")
            .join("yupana")
            .join("verdicts.jsonl")
    })
}

fn spool_path() -> Option<PathBuf> {
    resolve_path(
        std::env::var("YUPANA_VERDICT_PATH").ok().as_deref(),
        std::env::var("XDG_STATE_HOME").ok().as_deref(),
        std::env::var("HOME").ok().as_deref(),
    )
}

/// The signing key, if one already exists at `path`.
///
/// Deliberately NOT `load_or_generate`: see the module docs. A missing key on
/// the hook path is a state to report, not to silently resolve by minting one.
#[must_use]
pub fn existing_key(path: &Path) -> Option<Ed25519KeyPair> {
    if !path.exists() {
        return None;
    }
    let pkcs8 = std::fs::read(path).ok()?;
    Ed25519KeyPair::from_pkcs8(&pkcs8).ok()
}

/// Sign and spool one verdict per evaluated constraint.
///
/// `target_ref` identifies what was judged (the repo-relative path);
/// `evidence` is the text the predicate actually saw, so the verdict's evidence
/// hash binds to it and a later edit staleness-checks automatically.
///
/// Returns the number of verdicts written — 0 when there is no key, nothing was
/// evaluated, or the spool is unwritable. Never errors: a verdict that cannot
/// be recorded must not turn into an edit that cannot happen.
pub fn record(
    key: &Ed25519KeyPair,
    constraints: &[ConstraintEvaluation],
    target_ref: &str,
    evidence: &str,
    freshness: Freshness,
) -> usize {
    let Some(path) = spool_path() else { return 0 };
    record_to(&path, key, constraints, target_ref, evidence, freshness)
}

/// The writable core, path injected — the seam tests use.
pub fn record_to(
    path: &Path,
    key: &Ed25519KeyPair,
    constraints: &[ConstraintEvaluation],
    target_ref: &str,
    evidence: &str,
    freshness: Freshness,
) -> usize {
    let mut written = 0;
    for evaluation in constraints {
        // `satisfied` is the verdict's outcome, and it is NOT the guard's
        // decision: a constraint can be unsatisfied while the mode declined to
        // block. The verdict records what the predicate concluded; the response
        // lives in the trace record alongside it. Conflating them would make a
        // fleet in advise mode indistinguishable from a compliant one in the
        // governed record.
        let satisfied = evaluation.outcome == Outcome::Satisfied;
        // `unknown` has no signed form in the shape's outcome enum that yupana can
        // honestly produce here — an unknown verdict asserts "there was no
        // evidence", and a constraint yupana evaluated had evidence by
        // construction. Skip rather than mint a satisfied/unsatisfied claim for
        // something that concluded neither.
        if evaluation.outcome == Outcome::Unknown {
            continue;
        }
        let turtle = crate::verdict::verdict_turtle(
            key,
            &evaluation.id,
            target_ref,
            satisfied,
            evidence,
            freshness,
        );
        if append(path, &evaluation.id, target_ref, &turtle) {
            written += 1;
        }
    }
    written
}

/// Append one spooled verdict line. Swallows every error by contract.
fn append(path: &Path, predicate_id: &str, target_ref: &str, turtle: &str) -> bool {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let line = serde_json::json!({
            "ts": std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs()),
            "predicate_id": predicate_id,
            "target_ref": target_ref,
            // The signed Turtle, verbatim. Stored whole rather than as fields to
            // re-render at drain time: the signature covers a canonical message
            // derived from these values, and a drain that rebuilt the document
            // could change a byte and invalidate every verdict in the spool.
            "turtle": turtle,
        })
        .to_string();

        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(meta) = std::fs::metadata(path) {
            if meta.len() > ROTATE_BYTES {
                let _ = std::fs::rename(path, path.with_extension("jsonl.old"));
            }
        }
        use std::io::Write;
        match std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            Ok(mut f) => writeln!(f, "{line}").is_ok(),
            Err(_) => false,
        }
    }))
    .unwrap_or(false)
}

/// One spooled verdict, as read back by the drain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spooled {
    /// The constraint this verdict attests.
    pub predicate_id: String,
    /// What it was evaluated against.
    pub target_ref: String,
    /// The signed Turtle, exactly as written.
    pub turtle: String,
}

/// Read every well-formed verdict out of a spool file.
///
/// A torn or corrupt line is SKIPPED rather than failing the read, the same rule
/// the metrics converter uses: one bad record must not dam the rest, and a
/// half-written tail line is the expected consequence of appending from a
/// short-lived process.
#[must_use]
pub fn read_spool(text: &str) -> Vec<Spooled> {
    text.lines()
        .filter_map(|line| {
            let value: serde_json::Value = serde_json::from_str(line).ok()?;
            Some(Spooled {
                predicate_id: value.get("predicate_id")?.as_str()?.to_string(),
                target_ref: value.get("target_ref")?.as_str()?.to_string(),
                turtle: value.get("turtle")?.as_str()?.to_string(),
            })
        })
        .collect()
}

/// What a drain did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Drained {
    /// Verdicts quipu accepted.
    pub promoted: usize,
    /// Verdicts quipu refused. These are RETAINED, not discarded: a rejection is
    /// a fact about the verdict (a shape violation, an unregistered verifier)
    /// that an operator has to see, and dropping it would erase the evidence of
    /// the disagreement along with the verdict.
    pub rejected: usize,
}

/// Promote every spooled verdict at `path` to quipu, then truncate.
///
/// All-or-nothing on the FILE, not on the batch: the spool is truncated only
/// when every line was accepted. A partial drain leaves the file alone and
/// reports what failed, because a truncation that dropped rejected verdicts
/// would destroy exactly the records worth investigating. Re-promoting an
/// accepted verdict is harmless — the IRI is derived from the signature, so it
/// is idempotent by content.
pub fn drain(path: &Path, endpoint: &str) -> crate::errors::Result<Drained> {
    let Ok(text) = std::fs::read_to_string(path) else {
        // No spool is not an error: it is the normal state of a fleet that has
        // not tripped a constraint.
        return Ok(Drained::default());
    };
    let spooled = read_spool(&text);
    let mut out = Drained::default();
    for verdict in &spooled {
        match crate::promote::write_knot(
            endpoint,
            &verdict.turtle,
            &format!(
                "yupana verdict: {} on {}",
                verdict.predicate_id, verdict.target_ref
            ),
        ) {
            Ok(_) => out.promoted += 1,
            Err(_) => out.rejected += 1,
        }
    }
    if out.rejected == 0 && !spooled.is_empty() {
        let _ = std::fs::remove_file(path);
    }
    Ok(out)
}

#[cfg(test)]
#[path = "verdict_spool_test.rs"]
mod tests;
