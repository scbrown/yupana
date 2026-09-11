//! Tests for the durable exposure cache (aegis-8tumi4 item 2).
//!
//! The invariant these turn on is ASYMMETRIC, and it is the reason the module
//! exists: a failed lookup must degrade to LAST-KNOWN, never to allow, and a
//! failed lookup must never itself become the last-known thing.

// Test names shout the invariant they turn on — the same emphasis the prose
// uses throughout this repo, and load-bearing in a test name.
#![allow(non_snake_case)]

use super::*;

/// A private directory per test. Process id plus the test's own tag, so the
/// whole suite can run in parallel without tests sharing a cache.
fn tmpdir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("yupana-exposure-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).expect("create test dir");
    d
}

const EP: &str = "http://quipu.example";

#[test]
fn a_confirmed_verdict_round_trips() {
    let d = tmpdir("roundtrip");
    save(&d, EP, "shantytown", &RepoExposure::Public, 1_000);
    let got = load_servable(&d, EP, "shantytown", 3600, 1_030).expect("servable");
    assert_eq!(got.exposure, RepoExposure::Public);
    assert_eq!(got.repo, "shantytown");
    assert_eq!(got.age_secs(1_030), 30, "age is time since CONFIRMATION");
}

/// `Unknown` is a real answer — "this repo is not in the graph" — and caching it
/// is correct. What must never be cached is our FAILURE TO ASK, and that is
/// enforced at the call site by only ever saving an `Answered` lookup; this test
/// pins that the cache itself is willing to carry the verdict.
#[test]
fn an_absent_from_graph_verdict_is_a_real_answer_and_caches() {
    let d = tmpdir("unknown");
    let verdict = RepoExposure::Unknown("repo `x` is not in the graph".into());
    save(&d, EP, "x", &verdict, 500);
    let got = load_servable(&d, EP, "x", 3600, 500).expect("servable");
    assert_eq!(got.exposure, verdict);
}

#[test]
fn an_uncached_repo_is_ABSENT_and_never_invents_a_verdict() {
    let d = tmpdir("absent");
    assert_eq!(
        load_servable(&d, EP, "neverseen", 3600, 10).unwrap_err(),
        ExposureCacheMiss::Absent
    );
}

/// THE CEILING. A verdict that keeps enforcing from cache forever is worse than
/// no verdict, because it is unfalsifiable from outside the process.
#[test]
fn a_verdict_past_the_TTL_is_REFUSED_rather_than_served_stale() {
    let d = tmpdir("expired");
    save(&d, EP, "shantytown", &RepoExposure::Public, 1_000);
    assert_eq!(
        load_servable(&d, EP, "shantytown", 60, 1_100).unwrap_err(),
        ExposureCacheMiss::Expired {
            age_secs: 100,
            ttl_secs: 60
        }
    );
    // CONTROL: the same file inside the TTL still serves, so the refusal above
    // is the TTL deciding and not an unreadable cache.
    assert!(load_servable(&d, EP, "shantytown", 3600, 1_100).is_ok());
}

/// A zero TTL is the TIGHTEST CEILING, not an off switch — and the difference
/// is load-bearing because this cache reads the SAME knob as the projection
/// cache, whose contract is already pinned this way. One number meaning
/// "ceiling" for the rules and "off" for exposure would be a trap for whoever
/// tuned it.
///
/// A refusal is reported as an expiry, not as an absent cache: the file is
/// fine, policy is what refused it.
#[test]
fn a_zero_ttl_is_the_tightest_CEILING_not_an_off_switch() {
    let d = tmpdir("ttl0");
    save(&d, EP, "shantytown", &RepoExposure::Public, 1_000);
    assert_eq!(
        load_servable(&d, EP, "shantytown", 0, 1_001).unwrap_err(),
        ExposureCacheMiss::Expired {
            age_secs: 1,
            ttl_secs: 0
        },
        "an age over the ceiling must not masquerade as a missing cache"
    );
    // ...but a zero-age verdict under a zero TTL still serves, so the knob is a
    // ceiling on AGE and never a special case that discards a live lookup.
    assert!(load_servable(&d, EP, "shantytown", 0, 1_000).is_ok());
}

/// Serving another deployment's verdict would enforce its notion of "public"
/// while claiming to enforce ours.
#[test]
fn a_verdict_from_a_DIFFERENT_quipu_is_refused() {
    let d = tmpdir("endpoint");
    save(
        &d,
        "http://other.example",
        "shantytown",
        &RepoExposure::Public,
        10,
    );
    assert_eq!(
        load_servable(&d, EP, "shantytown", 3600, 10).unwrap_err(),
        ExposureCacheMiss::Endpoint("http://other.example".into())
    );
}

