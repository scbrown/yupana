//! Tests for the daemon-first projection path (aegis-x894x2).
//!
//! The three outcomes in the module docs are the three things worth pinning, and
//! the one that MUST NOT drift is that a down daemon falls back rather than
//! refusing. That distinction is the difference between "the guard ran the slow
//! way" and "killing one process turned enforcement off fleet-wide".

// Test names here shout the invariant they turn on — the same emphasis the
// prose uses throughout this repo, and load-bearing in a test name. Allowed
// explicitly and scoped to tests, as in `daemon::tests`.
#![allow(non_snake_case)]

use super::from_daemon;
use crate::config::YupanaConfig;
use crate::project::{ProjectionRegistry, ProjectionSource};

/// A stub daemon answering `GET /projection` once with `body`. Returns the port.
fn stub_daemon(body: String) -> u16 {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        let Ok((mut stream, _)) = listener.accept() else {
            return;
        };
        let mut chunk = [0u8; 2048];
        let _ = stream.read(&mut chunk);
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        let _ = stream.write_all(response.as_bytes());
        let _ = stream.flush();
    });
    port
}

/// A `ProjectionReply` JSON carrying an empty-but-valid catalogue from
/// `endpoint`, written `age_secs` ago.
fn reply_json(endpoint: &str, age_secs: u64, written_at: u64, failures: u64) -> String {
    serde_json::json!({
        "projection": {
            "version": crate::projection_cache::CACHE_VERSION,
            "written_at": written_at,
            "endpoint": endpoint,
            "policies": [],
            "text_rules": [],
            "tripwires": [],
            "memory_policies": [],
            "landing_policies": null,
            "grounded_rules": [],
            "grounding": null,
            "work_item_scopes": null,
            "work_item_parents": null,
        },
        "age_secs": age_secs,
        "last_error": if failures > 0 { serde_json::Value::from("quipu 502") } else { serde_json::Value::Null },
        "refreshes": 3,
        "consecutive_failures": failures,
    })
    .to_string()
}

fn config_for(port: u16, endpoint: &str, use_daemon: bool) -> YupanaConfig {
    let mut config = YupanaConfig::default();
    config.serve.use_daemon = use_daemon;
    config.serve.bind_address = "127.0.0.1".into();
    config.serve.mcp_http_port = port;
    config.quipu.enabled = true;
    config.quipu.endpoint = endpoint.to_string();
    config.quipu.projection_cache_ttl_secs = 3600;
    config
}

/// Opt-in means opt-in: with `use_daemon` false nothing is contacted at all, so
/// the default deployment is byte-for-byte the behaviour it had before.
#[test]
fn use_daemon_false_contacts_nothing() {
    let config = config_for(1, "http://quipu.test", false);
    let mut registry = ProjectionRegistry::new(&config.quipu.endpoint);
    assert!(from_daemon(&config, &mut registry, 1_000).is_none());
}

/// THE INVARIANT. A daemon that is expected and absent must fall back to the
/// live path — `None`, never `Some(Err)`. If this ever returns a refusal, then
/// stopping one process stops enforcement for every agent on the host, which is
/// the cheapest possible bypass and the thing the whole daemon client exists to
/// prevent.
#[test]
fn a_daemon_that_is_DOWN_falls_back_and_never_refuses() {
    // Port 1 is refused immediately rather than hanging.
    let config = config_for(1, "http://quipu.test", true);
    let mut registry = ProjectionRegistry::new(&config.quipu.endpoint);

    let outcome = from_daemon(&config, &mut registry, 1_000);
    assert!(
        outcome.is_none(),
        "a down daemon means FALL BACK TO LIVE, never 'stop enforcing'"
    );
}

/// A projection inside the TTL is installed and reported as a cache hit — the
/// fast path that replaces a contended live `/query`.
#[test]
fn a_fresh_daemon_projection_is_installed() {
    let endpoint = "http://quipu.test";
    let port = stub_daemon(reply_json(endpoint, 120, 880, 0));
    let config = config_for(port, endpoint, true);
    let mut registry = ProjectionRegistry::new(endpoint);

    match from_daemon(&config, &mut registry, 1_000) {
        Some(Ok(ProjectionSource::FreshCache { age_secs })) => {
            assert_eq!(age_secs, 120, "the age is carried, not merely implied");
        }
        other => panic!("expected a served projection, got {other:?}"),
    }
}

