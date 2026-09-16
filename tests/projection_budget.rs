//! One TOTAL projection budget per hook invocation (aegis-h9c0no).
//!
//! `http_timeout` bounds ONE call; nothing bounded their sum, and a pre-edit
//! hook makes 7-9 SERIAL queries. Measured 2026-09-16: with quipu answering just
//! past the 10s per-call ceiling, one hook ran **71.87s and orphaned all 7** —
//! it was killed by its parent mid-request, and quipu cannot cancel a read whose
//! caller is gone. Same day, same store, same code: `yupana-hook` abandoned
//! 26.4% of 424 reads while `yupana-daemon` — no parent that kills it —
//! abandoned 0.0% of 42.
//!
//! These turn on the decision itself, not on wall-clock, so they cannot flake.
// `project` is behind the `quipu` feature, so this whole file is too — otherwise every
// non-quipu CI job fails to COMPILE rather than failing a test.
#![cfg(feature = "quipu")]
// Test names SHOUT the invariant they turn on — the repo house convention
// (see src/landing_test.rs). Scoped to this test binary.
#![allow(non_snake_case)]

use std::time::Duration;

use yupana::project::budgeted;

#[test]
fn NO_budget_leaves_the_per_call_ceiling_exactly_as_it_was() {
    // CONTROL. The daemon and the CLI open no budget and must be untouched:
    // they have no parent that kills them, so bounding their total would trade
    // a correct projection for nothing.
    assert_eq!(
        budgeted(None, Duration::from_secs(10)),
        Some(Duration::from_secs(10))
    );
}

#[test]
fn a_spent_budget_says_DO_NOT_START_rather_than_start_and_be_killed() {
    // The load-bearing case. `None` is not "no timeout" — it is "do not send
    // this request", which is the only outcome that cannot leave an orphan.
    assert_eq!(
        budgeted(Some(Duration::ZERO), Duration::from_secs(10)),
        None
    );
}

#[test]
fn the_LESSER_of_the_two_wins_so_the_total_can_only_tighten() {
    let per_call = Duration::from_secs(10);
    // Little left: the remainder binds, so the last call cannot overrun the total.
    assert_eq!(
        budgeted(Some(Duration::from_secs(3)), per_call),
        Some(Duration::from_secs(3))
    );
    // Plenty left: the per-call ceiling still binds, so a budget can never
    // LENGTHEN a call — widening each of 7-9 serial budgets multiplies the worst
    // case, which is the inverse of the fix.
    assert_eq!(
        budgeted(Some(Duration::from_secs(90)), per_call),
        Some(per_call)
    );
}

#[test]
fn a_budget_equal_to_the_ceiling_still_permits_one_full_call() {
    let ten = Duration::from_secs(10);
    assert_eq!(budgeted(Some(ten), ten), Some(ten));
}