/// A trailing slash is the same deployment — `project` trims it when building
/// the URL, so it must not invalidate a cached verdict.
#[test]
fn a_trailing_slash_is_the_same_endpoint() {
    let d = tmpdir("slash");
    save(
        &d,
        "http://quipu.example/",
        "shantytown",
        &RepoExposure::Internal,
        10,
    );
    let got = load_servable(&d, "http://quipu.example", "shantytown", 3600, 10)
        .expect("trailing slash must not invalidate");
    assert_eq!(got.exposure, RepoExposure::Internal);
}

/// A clock that moved backwards makes the age untrustworthy, and an
/// untrustworthy age is not a young one.
#[test]
fn a_FUTURE_DATED_verdict_is_refused_rather_than_treated_as_fresh() {
    let d = tmpdir("future");
    save(&d, EP, "shantytown", &RepoExposure::Public, 9_000);
    assert_eq!(
        load_servable(&d, EP, "shantytown", 3600, 5_000).unwrap_err(),
        ExposureCacheMiss::FutureDated {
            written_at: 9_000,
            now: 5_000
        }
    );
}

#[test]
fn a_verdict_in_a_FOREIGN_FORMAT_is_refused_rather_than_parsed_leniently() {
    let d = tmpdir("version");
    let path = path_for(&d, "shantytown").expect("safe name");
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(
        &path,
        serde_json::json!({
            "version": 99u32,
            "written_at": 10u64,
            "endpoint": EP,
            "repo": "shantytown",
            "exposure": "Public",
        })
        .to_string(),
    )
    .unwrap();
    assert_eq!(
        load_servable(&d, EP, "shantytown", 3600, 10).unwrap_err(),
        ExposureCacheMiss::Version(99)
    );
}

#[test]
fn an_UNPARSEABLE_verdict_is_refused_and_reported_as_such() {
    let d = tmpdir("garbage");
    let path = path_for(&d, "shantytown").expect("safe name");
    std::fs::write(&path, b"{not json").unwrap();
    assert!(matches!(
        load_servable(&d, EP, "shantytown", 3600, 10).unwrap_err(),
        ExposureCacheMiss::Unreadable(_)
    ));
}

/// The filename is DERIVED, so a read must prove the file it found is the file
/// it wanted. Applying one repo's exposure to another is the worst thing this
/// cache could do, so the stored label is checked rather than trusted.
#[test]
fn a_verdict_ABOUT_ANOTHER_REPO_is_refused_even_when_the_filename_matches() {
    let d = tmpdir("mismatch");
    let path = path_for(&d, "shantytown").expect("safe name");
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(
        &path,
        serde_json::to_vec(&CachedExposure {
            version: EXPOSURE_CACHE_VERSION,
            written_at: 10,
            endpoint: EP.into(),
            repo: "someotherrepo".into(),
            exposure: RepoExposure::Public,
        })
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        load_servable(&d, EP, "shantytown", 3600, 10).unwrap_err(),
        ExposureCacheMiss::RepoMismatch("someotherrepo".into())
    );
}

/// A repo label comes from a remote URL, so it is externally influenced. The
/// cache must not become a write primitive, and the refusal must be NAMED
/// rather than silent — a guard with quiet holes reports the same success for a
/// repo it covers and one it has never been able to cache.
#[test]
fn a_TRAVERSING_or_otherwise_unsafe_repo_label_is_REFUSED_and_named() {
    let d = tmpdir("unsafe");
    for bad in [
        "../../etc/passwd",
        "a/b",
        "",
        ".",
        "..",
        "a b",
        "a\0b",
        "sub\\dir",
    ] {
        assert_eq!(
            path_for(&d, bad).unwrap_err(),
            ExposureCacheMiss::UnsafeName(bad.to_string()),
            "`{bad}` must not become a cache filename"
        );
    }
    // Over-long, separately: the charset is fine, the length is not.
    let long = "a".repeat(MAX_REPO_NAME_LEN + 1);
    assert_eq!(
        path_for(&d, &long).unwrap_err(),
        ExposureCacheMiss::UnsafeName(long)
    );
    // CONTROL — the real fleet's labels, and the charset's edges, must all pass.
    for good in ["shantytown", "yupana", "goldblum", "desire-path", "a.b_c-1"] {
        assert!(path_for(&d, good).is_ok(), "`{good}` must be cacheable");
    }
}

