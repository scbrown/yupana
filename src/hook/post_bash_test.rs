//! Tests for the action-outcome hook (aegis-368cu.10, gap 3).

// Test names shout the invariant they turn on — the emphasis used throughout
// this repo and in `daemon::tests`. Scoped to tests.
#![allow(non_snake_case)]

use super::{outcome_fields, outcome_of};

/// The arms below are about fields OTHER than the work item, so they build the
/// record with an ABSTAINED item — the honest-silence shape. The work item has
/// its own two-sided arms at the bottom of this file.
fn fields_of(payload: &str) -> Vec<(&'static str, serde_json::Value)> {
    outcome_fields(payload, serde_json::Value::Null)
}

fn field<'a>(fields: &'a [(&str, serde_json::Value)], key: &str) -> Option<&'a serde_json::Value> {
    fields.iter().find(|(k, _)| *k == key).map(|(_, v)| v)
}

fn payload(event: &str, id: Option<&str>, extra: &serde_json::Value) -> String {
    let mut obj = serde_json::json!({
        "hook_event_name": event,
        "tool_name": "Bash",
        "tool_input": {"command": "echo hi"},
        "tool_response": extra.clone(),
    });
    if let Some(id) = id {
        obj["tool_use_id"] = id.into();
    }
    obj.to_string()
}

/// THE JOIN. The whole point of gap 3: an outcome record that cannot be matched
/// to its action is not evidence about that action.
#[test]
fn the_outcome_carries_the_harness_correlation_id() {
    let fields = fields_of(&payload(
        "PostToolUse",
        Some("toolu_01P22bAxbYw46r3SRRpjY8DE"),
        &serde_json::json!({}),
    ));
    assert_eq!(
        field(&fields, "action_id").and_then(serde_json::Value::as_str),
        Some("toolu_01P22bAxbYw46r3SRRpjY8DE"),
        "the id the pre-record carries must be the id the post-record carries"
    );
}

/// A payload with no id OMITS the field rather than writing an empty one. A
/// reader must be able to tell "join impossible for this row" from "the id was
/// blank", because only the second is a bug.
#[test]
fn a_missing_id_is_omitted_not_blanked() {
    let fields = fields_of(&payload("PostToolUse", None, &serde_json::json!({})));
    assert!(field(&fields, "action_id").is_none());
    assert_eq!(
        field(&fields, "outcome").and_then(serde_json::Value::as_str),
        Some("ok"),
        "the outcome is still recorded — an unjoinable row is not a useless one"
    );
}

/// The outcome is READ from the event the harness fired, not inferred.
#[test]
fn the_outcome_comes_from_the_harness_event() {
    assert_eq!(outcome_of(Some("PostToolUse")), "ok");
    assert_eq!(outcome_of(Some("PostToolUseFailure")), "error");
}

/// AN UNRECOGNISED OR ABSENT EVENT IS `unknown`, NEVER `ok`.
///
/// Defaulting to success would report every payload this hook failed to
/// understand as a working action — the shape of a guard believed to be passing
/// while it inspects nothing. The trace's only value is that it is evidence.
#[test]
fn an_unknown_event_is_never_reported_as_success() {
    assert_eq!(outcome_of(None), "unknown");
    assert_eq!(outcome_of(Some("SomethingElse")), "unknown");
    assert_eq!(outcome_of(Some("")), "unknown");
}

/// `interrupted` is neither success nor failure of the ACTION, so it is carried
/// separately rather than folded into `outcome`.
#[test]
fn interrupted_is_carried_separately_from_the_outcome() {
    let fields = fields_of(&payload(
        "PostToolUse",
        Some("toolu_x"),
        &serde_json::json!({"interrupted": true}),
    ));
    assert_eq!(
        field(&fields, "interrupted").and_then(serde_json::Value::as_bool),
        Some(true)
    );
    assert_eq!(
        field(&fields, "outcome").and_then(serde_json::Value::as_str),
        Some("ok"),
        "the harness still said PostToolUse; interruption is a separate fact"
    );
}