/// Past the TTL the hook refuses IMMEDIATELY instead of adding its own live
/// query to a projection path that is already failing. The reason must name the
/// refresher's failure, so the notice says why the resident copy went stale
/// rather than only that it did.
#[test]
fn a_stale_daemon_projection_refuses_without_a_live_query() {
    let endpoint = "http://quipu.test";
    // 7200s old against the 3600s TTL, with the refresher failing.
    let port = stub_daemon(reply_json(endpoint, 7_200, 1_000, 4));
    let config = config_for(port, endpoint, true);
    let mut registry = ProjectionRegistry::new(endpoint);

    match from_daemon(&config, &mut registry, 8_200) {
        Some(Err(reason)) => {
            assert!(reason.contains("7200"), "the age is named: {reason}");
            assert!(reason.contains("3600"), "and the TTL it exceeded: {reason}");
            assert!(
                reason.contains('4') && reason.contains("quipu 502"),
                "and WHY the resident copy went stale: {reason}"
            );
        }
        other => panic!("expected an immediate refusal, got {other:?}"),
    }
    assert!(
        registry.policies().is_empty(),
        "an expired projection is not quietly enforced anyway"
    );
}

/// A daemon projecting a DIFFERENT quipu is ignored. `install_cached` does not
/// check the endpoint — on the disk path `load_servable` already did — so this
/// is the only place the check exists on the daemon path. Serving it would
/// enforce another deployment's policy while claiming to enforce ours.
#[test]
fn a_daemon_projecting_a_foreign_endpoint_is_ignored() {
    let port = stub_daemon(reply_json("http://someone-elses-quipu.invalid", 10, 990, 0));
    let config = config_for(port, "http://quipu.test", true);
    let mut registry = ProjectionRegistry::new(&config.quipu.endpoint);

    assert!(
        from_daemon(&config, &mut registry, 1_000).is_none(),
        "another estate's catalogue is not ours to enforce — fall back to live"
    );
    assert!(registry.policies().is_empty());
}

/// A stub answering `POST /policy/check` with `outcome`, once. Returns its port.
fn stub_quipu(outcome: &str) -> u16 {
    use std::io::{Read, Write};
    let body = serde_json::json!({"outcome": outcome}).to_string();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        let Ok((mut stream, _)) = listener.accept() else {
            return;
        };
        let mut chunk = [0u8; 4096];
        let _ = stream.read(&mut chunk);
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        let _ = stream.write_all(response.as_bytes());
        let _ = stream.flush();
    });
    port
}

/// THE INVARIANT for exposure, and it is sharper than the projection's.
///
/// A down daemon must fall through to the LIVE `/policy/check`, not answer
/// `unknown`. Unknown DOWNGRADES block-tier rules to warnings, so folding a
/// transport failure into it would silently weaken enforcement every time one
/// process was not running — a policy change wearing the costume of a
/// connection error.
///
/// Proven by making the live path REACHABLE and the daemon dead: if the code
/// short-circuited to unknown, the stub's `public` could never come back.
#[test]
fn a_DOWN_daemon_resolves_exposure_LIVE_rather_than_answering_unknown() {
    let quipu_port = stub_quipu("satisfied");
    let mut config = config_for(1, &format!("http://127.0.0.1:{quipu_port}"), true);
    config.serve.mcp_http_port = 1; // refused immediately

    let exposure = super::exposure_for(&config, "somerepo");
    assert_eq!(
        exposure,
        crate::project::RepoExposure::Public,
        "the live path answered; a down daemon must never become `unknown`, which \
         would downgrade block-tier rules to warnings"
    );
}

/// With the daemon off, exposure resolves live and nothing is contacted on the
/// serve port — the default deployment is unchanged.
#[test]
fn use_daemon_false_resolves_exposure_live() {
    let quipu_port = stub_quipu("unsatisfied");
    let config = config_for(1, &format!("http://127.0.0.1:{quipu_port}"), false);
    assert_eq!(
        super::exposure_for(&config, "somerepo"),
        crate::project::RepoExposure::Internal
    );
}

/// The exposure budget must EXCEED the live path's own ceiling.
///
/// `/exposure` answers from memory on a hit, but on a MISS the daemon makes the
/// live `POST /policy/check` — measured at 2.4-7.2s. If the hook gave up first it
/// would print a spurious daemon-down notice and then make its OWN live call, so
/// a cold repo would cost TWO round-trips: strictly worse than having no cache
/// at all. Pinned as a relationship rather than a number, so raising
/// `http_timeout` cannot silently reintroduce it.
#[test]
fn the_exposure_budget_outlasts_the_live_call_it_may_wait_on() {
    let live = crate::project::http_timeout();
    let exposure = super::daemon_exposure_timeout();
    assert!(
        exposure > live,
        "exposure budget {exposure:?} must outlast the live call {live:?} the daemon \
         may be making on a miss, or a miss costs two /policy/check calls"
    );
    assert!(
        exposure > super::DAEMON_TIMEOUT,
        "and it is deliberately NOT the projection budget, which answers from memory"
    );
}
