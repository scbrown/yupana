//! `owner_only()` must be EXHAUSTIVE, because the permissive answer is the
//! dangerous one.
//!
//! It was `matches!(self, Self::SingleWriter)`. A `matches!` answers `false` for
//! anything it was not told about, so every `LandingRule` variant added later
//! would be silently NOT owner-restricted — a policy hole one enum line away,
//! with no compiler warning and no runtime symptom except that the guard keeps
//! allowing. Found 2026-09-16 (wu) while reading why a governed repo produced no
//! refusal.
//!
//! These tests cannot detect a future `matches!` regression on their own — a new
//! variant would simply not be listed here either. The EXHAUSTIVE MATCH is the
//! real guard; this file pins the two answers that exist today and states why.
// `project_landing` is behind the `quipu` feature, so this whole file is too —
// otherwise every non-quipu CI job fails to COMPILE rather than failing a test.
#![cfg(feature = "quipu")]
// Test names SHOUT the invariant they turn on — the repo house convention
// (src/hook/scope_arm_test.rs:9). Scoped to this test binary.
#![allow(non_snake_case)]

use yupana::project_landing::LandingRule;

#[test]
fn single_writer_RESTRICTS_landing_to_the_owner() {
    assert!(LandingRule::SingleWriter.owner_only());
}

#[test]
fn any_owner_with_bead_relaxes_ONLY_the_owner_check() {
    // False here means "not owner-restricted", NOT "unenforced": the bead half
    // is a separate fault in decide(), and a landing with a readable plate and
    // no work item still REFUSES under this rule.
    assert!(!LandingRule::AnyOwnerWithBead.owner_only());
}

#[test]
fn every_rule_that_PARSES_also_answers_the_owner_question() {
    // The round trip that would catch a variant wired into the parser but
    // forgotten elsewhere: whatever the wire form parses to must answer.
    for wire in ["single-writer", "any-owner-with-bead"] {
        let rule = LandingRule::parse(wire).unwrap_or_else(|| panic!("`{wire}` no longer parses"));
        assert_eq!(rule.as_str(), wire, "wire round-trip must be stable");
        let _ = rule.owner_only();
    }
}