/// A save under an unsafe label must be a silent no-op, not a panic and not a
/// write somewhere else. Writes are fail-silent because bookkeeping about
/// enforcement must never be able to change an enforcement outcome.
#[test]
fn saving_an_unsafe_label_writes_NOTHING_and_does_not_panic() {
    let d = tmpdir("unsafe-save");
    save(&d, EP, "../escape", &RepoExposure::Public, 10);
    let entries: Vec<_> = std::fs::read_dir(&d)
        .expect("dir exists")
        .filter_map(Result::ok)
        .collect();
    assert!(
        entries.is_empty(),
        "an unsafe label wrote {} file(s): {:?}",
        entries.len(),
        entries
            .iter()
            .map(std::fs::DirEntry::file_name)
            .collect::<Vec<_>>()
    );
    assert!(!d.join("..").join("escape.json").exists());
}

/// Per-repo files exist so that concurrent confirmations cannot erode each
/// other. With one map file this is the test that would fail: each writer would
/// write back a map missing the other's entry.
#[test]
fn two_repos_confirmed_independently_do_NOT_erode_each_other() {
    let d = tmpdir("noerode");
    save(&d, EP, "shantytown", &RepoExposure::Public, 1_000);
    save(&d, EP, "goldblum", &RepoExposure::Internal, 1_001);
    save(&d, EP, "yupana", &RepoExposure::Public, 1_002);
    assert_eq!(
        load_servable(&d, EP, "shantytown", 3600, 1_002)
            .expect("shantytown survives")
            .exposure,
        RepoExposure::Public
    );
    assert_eq!(
        load_servable(&d, EP, "goldblum", 3600, 1_002)
            .expect("goldblum survives")
            .exposure,
        RepoExposure::Internal
    );
    assert_eq!(
        load_servable(&d, EP, "yupana", 3600, 1_002)
            .expect("yupana survives")
            .exposure,
        RepoExposure::Public
    );
}

/// A later confirmation of the same repo replaces the earlier one, and the age
/// resets — the record's age must mean "since we last CHECKED", not "since the
/// verdict first appeared".
#[test]
fn re_confirming_a_repo_refreshes_its_age() {
    let d = tmpdir("refresh");
    save(&d, EP, "shantytown", &RepoExposure::Internal, 1_000);
    save(&d, EP, "shantytown", &RepoExposure::Public, 2_000);
    let got = load_servable(&d, EP, "shantytown", 3600, 2_010).expect("servable");
    assert_eq!(got.exposure, RepoExposure::Public, "latest verdict wins");
    assert_eq!(got.age_secs(2_010), 10);
}

/// Every miss has a stable one-word label so fail-opens can be grouped by cause
/// without parsing prose. Distinctness is the property that matters: two causes
/// sharing a label is the aegis-8tumi4 defect itself — two facts wearing one
/// token.
#[test]
fn every_miss_label_is_DISTINCT_so_causes_can_be_grouped() {
    let all = [
        ExposureCacheMiss::NoStateDir,
        ExposureCacheMiss::Absent,
        ExposureCacheMiss::Unreadable("x".into()),
        ExposureCacheMiss::Version(9),
        ExposureCacheMiss::Endpoint("x".into()),
        ExposureCacheMiss::RepoMismatch("x".into()),
        ExposureCacheMiss::Expired {
            age_secs: 1,
            ttl_secs: 0,
        },
        ExposureCacheMiss::FutureDated {
            written_at: 2,
            now: 1,
        },
        ExposureCacheMiss::UnsafeName("x".into()),
    ];
    let labels: Vec<&str> = all.iter().map(miss_label).collect();
    let mut uniq = labels.clone();
    uniq.sort_unstable();
    uniq.dedup();
    assert_eq!(uniq.len(), labels.len(), "labels collide: {labels:?}");
    for m in &all {
        assert!(!m.to_string().is_empty(), "every miss must explain itself");
    }
}

#[test]
fn the_state_dir_precedence_is_explicit_env_then_xdg_then_home() {
    assert_eq!(
        resolve_dir(Some("/explicit"), Some("/xdg"), Some("/home")),
        Some(PathBuf::from("/explicit"))
    );
    assert_eq!(
        resolve_dir(None, Some("/xdg"), Some("/home")),
        Some(PathBuf::from("/xdg/yupana/exposure"))
    );
    assert_eq!(
        resolve_dir(None, None, Some("/home")),
        Some(PathBuf::from("/home/.local/state/yupana/exposure"))
    );
    assert_eq!(
        resolve_dir(None, None, None),
        None,
        "no state dir at all must be representable, not guessed"
    );
}
