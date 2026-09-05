//! Tests for the resident projected-policy subgraph (aegis-x894x2).
//!
//! These deliberately do NOT need a live quipu. The properties that matter are
//! about what the daemon does when quipu is UNAVAILABLE — which is the state
//! that produced every one of the 69 measured projection fail-opens — and a test
//! that needs a working quipu to prove them would be testing the happy path this
//! module was not written for.

use super::ResidentProjection;
use crate::projection_cache::{CachedProjection, CACHE_VERSION};

/// `127.0.0.1:1` is refused immediately rather than hanging, so a "quipu is
/// down" test stays fast and deterministic.
const UNREACHABLE: &str = "http://127.0.0.1:1";

fn seed(path: &std::path::Path, endpoint: &str, written_at: u64) {
    crate::projection_cache::save(
        path,
        &CachedProjection {
            version: CACHE_VERSION,
            written_at,
            endpoint: endpoint.to_string(),
            policies: Vec::new(),
            text_rules: Vec::new(),
            tripwires: Vec::new(),
            memory_policies: Vec::new(),
            landing_policies: None,
            grounded_rules: Vec::new(),
            grounding: None,
            work_item_scopes: None,
            work_item_parents: None,
            // `None`, not `Some(vec![])`: this fixture does not exercise the
            // trajectory channel, and None is what distinguishes that from a
            // catalogue that was queried and came back empty.
            trajectory_policies: None,
        },
    );
}

/// THE PROPERTY THE BEAD IS ABOUT: a reader gets an answer without contacting
/// quipu. The endpoint here is refused, so if `snapshot` touched the network at
/// all this test could not pass.
#[test]
fn a_snapshot_is_served_without_touching_quipu() {
    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join("projection.json");
    seed(&cache, UNREACHABLE, 1_000);

    let resident = ResidentProjection::new(UNREACHABLE, Some(cache), 1_030);
    let reply = resident.snapshot(1_030);

    assert!(
        reply.projection.is_some(),
        "the seeded catalogue is held resident"
    );
    assert_eq!(
        reply.age_secs,
        Some(30),
        "the age is carried so the CALLER can apply its own TTL"
    );
    assert_eq!(reply.consecutive_failures, 0);
}

/// A failed refresh must NEVER drop the last-known catalogue. This copy is what
/// every hook reads, so discarding it on a transient 502 would convert one
/// failed query into a fleet-wide loss of enforcement — strictly worse than the
/// per-hook behaviour this module replaces.
#[test]
fn a_failed_refresh_does_not_clear_the_last_known_projection() {
    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join("projection.json");
    seed(&cache, UNREACHABLE, 1_000);

    let resident = ResidentProjection::new(UNREACHABLE, Some(cache), 1_030);
    let err = resident
        .refresh_once(1_030)
        .expect_err("127.0.0.1:1 is refused");
    assert!(!err.is_empty(), "the failure names itself: {err}");

    let reply = resident.snapshot(1_040);
    assert!(
        reply.projection.is_some(),
        "the prior catalogue survives a failed refresh"
    );
    assert_eq!(reply.age_secs, Some(40), "and ages honestly while it does");
    assert_eq!(reply.consecutive_failures, 1);
    assert!(
        reply.last_error.is_some(),
        "serving a catalogue AND failing to confirm it is a state the operator \
         must be able to see — it is how a TTL expiry is predicted rather than \
         discovered"
    );
}

/// The seed ignores the TTL — a daemon restarting during a quipu outage must not
/// throw away the only copy anyone has — but every other refusal in
/// `load_servable` still applies. A cache written against a DIFFERENT deployment
/// is refused, because serving it would enforce another estate's policy while
/// claiming to enforce this one's.
#[test]
fn a_seed_ignores_the_ttl_but_still_refuses_a_foreign_endpoint() {
    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join("projection.json");
    seed(&cache, "http://someone-elses-quipu.invalid", 1_000);

    let resident = ResidentProjection::new(UNREACHABLE, Some(cache.clone()), 500_000);
    assert!(
        resident.snapshot(500_000).projection.is_none(),
        "a cache from another endpoint is not this deployment's policy"
    );

    // The same file, read as its own endpoint, IS seeded however old it is.
    let resident =
        ResidentProjection::new("http://someone-elses-quipu.invalid", Some(cache), 500_000);
    let reply = resident.snapshot(500_000);
    assert!(
        reply.projection.is_some(),
        "age alone never refuses a seed — the caller's TTL decides servability"
    );
    assert_eq!(
        reply.age_secs,
        Some(499_000),
        "and the real age is reported, so that decision can be made"
    );
}

/// Never-obtained is reported as an ABSENCE, not as a successful empty
/// catalogue. "No rules" and "could not get the rules" need opposite responses,
/// and an empty `Vec` cannot tell them apart — the silent-no-enforcement failure
/// one layer down.
#[test]
fn nothing_ever_obtained_reports_absence_not_an_empty_catalogue() {
    let dir = tempfile::tempdir().unwrap();
    let resident = ResidentProjection::new(UNREACHABLE, Some(dir.path().join("never.json")), 1_000);

    let reply = resident.snapshot(1_000);
    assert!(reply.projection.is_none());
    assert!(reply.age_secs.is_none(), "an absent projection has no age");
}

/// Dropping the handle must return PROMPTLY, not wait on the refresher.
///
/// This is the daemon-shutdown SLO in miniature: `tests/daemon.rs` holds the
/// daemon to exiting within 10s of SIGTERM, and the refresher can be inside a
/// quipu request whose timeout is longer than that. An earlier version joined
/// the thread on drop and failed exactly that test — so this asserts the
/// property that regression violated, not merely that drop works.
#[test]
fn dropping_the_handle_stops_the_refresher() {
    let dir = tempfile::tempdir().unwrap();
    let resident = ResidentProjection::new(UNREACHABLE, Some(dir.path().join("p.json")), 1_000);

    let handle = resident.spawn_refresher(std::time::Duration::from_secs(3600));
    // The first attempt runs immediately (wait starts at zero) and fails
    // against the refused endpoint; dropping must then return promptly rather
    // than blocking for the retry backoff.
    let started = std::time::Instant::now();
    drop(handle);
    assert!(
        started.elapsed() < std::time::Duration::from_secs(5),
        "drop joins the refresher promptly, it does not wait out the backoff"
    );
}
