//! The `promote_on` decision table (FR-19, GH #3), exhaustively.
//!
//! Every cell of the module doc's table is asserted here, so the documented
//! contract and the code cannot drift: a change to `decide` that alters any of
//! the nine outcomes fails a named test.

use super::{decide, Decision, Trigger};

/// Convenience: did it promote?
fn promoted(promote_on: &str, trigger: Trigger, is_merge: bool) -> bool {
    matches!(
        decide(promote_on, trigger, is_merge).unwrap(),
        Decision::Promote
    )
}

/// The shipped default. `merge` promotes on merges and declines plain commits —
/// which is §14.3's "only merges to tracked branches", the reason the key has a
/// default at all.
#[test]
fn default_merge_promotes_on_merges_only() {
    assert!(
        !promoted("merge", Trigger::Commit, false),
        "a plain commit must NOT auto-promote under promote_on = merge"
    );
    assert!(
        promoted("merge", Trigger::Commit, true),
        "a commit event on a MERGE commit is a merge — git knows even when the hook does not"
    );
    assert!(promoted("merge", Trigger::Merge, false));
    assert!(promoted("merge", Trigger::Manual, false));
}

/// `commit` is the promote-everything policy: every automated event admitted.
#[test]
fn commit_promotes_on_every_event() {
    for (trigger, is_merge) in [
        (Trigger::Manual, false),
        (Trigger::Commit, false),
        (Trigger::Commit, true),
        (Trigger::Merge, false),
    ] {
        assert!(
            promoted("commit", trigger, is_merge),
            "promote_on = commit must admit {trigger:?} (merge commit: {is_merge})"
        );
    }
}

/// THE criterion the issue names explicitly: `manual` does NOT auto-promote.
/// An explicit invocation still does — `promote_on` governs automation, not
/// authorization (see the module note).
#[test]
fn manual_never_auto_promotes() {
    for (trigger, is_merge) in [
        (Trigger::Commit, false),
        (Trigger::Commit, true),
        (Trigger::Merge, false),
        (Trigger::Merge, true),
    ] {
        let decision = decide("manual", trigger, is_merge).unwrap();
        let Decision::Declined(why) = decision else {
            panic!("promote_on = manual must decline {trigger:?} (merge commit: {is_merge})");
        };
        assert!(
            why.contains("manual"),
            "the decline must name the configured value so it can be acted on: {why}"
        );
    }
    assert!(
        promoted("manual", Trigger::Manual, false),
        "an explicit `yupana promote` is the manual case and must still promote"
    );
}

/// A decline says what it saw AND that it wrote nothing — the two facts a
/// caller reading a log needs, given the exit code is 0.
#[test]
fn a_decline_names_the_policy_and_says_it_wrote_nothing() {
    let Decision::Declined(why) = decide("merge", Trigger::Commit, false).unwrap() else {
        panic!("expected a decline");
    };
    assert!(why.contains("promote_on"), "must name the key: {why}");
    assert!(why.contains("Wrote nothing"), "must say so: {why}");
    assert!(
        why.contains("not a merge commit"),
        "must say WHY this commit did not qualify: {why}"
    );
}

/// An unrecognised value REFUSES rather than falling back. A typo that behaved
/// as the default would be indistinguishable from the key working, which is the
/// inert-control defect this whole module exists to close.
#[test]
fn an_unknown_promote_on_refuses_and_lists_the_valid_values() {
    let err = decide("merges", Trigger::Commit, false)
        .unwrap_err()
        .to_string();
    assert!(err.contains("merges"), "must quote what was set: {err}");
    for value in ["commit", "merge", "manual"] {
        assert!(err.contains(value), "must list {value}: {err}");
    }
    // Including on the manual path: a broken config must not be discoverable
    // only via an automated run.
    assert!(decide("", Trigger::Manual, false).is_err());
}

/// Surrounding whitespace in a hand-edited TOML value is not a typo.
#[test]
fn a_padded_value_is_still_the_value() {
    assert!(!promoted("  merge  ", Trigger::Commit, false));
    assert!(promoted(" commit\t", Trigger::Commit, false));
}

/// The config default and this module's vocabulary must agree — a default the
/// decision table rejects would refuse every promotion out of the box.
#[test]
fn the_config_default_is_a_value_decide_accepts() {
    let default = crate::config::YupanaConfig::default().quipu.promote_on;
    assert!(
        decide(&default, Trigger::Manual, false).is_ok(),
        "config default promote_on = {default:?} is not in decide()'s vocabulary"
    );
    assert_eq!(
        default, "merge",
        "spec §11 / FR-19 pin the default to `merge`"
    );
}
