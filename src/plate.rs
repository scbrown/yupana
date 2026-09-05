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
//!   behind by a dead session;
//! * we refuse a plate stamped by a DIFFERENT session than the one reading it
//!   (aegis-368cu.7). This is the "session id on both sides" this note used to
//!   ask for: `st anchor` stamps its own session because it runs inside the
//!   agent's, and the reader compares it against the hook payload's.
//!
//! ONE HALF OF THAT IS DELIBERATELY NOT SCOPED. A dispatcher (`st go`) writes
//! ANOTHER agent's plate and cannot know the recipient's session, so it stores
//! null, meaning "not session-scoped" — and null is still read. Rejecting it
//! would make every DISPATCHED plate unreadable, and dispatch writes most
//! plates: a staleness guard would become a total attribution outage.
//!
//! What remains open: between a work item CLOSING and the next republish, a
//! same-session plate is still confidently wrong. Session scope catches the
//! dead-session case, not the closed-item case; that window is still bounded
//! only by the publisher's cadence. Do not read the session check as closing it.

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
pub fn parse(doc: &str, now: u64, max_age: u64, session: Option<&str>) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(doc).ok()?;
    let item = v.get("item")?.as_str()?;
    if item.is_empty() {
        return None;
    }
    // SESSION SCOPE — the other half of the window this module's docs describe.
    // The publisher stamps its session only where it legitimately knows one
    // (`st anchor`, which runs inside the agent's own session); a DISPATCHER
    // writes null because it cannot know the recipient's.
    //
    // So a stored null means "not session-scoped", NOT "belongs to no session",
    // and must still be read. Rejecting it would make every DISPATCHED plate
    // unreadable — and dispatch is how most plates are written, so a staleness
    // guard would become a total attribution outage. Only a stored session that
    // DISAGREES with the reader's is a mismatch, and that is the case worth
    // catching: a plate left behind by a session that has since died.
    if let (Some(reader), Some(stored)) = (session, v.get("session").and_then(|x| x.as_str())) {
        if reader != stored {
            return None;
        }
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
pub fn current(session: Option<&str>) -> Option<String> {
    let path = path_from_env()?;
    let doc = std::fs::read_to_string(path).ok()?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let max_age = std::env::var("YUPANA_PLATE_MAX_AGE_SECS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(DEFAULT_MAX_AGE_SECS);
    parse(&doc, now, max_age, session)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_000_000;
    const MAX: u64 = 3600;

    fn doc(item: &str, at: u64) -> String {
        format!(r#"{{"item":"{item}","at":{at},"session":null}}"#)
    }

    /// A plate stamped by a session, as `st anchor` writes it.
    fn doc_session(item: &str, at: u64, session: &str) -> String {
        format!(r#"{{"item":"{item}","at":{at},"session":"{session}"}}"#)
    }

    #[test]
    fn reads_a_fresh_plate() {
        assert_eq!(
            parse(&doc("abc-1", NOW), NOW, MAX, None).as_deref(),
            Some("abc-1")
        );
    }

    #[test]
    fn stale_is_unknown_and_fresh_is_not() {
        // Both directions: a guard tested only where it fires cannot be
        // distinguished from one that always fires.
        assert_eq!(parse(&doc("abc-2", NOW - MAX - 1), NOW, MAX, None), None);
        assert_eq!(
            parse(&doc("abc-2", NOW - MAX + 1), NOW, MAX, None).as_deref(),
            Some("abc-2")
        );
    }

    #[test]
    fn boundary_is_inclusive_at_exactly_max_age() {
        assert_eq!(
            parse(&doc("abc-3", NOW - MAX), NOW, MAX, None).as_deref(),
            Some("abc-3")
        );
    }

    #[test]
    fn empty_plate_is_unknown() {
        // The tracker publishes {"item": null} for an empty plate; that is a
        // FACT meaning "no work", and it must not resolve to a work item.
        assert_eq!(parse(r#"{"item":null,"at":1000000}"#, NOW, MAX, None), None);
        assert_eq!(parse(&doc("", NOW), NOW, MAX, None), None);
    }

    #[test]
    fn malformed_is_unknown_and_never_treated_as_fresh() {
        assert_eq!(parse("{not json", NOW, MAX, None), None);
        assert_eq!(parse("[]", NOW, MAX, None), None);
        assert_eq!(parse(r#"{"item":42,"at":1000000}"#, NOW, MAX, None), None);
        // No `at` at all: abstain, NOT "assume fresh".
        assert_eq!(parse(r#"{"item":"abc-4"}"#, NOW, MAX, None), None);
        // Non-numeric `at`: same.
        assert_eq!(
            parse(r#"{"item":"abc-4","at":"soon"}"#, NOW, MAX, None),
            None
        );
    }

    #[test]
    fn a_clock_that_went_backwards_does_not_reject_everything() {
        // saturating_sub: a plate stamped in the future is not stale.
        assert_eq!(
            parse(&doc("abc-5", NOW + 500), NOW, MAX, None).as_deref(),
            Some("abc-5")
        );
    }

    // --- session scope (aegis-368cu.7) ------------------------------------
    //
    // Both-outcome per this module's contract. The load-bearing case is the
    // DISPATCHER plate (stored null) read by a session: rejecting it would make
    // every dispatched item unattributable, and dispatch writes most plates.

    #[test]
    fn session_scoped_plate_is_read_by_its_own_session() {
        let d = doc_session("abc-1", NOW, "sess-abc");
        assert_eq!(
            parse(&d, NOW, MAX, Some("sess-abc")).as_deref(),
            Some("abc-1")
        );
    }

    #[test]
    fn session_scoped_plate_abstains_for_a_different_session() {
        let d = doc_session("abc-1", NOW, "sess-abc");
        assert!(
            parse(&d, NOW, MAX, Some("sess-dead")).is_none(),
            "a plate left by another session must not attribute this session's work"
        );
    }

    #[test]
    fn dispatcher_plate_is_read_by_any_session() {
        // stored null = "not session-scoped". THE regression this change turns on.
        let d = doc("abc-2", NOW);
        assert_eq!(
            parse(&d, NOW, MAX, Some("sess-anything")).as_deref(),
            Some("abc-2")
        );
    }

    #[test]
    fn session_scoped_plate_is_read_when_the_reader_has_no_session() {
        let d = doc_session("abc-3", NOW, "sess-abc");
        assert_eq!(parse(&d, NOW, MAX, None).as_deref(), Some("abc-3"));
    }

    #[test]
    fn session_scope_does_not_bypass_the_staleness_guard() {
        // A matching session must not resurrect a stale plate: the two guards
        // are independent and either one abstaining is enough.
        let d = doc_session("abc-4", NOW - MAX - 1, "sess-abc");
        assert!(parse(&d, NOW, MAX, Some("sess-abc")).is_none());
    }
}
