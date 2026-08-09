//! plate — read the agent's current work item, published by the work-item tracker.
//!
//! The spool answers "who did this" (`agent`) and "to what" (target), but not
//! **which piece of work caused it**. That third field is what makes a record
//! queryable as policy rather than merely auditable: a rule is a saved query
//! over records, and without the work item the records cannot express the
//! question anyone actually asks.
//!
//! yupana does not resolve the work item itself, and deliberately so. The tracker
//! owns that resolution and has more than one backend behind a single
//! interface; reimplementing it here would create a SECOND implementation that
//! can disagree with the first — a rule with two implementations, landing in
//! the component whose entire job is attribution. Shelling out to the tracker's
//! CLI is equally rejected: this code runs on the pre-edit path, ~1/edit, and a
//! subprocess against a network-backed tracker is not affordable there.
//!
//! So the tracker PUBLISHES and we READ one small file:
//!
//! ```text
//! $SHANTY_ROOT/crew/$SHANTY_AGENT/plate.json
//! {"item": "abc-123", "at": <unix secs>, "session": "<id>|null"}
//! ```
//!
//! ABSTAIN, NEVER GUESS. Missing, unreadable, malformed, empty, or stale all
//! return `None`, and `None` means UNKNOWN — not "no work". Downstream these
//! records get REPLAYED to derive enforcement rules, so a wrong work item does
//! not merely mislabel one action, it manufactures a false justification for a
//! rule that then applies to everyone. An honest unknown costs one row of
//! coverage; a confident wrong answer costs the rule.
//!
//! STALENESS, and an honest statement of its limit. A plate written while a
//! work item was open keeps answering after that item closes, so without a
//! guard every later action is attributed to the closed item — plausibly, which
//! is the dangerous kind of wrong. Two mitigations, and neither is complete on
//! its own:
//!
//! * the publisher rewrites the plate at every point it resolves one, so the
//!   file tracks reality within a tend cycle;
//! * we refuse a plate older than `max_age` as a backstop against a file left
//!   behind by a dead session.
//!
//! Between a close and the next republish there remains a window in which the
//! plate is confidently wrong. That window is bounded by the publisher's cadence
//! and is not closed here; closing it wants a session id on both sides, which
//! the harness has and the publisher does not yet carry.

use std::path::{Path, PathBuf};

/// A plate older than this is UNKNOWN. Overridable with `YUPANA_PLATE_MAX_AGE_SECS`.
///
/// Four hours is chosen to be longer than any plausible gap between tracker
/// touches within one working session, and far shorter than the lifetime of a
/// file left by a session that died. It is a backstop, not the primary guard.
const DEFAULT_MAX_AGE_SECS: u64 = 4 * 60 * 60;

/// Where the tracker publishes this agent's plate, from the environment.
///
/// Returns `None` when either env var is absent rather than inventing a
/// default path: guessing a root would make us read some *other* deployment's
/// plate, which is worse than reading none.
#[must_use]
pub fn path_from_env() -> Option<PathBuf> {
    let root = std::env::var("SHANTY_ROOT").ok()?;
    let agent = std::env::var("SHANTY_AGENT").ok()?;
    if root.is_empty() || agent.is_empty() {
        return None;
    }
    Some(Path::new(&root).join("crew").join(agent).join("plate.json"))
}

/// Parse a plate document, applying the staleness guard.
///
/// Split from the IO so the guard is testable without a filesystem — the
/// staleness rule is the part most likely to be wired inside-out, and a rule
/// that can only be tested through a temp directory tends not to be tested in
/// both directions.
#[must_use]
pub fn parse(doc: &str, now: u64, max_age: u64) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(doc).ok()?;
    let item = v.get("item")?.as_str()?;
    if item.is_empty() {
        return None;
    }
    // A missing/!numeric `at` is malformed, and malformed abstains. It must NOT
    // fall through as "fresh" — that would make a corrupt file the one input
    // that bypasses the staleness guard entirely.
    let at = v.get("at")?.as_u64()?;
    if now.saturating_sub(at) > max_age {
        return None;
    }
    Some(item.to_string())
}

/// This agent's current work item, or `None` for UNKNOWN.
///
/// Never panics, never blocks, and never returns an error: every failure mode
/// collapses to `None`, because a bookkeeping read must not be able to change
/// what the guard it annotates decides.
#[must_use]
pub fn current() -> Option<String> {
    let path = path_from_env()?;
    let doc = std::fs::read_to_string(path).ok()?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let max_age = std::env::var("YUPANA_PLATE_MAX_AGE_SECS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(DEFAULT_MAX_AGE_SECS);
    parse(&doc, now, max_age)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_000_000;
    const MAX: u64 = 3600;

    fn doc(item: &str, at: u64) -> String {
        format!(r#"{{"item":"{item}","at":{at},"session":null}}"#)
    }

    #[test]
    fn reads_a_fresh_plate() {
        assert_eq!(
            parse(&doc("abc-1", NOW), NOW, MAX).as_deref(),
            Some("abc-1")
        );
    }

    #[test]
    fn stale_is_unknown_and_fresh_is_not() {
        // Both directions: a guard tested only where it fires cannot be
        // distinguished from one that always fires.
        assert_eq!(parse(&doc("abc-2", NOW - MAX - 1), NOW, MAX), None);
        assert_eq!(
            parse(&doc("abc-2", NOW - MAX + 1), NOW, MAX).as_deref(),
            Some("abc-2")
        );
    }

    #[test]
    fn boundary_is_inclusive_at_exactly_max_age() {
        assert_eq!(
            parse(&doc("abc-3", NOW - MAX), NOW, MAX).as_deref(),
            Some("abc-3")
        );
    }

    #[test]
    fn empty_plate_is_unknown() {
        // The tracker publishes {"item": null} for an empty plate; that is a
        // FACT meaning "no work", and it must not resolve to a work item.
        assert_eq!(parse(r#"{"item":null,"at":1000000}"#, NOW, MAX), None);
        assert_eq!(parse(&doc("", NOW), NOW, MAX), None);
    }

    #[test]
    fn malformed_is_unknown_and_never_treated_as_fresh() {
        assert_eq!(parse("{not json", NOW, MAX), None);
        assert_eq!(parse("[]", NOW, MAX), None);
        assert_eq!(parse(r#"{"item":42,"at":1000000}"#, NOW, MAX), None);
        // No `at` at all: abstain, NOT "assume fresh".
        assert_eq!(parse(r#"{"item":"abc-4"}"#, NOW, MAX), None);
        // Non-numeric `at`: same.
        assert_eq!(parse(r#"{"item":"abc-4","at":"soon"}"#, NOW, MAX), None);
    }

    #[test]
    fn a_clock_that_went_backwards_does_not_reject_everything() {
        // saturating_sub: a plate stamped in the future is not stale.
        assert_eq!(
            parse(&doc("abc-5", NOW + 500), NOW, MAX).as_deref(),
            Some("abc-5")
        );
    }
}
