//! Tests for throttle state. Size-exempt (`_test.rs`).

use super::*;

const NOW: u64 = 1_700_000_000;
const SCOPE: &str = "/repo/a";

#[test]
fn precedence_explicit_then_xdg_then_home() {
    assert_eq!(
        resolve_path(Some("/x/t.jsonl"), Some("/s"), Some("/h")).unwrap(),
        PathBuf::from("/x/t.jsonl")
    );
    assert_eq!(
        resolve_path(None, Some("/s"), Some("/h")).unwrap(),
        PathBuf::from("/s/yupana/throttles.jsonl")
    );
    assert!(resolve_path(None, None, None).is_none());
}

#[test]
fn a_recorded_throttle_is_active_until_it_expires() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("t.jsonl");
    record_to(&path, "cs_window_spend", SCOPE, 300, NOW);
    let text = std::fs::read_to_string(&path).unwrap();

    let active_now = active(&text, SCOPE, NOW);
    assert_eq!(active_now.len(), 1);
    assert_eq!(active_now[0].constraint_id, "cs_window_spend");

    assert_eq!(
        active(&text, SCOPE, NOW + 299).len(),
        1,
        "still inside the window"
    );
    assert!(
        active(&text, SCOPE, NOW + 301).is_empty(),
        "past the window it is gone"
    );
}

#[test]
fn re_crossing_extends_rather_than_duplicating() {
    // The same rule crossed twice is one advisory with a later expiry, not two
    // advisories about the same rule — an agent told the same thing twice in one
    // message learns to skim.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("t.jsonl");
    record_to(&path, "c", SCOPE, 100, NOW);
    record_to(&path, "c", SCOPE, 500, NOW);
    let text = std::fs::read_to_string(&path).unwrap();
    let active_now = active(&text, SCOPE, NOW);
    assert_eq!(active_now.len(), 1);
    assert_eq!(active_now[0].until, NOW + 500, "the later expiry wins");
}

#[test]
fn distinct_constraints_are_distinct_throttles() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("t.jsonl");
    record_to(&path, "zeta", SCOPE, 300, NOW);
    record_to(&path, "alpha", SCOPE, 300, NOW);
    let text = std::fs::read_to_string(&path).unwrap();
    let throttles = active(&text, SCOPE, NOW);
    let ids: Vec<&str> = throttles.iter().map(|t| t.constraint_id.as_str()).collect();
    // Sorted, so the same set always renders the same advisory.
    assert_eq!(ids, vec!["alpha", "zeta"]);
}

#[test]
fn the_advisory_says_plainly_that_nothing_is_blocked() {
    // A soft constraint that reads like a refusal teaches agents to treat
    // refusals as advisory, which is strictly worse than saying nothing.
    let t = Throttle {
        constraint_id: "cs_window_spend".to_string(),
        scope: SCOPE.to_string(),
        until: NOW + 120,
    };
    let text = t.advisory(NOW);
    assert!(text.contains("cs_window_spend"));
    assert!(text.contains("120s"), "it names the remaining time: {text}");
    assert!(
        text.contains("advisory") && text.contains("nothing is"),
        "it must say plainly that nothing is blocked: {text}"
    );
}

#[test]
fn the_sample_backoff_formula_is_understood() {
    // SARC's own sample: exp(min(overage / 50000, 5)).
    let f = "exp(min(overage / 50000, 5))";
    assert_eq!(backoff_secs(f, 0.0), Some(1), "e^0 = 1");
    // 50000 over => e^1 ≈ 2.718 => 2 whole seconds.
    assert_eq!(backoff_secs(f, 50_000.0), Some(2));
    // Far past the cap => e^5 ≈ 148.
    assert_eq!(backoff_secs(f, 10_000_000.0), Some(148));
}

#[test]
fn an_unrecognised_formula_yields_nothing_rather_than_a_default() {
    // A default would be a backoff nobody declared, applied under a constraint
    // whose entire point is that its cost WAS declared. The caller reports the
    // unparsed formula, so a typo is a visible gap not a wrong number.
    assert_eq!(backoff_secs("linear(overage)", 100.0), None);
    assert_eq!(backoff_secs("", 100.0), None);
    assert_eq!(backoff_secs("exp(min(overage / 0, 5))", 100.0), None);
    assert_eq!(backoff_secs("exp(min(overage / abc, 5))", 100.0), None);
}

#[test]
fn a_backoff_is_capped_however_large_the_formula_goes() {
    // A declared formula is data from the graph. An exponential one with a big
    // overage produces a number that is either meaningless or hostile.
    let huge = "exp(min(overage / 1, 100))";
    assert_eq!(backoff_secs(huge, 1000.0), Some(MAX_BACKOFF_SECS));
    // And the recorded expiry is capped too, not just the computed value.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("t.jsonl");
    record_to(&path, "c", SCOPE, u64::MAX, NOW);
    let text = std::fs::read_to_string(&path).unwrap();
    assert_eq!(active(&text, SCOPE, NOW)[0].until, NOW + MAX_BACKOFF_SECS);
}

#[test]
fn a_throttle_from_another_repo_does_not_advise_this_one() {
    // The failure this scope exists to prevent: state is one file per user, not
    // per checkout, so an unscoped throttle would make a window crossed in one
    // repo advise an agent editing an unrelated one — true about something, and
    // false about the work in front of it.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("t.jsonl");
    record_to(&path, "c", "/repo/other", 300, NOW);
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(active(&text, SCOPE, NOW).is_empty());
    assert_eq!(active(&text, "/repo/other", NOW).len(), 1);
}

#[test]
fn an_unscoped_record_matches_nothing_rather_than_everything() {
    // The permissive reading would make one legacy line advise every repo on the
    // host. Matching nothing is the conservative direction.
    let line = serde_json::json!({ "constraint_id": "c", "until": NOW + 60 }).to_string();
    assert!(active(&line, SCOPE, NOW).is_empty());
    assert!(active(&line, "", NOW).is_empty());
}

#[test]
fn a_torn_line_is_skipped_not_fatal() {
    let good =
        serde_json::json!({ "constraint_id": "c", "scope": SCOPE, "until": NOW + 60 }).to_string();
    let text = format!("{good}\n{{\"constraint_id\": \"tor\n{good}\n");
    assert_eq!(active(&text, SCOPE, NOW).len(), 1);
}

#[test]
fn an_unwritable_path_is_swallowed_whole() {
    let dir = tempfile::tempdir().unwrap();
    record_to(dir.path(), "c", SCOPE, 60, NOW);
    // Reaching here without a panic IS the assertion.
}
