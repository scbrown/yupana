//! Tests for the capability-scope ladder (`scope_arm`) — which rung answers,
//! in what order, and what happens when none does.
//!
//! Child module of `scope_arm` (`super::*` reaches its private
//! `ladder_fallback` and `ScopePlane`); size-exempt by the `_test.rs` naming,
//! the same split `pre_edit`/`pre_edit_test` already uses.

// Test names shout the invariant they turn on, the repo's emphasis convention.
#![allow(non_snake_case)]

use super::*;
use crate::policy::WorkItemScopes;

fn config(mode: Mode, rung: Mode) -> YupanaConfig {
    let mut config = YupanaConfig::default();
    config.policy.mode = mode;
    config.policy.work_item_scope = rung;
    config
}

/// A session id unique to this call. See the note on the fall-through test:
/// the once-per-session marker persists in the temp dir across runs, so a
/// fixed id makes the second run of a notice assertion vacuous.
fn unique_session() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    format!(
        "scope-arm-{}-{nanos}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

fn input(session: &str) -> HookInput {
    HookInput::parse(
        &serde_json::json!({
            "session_id": session,
            "tool_name": "Edit",
            "tool_input": { "file_path": "/r/src/a.rs" },
        })
        .to_string(),
    )
    .unwrap()
}

fn plane(rows: &[(&str, &str)]) -> ScopePlane {
    ScopePlane {
        scopes: WorkItemScopes::from_rows(
            rows.iter()
                .map(|(a, b)| ((*a).to_string(), (*b).to_string())),
        ),
        parents: None,
        cache_age: None,
    }
}

/// A plane with a parent map, for the derived rung.
fn plane_with_parents(rows: &[(&str, &str)], parents: &[(&str, &str)]) -> ScopePlane {
    ScopePlane {
        parents: Some(crate::policy::WorkItemParents::from_rows(
            parents
                .iter()
                .map(|(a, b)| ((*a).to_string(), (*b).to_string())),
        )),
        ..plane(rows)
    }
}

#[test]
fn the_rung_is_OFF_by_default_preserving_the_pre_ladder_contract() {
    let d = ladder_fallback(
        &YupanaConfig::default(),
        &input("s-default-off"),
        Some("polecat"),
        "src/other.rs",
        Some("aegis-1"),
        Some(&plane(&[("aegis-1", "src/a.rs")])),
    );
    assert_eq!(d.outcome, Outcome::Allow);
}

#[test]
fn observed_scope_allows_a_touched_path_silently() {
    let d = ladder_fallback(
        &config(Mode::Enforce, Mode::Advise),
        &input("s-obs-in"),
        Some("polecat"),
        "src/a.rs",
        Some("aegis-1"),
        Some(&plane(&[("aegis-1", "src/a.rs")])),
    );
    assert_eq!(d.outcome, Outcome::Allow);
}

#[test]
fn observed_scope_ADVISES_out_of_scope_at_the_advise_rung() {
    let d = ladder_fallback(
        &config(Mode::Enforce, Mode::Advise),
        &input("s-obs-out"),
        Some("polecat"),
        "src/other.rs",
        Some("aegis-1"),
        Some(&plane(&[("aegis-1", "src/a.rs")])),
    );
    let Outcome::Notify(message) = &d.outcome else {
        panic!(
            "observed scope must notify by default, not deny: {:?}",
            d.outcome
        );
    };
    assert!(message.contains("OBSERVED"), "names its rung: {message}");
    assert!(message.contains("aegis-1"), "names the item: {message}");
    assert_eq!(d.constraints[0].id, "scope-observed:allow_paths");
}

#[test]
fn work_item_scope_enforce_DENIES_out_of_scope() {
    let d = ladder_fallback(
        &config(Mode::Enforce, Mode::Enforce),
        &input("s-obs-deny"),
        Some("polecat"),
        "src/other.rs",
        Some("aegis-1"),
        Some(&plane(&[("aegis-1", "src/a.rs")])),
    );
    let Outcome::Deny(message) = &d.outcome else {
        panic!("work_item_scope=enforce must deny: {:?}", d.outcome);
    };
    // The constraint still TEACHES: the deny names the item, its paths,
    // and both legitimate ways forward.
    assert!(message.contains("aegis-1"), "names the item: {message}");
    assert!(
        message.contains("update your tracked item"),
        "guides: {message}"
    );
}

#[test]
fn ambient_advise_stays_a_ceiling_over_enforce() {
    let d = ladder_fallback(
        &config(Mode::Advise, Mode::Enforce),
        &input("s-obs-ceiling"),
        Some("polecat"),
        "src/other.rs",
        Some("aegis-1"),
        Some(&plane(&[("aegis-1", "src/a.rs")])),
    );
    assert!(
        matches!(d.outcome, Outcome::Notify(_)),
        "advise-mode deployments never deny: {:?}",
        d.outcome
    );
}

#[test]
fn unknown_scope_PRODUCES_an_advisory_for_the_delivery_gate() {
    let session = format!("scope-arm-test-{}", std::process::id());
    let d = ladder_fallback(
        &config(Mode::Enforce, Mode::Advise),
        &input(&session),
        Some("polecat"),
        "src/a.rs",
        None,
        None,
    );
    let Outcome::Notify(message) = &d.outcome else {
        panic!(
            "unknown scope must notify, not allow silently: {:?}",
            d.outcome
        );
    };
    assert!(message.contains("UNGUARDED"), "says unguarded: {message}");
    // The producer remains deterministic. The shared delivery boundary owns
    // once-per-session suppression so a changed cause can speak again.
    let d2 = ladder_fallback(
        &config(Mode::Enforce, Mode::Advise),
        &input(&session),
        Some("polecat"),
        "src/a.rs",
        None,
        None,
    );
    assert_eq!(d2.outcome, d.outcome);
}

#[test]
fn an_unidentified_caller_stays_silent() {
    let d = ladder_fallback(
        &config(Mode::Enforce, Mode::Advise),
        &input("s-anon"),
        None,
        "src/a.rs",
        None,
        None,
    );
    assert_eq!(d.outcome, Outcome::Allow);
}

#[test]
fn mode_off_is_inert() {
    let d = ladder_fallback(
        &config(Mode::Off, Mode::Enforce),
        &input("s-off"),
        Some("polecat"),
        "src/a.rs",
        Some("aegis-1"),
        Some(&plane(&[("aegis-1", "src/a.rs")])),
    );
    assert_eq!(d.outcome, Outcome::Allow);
}

// --- the DERIVED rung ---------------------------------------------------

/// RED. An item with no observed ground of its own inherits its parent's,
/// and an edit outside that draws an advisory naming BOTH ids — the reader
/// needs to know whose ground it just left.
#[test]
fn an_item_with_no_ground_of_its_own_inherits_its_parents() {
    let mut config = YupanaConfig::default();
    config.policy.mode = Mode::Advise;
    config.policy.work_item_scope = Mode::Advise;
    let d = ladder_fallback(
        &config,
        &input("s-derived"),
        Some("polecat"),
        "src/elsewhere.rs",
        Some("aegis-child"),
        Some(&plane_with_parents(
            &[("aegis-epic", "src/a.rs")],
            &[("aegis-child", "aegis-epic")],
        )),
    );
    let Outcome::Notify(msg) = &d.outcome else {
        panic!("expected an advisory, got {:?}", d.outcome);
    };
    assert!(msg.contains("DERIVED"), "{msg}");
    assert!(msg.contains("aegis-child"), "{msg}");
    assert!(msg.contains("aegis-epic"), "{msg}");
}

/// GREEN, and the control. An edit INSIDE the parent's ground is silent —
/// without this the test above would pass against a rung that advised on
/// everything, which is the failure mode that trains agents to ignore it.
#[test]
fn an_edit_inside_the_parents_ground_is_silent() {
    let mut config = YupanaConfig::default();
    config.policy.mode = Mode::Advise;
    config.policy.work_item_scope = Mode::Advise;
    let d = ladder_fallback(
        &config,
        &input("s-derived-in"),
        Some("polecat"),
        "src/a.rs",
        Some("aegis-child"),
        Some(&plane_with_parents(
            &[("aegis-epic", "src/a.rs")],
            &[("aegis-child", "aegis-epic")],
        )),
    );
    assert!(matches!(d.outcome, Outcome::Allow), "{:?}", d.outcome);
}

/// AN INFERENCE MUST NOT OVERRIDE A RECORD. When the item has its own
/// observed ground the derived rung must not fire at all — the message
/// proves which rung answered.
#[test]
fn an_OBSERVED_ground_wins_over_the_derived_one() {
    let mut config = YupanaConfig::default();
    config.policy.mode = Mode::Advise;
    config.policy.work_item_scope = Mode::Advise;
    let d = ladder_fallback(
        &config,
        &input("s-observed-wins"),
        Some("polecat"),
        "src/elsewhere.rs",
        Some("aegis-child"),
        Some(&plane_with_parents(
            &[("aegis-child", "src/own.rs"), ("aegis-epic", "src/a.rs")],
            &[("aegis-child", "aegis-epic")],
        )),
    );
    let Outcome::Notify(msg) = &d.outcome else {
        panic!("expected an advisory, got {:?}", d.outcome);
    };
    assert!(
        msg.contains("OBSERVED"),
        "the item's own record must answer: {msg}"
    );
    assert!(!msg.contains("DERIVED"), "{msg}");
}

/// THE RUNG NEVER HARD-DENIES, even at enforce. `ScopeProvenance` states
/// the rule: a declared scope may hard-deny; derived advises. Denying on a
/// sibling's history would strand an agent on somebody else's record, and a
/// guard that strands an operator is worse than what it prevents.
#[test]
fn the_derived_rung_ADVISES_even_under_enforce() {
    let mut config = YupanaConfig::default();
    config.policy.mode = Mode::Enforce;
    config.policy.work_item_scope = Mode::Enforce;
    let d = ladder_fallback(
        &config,
        &input("s-derived-enforce"),
        Some("polecat"),
        "src/elsewhere.rs",
        Some("aegis-child"),
        Some(&plane_with_parents(
            &[("aegis-epic", "src/a.rs")],
            &[("aegis-child", "aegis-epic")],
        )),
    );
    assert!(
        matches!(d.outcome, Outcome::Notify(_)),
        "derived must never deny, got {:?}",
        d.outcome
    );
}

/// An inference that cannot be drawn is UNKNOWN, not a scope. No parent
/// map, no parent recorded, or a parent with no ground of its own must all
/// fall through to the unknown-scope notice rather than inventing one.
///
/// The session ids are per-call and nanosecond-stamped, not fixed strings:
/// the unknown-scope notice fires once per `(session, kind)` and the marker
/// lives in the temp dir, so a fixed id is consumed by the FIRST run and
/// every later run silently gets `Allow` — a test that passes once and then
/// asserts nothing. Same reason `pre_edit_test::unique_session` exists.
#[test]
fn a_derivation_that_cannot_be_drawn_falls_through_to_UNKNOWN() {
    let mut config = YupanaConfig::default();
    config.policy.mode = Mode::Advise;
    config.policy.work_item_scope = Mode::Advise;
    for (why, plane) in [
        ("no parent map", plane(&[("aegis-epic", "src/a.rs")])),
        (
            "no parent for this item",
            plane_with_parents(&[("aegis-epic", "src/a.rs")], &[("other", "aegis-epic")]),
        ),
        (
            "parent has no ground either",
            plane_with_parents(&[], &[("aegis-child", "aegis-epic")]),
        ),
    ] {
        let d = ladder_fallback(
            &config,
            &input(&unique_session()),
            Some("polecat"),
            "src/elsewhere.rs",
            Some("aegis-child"),
            Some(&plane),
        );
        let msg = match &d.outcome {
            Outcome::Notify(m) => m.clone(),
            other => panic!("{why}: expected the unknown notice, got {other:?}"),
        };
        assert!(
            msg.contains("UNGUARDED by scope"),
            "{why}: expected unknown-scope, got: {msg}"
        );
    }
}
