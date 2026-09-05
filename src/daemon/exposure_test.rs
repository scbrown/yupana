//! Tests for the resident exposure cache (aegis-q4tt56).
//!
//! `UNREACHABLE` is `127.0.0.1:1`, refused immediately, so "quipu is down" is
//! fast and deterministic. That is also the shape the most important test here
//! needs: a failure to ask must never become a stored verdict.

// Test names shout the invariant they turn on — the same emphasis used
// throughout this repo and in `daemon::tests`. Scoped to tests.
#![allow(non_snake_case)]

use super::{ExposureCache, ExposureReply};
use crate::project_exposure::RepoExposure;

const UNREACHABLE: &str = "http://127.0.0.1:1";

/// THE SAFETY PROPERTY. A verdict we never received must not be cached.
///
/// Caching it would freeze a timeout or a 502 for the whole TTL — converting a
/// transient blip into hours of degraded enforcement, during exactly the quipu
/// trouble that produced it. The call still ANSWERS `unknown` to its caller, and
/// a governed rule never blocks on that; what must not happen is the answer
/// being remembered.
#[test]
fn an_unreachable_quipu_is_answered_but_NEVER_cached() {
    let cache = ExposureCache::default();

    let first = cache.resolve(UNREACHABLE, "somerepo", 0xabc, 3600, 1_000);
    assert_eq!(first.verdict, "unknown", "the caller still gets a decision");
    assert!(!first.from_cache);
    assert!(
        first.reason.is_some(),
        "and it explains itself rather than saying only `unknown`"
    );

    assert!(
        cache.is_empty(),
        "a failure to ASK must never be stored as a verdict — this is the whole \
         reason ExposureAnswer distinguishes Answered from Unreachable"
    );

    // A second call must go live again rather than serve the remembered failure.
    let second = cache.resolve(UNREACHABLE, "somerepo", 0xabc, 3600, 1_010);
    assert!(!second.from_cache, "still not served from cache");
    assert_eq!(second.age_secs, 0);
}

/// A change to the RULE SET invalidates a verdict even inside the TTL. This is
/// invalidation by meaning rather than by clock: a policy change that could
/// alter an exposure verdict must not be outlived by a cached one.
#[test]
fn a_different_rule_set_hash_misses_even_when_fresh() {
    let cache = ExposureCache::default();
    // Seed a verdict directly; resolving live is not what this test is about.
    cache.insert_for_test("repo-a", RepoExposure::Public, 1_000, 0xaaa);

    let same = cache.hit_for_test("repo-a", 0xaaa, 3600, 1_030);
    assert!(same.is_some(), "same rules, inside TTL -> hit");
    assert_eq!(same.unwrap().age_secs, 30, "and the age is carried");

    let changed = cache.hit_for_test("repo-a", 0xbbb, 3600, 1_030);
    assert!(
        changed.is_none(),
        "the rules moved under it; the verdict must not be reused"
    );
}

/// The TTL still bounds a verdict whose rule set has not changed.
#[test]
fn a_verdict_past_the_ttl_misses() {
    let cache = ExposureCache::default();
    cache.insert_for_test("repo-a", RepoExposure::Internal, 1_000, 0xaaa);

    assert!(cache.hit_for_test("repo-a", 0xaaa, 60, 1_030).is_some());
    assert!(
        cache.hit_for_test("repo-a", 0xaaa, 60, 1_100).is_none(),
        "100s old against a 60s TTL"
    );
}

/// "quipu says it does not know this repo" IS an answer and is cacheable — it is
/// stable, and re-asking every edit is what made this the constant half of the
/// store's load. It is kept distinct from a failure to ask (first test above).
#[test]
fn a_graph_answered_unknown_is_cacheable() {
    let cache = ExposureCache::default();
    cache.insert_for_test(
        "ghost",
        RepoExposure::Unknown("repo `ghost` is not in the graph".into()),
        1_000,
        0xaaa,
    );
    let hit = cache.hit_for_test("ghost", 0xaaa, 3600, 1_005);
    let hit = hit.expect("an answered unknown is servable");
    assert_eq!(hit.verdict, "unknown");
    assert!(hit.from_cache);
    assert!(
        hit.reason.is_some_and(|r| r.contains("not in the graph")),
        "and it still carries WHY, so the guard's notice can explain itself"
    );
}

/// The decision value survives the wire round-trip. A reply whose `exposure()`
/// did not reconstruct faithfully would let the daemon and the live path
/// disagree about what "public" means, which is the one thing
/// `fetch_repo_exposure`'s own docs say must never happen.
#[test]
fn the_reply_reconstructs_the_decision_value() {
    for (verdict, expect_public, expect_internal) in [
        ("public", true, false),
        ("internal", false, true),
        ("unknown", false, false),
    ] {
        let reply = ExposureReply {
            verdict: verdict.to_string(),
            reason: Some("because".into()),
            from_cache: true,
            age_secs: 5,
            rules_hash: "abc".into(),
        };
        let exposure = reply.exposure();
        assert_eq!(exposure == RepoExposure::Public, expect_public);
        assert_eq!(exposure == RepoExposure::Internal, expect_internal);
    }
}

/// `clear` drops held verdicts — housekeeping when the rule set moves, so the
/// map does not grow one dead entry per repo per policy change.
#[test]
fn clear_drops_held_verdicts() {
    let cache = ExposureCache::default();
    cache.insert_for_test("repo-a", RepoExposure::Public, 1_000, 0xaaa);
    assert_eq!(cache.len(), 1);
    cache.clear();
    assert!(cache.is_empty());
}