/// The HARNESS's duration is carried when present. A hook cannot time a call
/// that finished before it started, so this is the only honest source.
#[test]
fn the_harness_duration_is_carried_when_present() {
    let mut obj: serde_json::Value =
        serde_json::from_str(&payload("PostToolUse", Some("t"), &serde_json::json!({}))).unwrap();
    obj["duration_ms"] = 56.into();
    let fields = fields_of(&obj.to_string());
    assert_eq!(
        field(&fields, "duration_ms").and_then(serde_json::Value::as_u64),
        Some(56)
    );

    let without = fields_of(&payload("PostToolUse", Some("t"), &serde_json::json!({})));
    assert!(
        field(&without, "duration_ms").is_none(),
        "absent rather than zero — 0ms and 'not reported' are different facts"
    );
}

/// TWO-SIDED LIVENESS. `parsed` fires on every invocation, so an unparseable
/// payload is distinguishable from the hook never running at all. Those need
/// opposite fixes and neither may look like a clean run.
#[test]
fn an_unparseable_payload_still_records_that_the_hook_RAN() {
    let fields = fields_of("not json at all");
    assert_eq!(
        field(&fields, "parsed").and_then(serde_json::Value::as_bool),
        Some(false)
    );
    assert_eq!(
        field(&fields, "payload_bytes").and_then(serde_json::Value::as_u64),
        Some(15),
        "the size separates 'stdin was empty' from 'we did not understand it'"
    );
    assert!(
        field(&fields, "outcome").is_none(),
        "and it claims no outcome"
    );
}

/// Empty stdin is not the same failure as a malformed payload.
#[test]
fn empty_stdin_is_distinguishable_from_a_bad_payload() {
    let fields = fields_of("");
    assert_eq!(
        field(&fields, "payload_bytes").and_then(serde_json::Value::as_u64),
        Some(0)
    );
    assert_eq!(
        field(&fields, "parsed").and_then(serde_json::Value::as_bool),
        Some(false)
    );
}

// ---------------------------------------------------------------------------
// THE WORK ITEM (aegis-1mp1ls). Both outcomes, because a field that is only
// ever tested where it is present cannot be distinguished from one that is
// always present.
// ---------------------------------------------------------------------------

/// An ABSTAINED work item must be CLAIMED, not omitted.
///
/// This is the whole defect. `metrics::emit` fills `item` from an UNSCOPED
/// `plate::current(None)` for any caller that does not claim the field
/// (aegis-368cu.7), so a builder that abstains by pushing nothing has its
/// abstention silently overridden — and the record then asserts the work item
/// of whatever session last stamped the plate, which may be long dead. The
/// record must carry a Null the emitter can drop, not a hole the emitter fills.
#[test]
fn an_ABSTAINED_item_is_CLAIMED_so_the_unscoped_fallback_cannot_refill_it() {
    let fields = outcome_fields(
        &payload("PostToolUse", Some("t"), &serde_json::json!({})),
        serde_json::Value::Null,
    );
    let item = field(&fields, "item");
    assert!(
        item.is_some(),
        "the field must be CLAIMED even when abstaining — an unclaimed field is \
         refilled by metrics::emit's unscoped fallback: {fields:?}"
    );
    assert!(
        item.unwrap().is_null(),
        "an abstention is Null (dropped by emit), never a value: {fields:?}"
    );
}

/// The other direction: a resolved item is carried through unchanged.
#[test]
fn a_RESOLVED_item_is_carried_onto_the_outcome_record() {
    let fields = outcome_fields(
        &payload("PostToolUse", Some("t"), &serde_json::json!({})),
        "aegis-1mp1ls".into(),
    );
    assert_eq!(
        field(&fields, "item").and_then(serde_json::Value::as_str),
        Some("aegis-1mp1ls")
    );
}

/// The UNPARSEABLE branch returns early — and must still claim the field.
///
/// An early return that skips the push is the same bug in a rarer branch, and
/// it is the branch where a fabricated attribution is least defensible: we
/// could not read whose session the payload even belonged to, so the unscoped
/// fallback would attribute it to whoever last stamped the plate.
#[test]
fn an_UNPARSEABLE_payload_still_claims_the_item_rather_than_leaving_a_hole() {
    let fields = outcome_fields("{not json", serde_json::Value::Null);
    assert_eq!(
        field(&fields, "parsed"),
        Some(&serde_json::Value::Bool(false))
    );
    let item = field(&fields, "item");
    assert!(
        item.is_some() && item.unwrap().is_null(),
        "the early return must claim `item` too: {fields:?}"
    );
}
