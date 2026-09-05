//! Tests for the durable projection cache (aegis-0upyu).
//!
//! The load path is tested for BOTH outcomes of every refusal, not just the
//! happy one: this cache exists because a guard degraded silently, and a cache
//! that served the wrong catalogue — a stale one, another deployment's, a
//! half-parsed one — would degrade it silently again one layer down. Each
//! refusal below is a way that could happen.

use super::*;

use crate::rules::{MatchType, Rule};
use crate::textrules::TextTier;

fn a_policy(name: &str) -> ProjectedPolicy {
    ProjectedPolicy {
        rule: Rule {
            name: name.to_string(),
            language: "rust".to_string(),
            query: "(line_comment) @c".to_string(),
            gate: None,
            match_type: MatchType::MustNotMatch,
            pattern: "TODO".to_string(),
            applies_to: Vec::new(),
            message: None,
            class: None,
            verification_point: None,
            backoff_formula: None,
        },
        effect: "warn".to_string(),
        latency_budget_ms: None,
        hosted_at_layer: None,
    }
}

fn a_text_rule(name: &str) -> TextRule {
    TextRule {
        name: name.to_string(),
        label: None,
        pattern: "FORBIDDEN-TOKEN".to_string(),
        tier: TextTier::Block,
        class: None,
        exempt_path_regex: None,
        rationale: None,
    }
}

/// A cache written BEFORE the landing plane existed must not read as "no
/// repository is governed".
///
/// MEASURED 2026-09-05, and it is the reason the field is an `Option`. The
/// landing plane first shipped as a plain `Vec` with `#[serde(default)]`, so a
/// pre-upgrade cache deserialised to an empty catalogue — and because
/// `refresh_or_cached` serves a FRESH cache without contacting quipu at all, the
/// governed landing rule was silently inert on every host with a warm cache. The
/// guard reported ALLOW on a non-owner merge onto a governed repository, with no
/// error anywhere: exactly the shape of a guard that looks armed and is not.
#[test]
fn a_cache_predating_the_landing_plane_is_unknown_not_an_empty_catalogue() {
    let mut value = serde_json::to_value(a_projection("http://q", 10)).unwrap();
    // Drop the field, reproducing a cache written by the previous version.
    value.as_object_mut().unwrap().remove("landing_policies");
    let restored: CachedProjection = serde_json::from_value(value).unwrap();
    assert!(
        restored.landing_policies.is_none(),
        "an absent catalogue must stay absent — Some(vec![]) would claim the \
         cache had ASKED and found nothing governed"
    );
    // The other planes still restore, so refusing to speak about landings does
    // not cost the guard everything else it knows.
    assert_eq!(restored.policies.len(), 1);
    assert_eq!(restored.text_rules.len(), 1);
}

/// …and a cache that DOES carry the plane, holding no governed repository, is a
/// real answer that must survive the round trip as `Some(empty)`.
#[test]
fn a_cache_carrying_an_empty_landing_catalogue_stays_a_real_answer() {
    let mut projection = a_projection("http://q", 10);
    projection.landing_policies = Some(Vec::new());
    let restored: CachedProjection =
        serde_json::from_str(&serde_json::to_string(&projection).unwrap()).unwrap();
    assert_eq!(restored.landing_policies, Some(Vec::new()));
}

fn a_projection(endpoint: &str, written_at: u64) -> CachedProjection {
    CachedProjection {
        version: CACHE_VERSION,
        written_at,
        endpoint: endpoint.to_string(),
        policies: vec![a_policy("no-todo")],
        text_rules: vec![a_text_rule("internal-hostname")],
        tripwires: Vec::new(),
        memory_policies: Vec::new(),
        trajectory_policies: None,
        landing_policies: None,
        grounded_rules: Vec::new(),
        grounding: None,
        work_item_scopes: None,
        work_item_parents: None,
    }
}

#[test]
fn precedence_explicit_then_xdg_then_home() {
    assert_eq!(
        resolve_path(Some("/x/p.json"), Some("/s"), Some("/h")).unwrap(),
        PathBuf::from("/x/p.json")
    );
    assert_eq!(
        resolve_path(None, Some("/s"), Some("/h")).unwrap(),
        PathBuf::from("/s/yupana/projection.json")
    );
    assert_eq!(
        resolve_path(None, None, Some("/h")).unwrap(),
        PathBuf::from("/h/.local/state/yupana/projection.json")
    );
    assert!(resolve_path(None, None, None).is_none());
}

/// The whole point of the module: what was saved is what is served, BOTH
/// catalogues, across a process boundary the in-memory registry could not cross.
#[test]
fn a_saved_projection_round_trips_with_both_catalogues() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("projection.json");
    save(&p, &a_projection("http://quipu.test", 1_000));

    let loaded = load_servable(&p, "http://quipu.test", 3600, 1_010).unwrap();
    assert_eq!(loaded.policies.len(), 1);
    assert_eq!(loaded.policies[0].rule.name, "no-todo");
    assert_eq!(loaded.text_rules.len(), 1);
    assert_eq!(loaded.text_rules[0].name, "internal-hostname");
    assert_eq!(
        loaded.age_secs(1_010),
        10,
        "the age is what the record reports"
    );
}

#[test]
fn an_absent_cache_is_absent_and_not_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("nothing-here.json");
    assert_eq!(
        load_servable(&p, "http://quipu.test", 3600, 1_000).unwrap_err(),
        CacheMiss::Absent
    );
}

/// Past the TTL the cache is REFUSED, and the refusal names both numbers. A
/// guard silently enforcing week-old rules is the next version of the bug this
/// module fixes — a retired rule that keeps firing from cache is worse than no
/// rule, because it is unfalsifiable from the outside.
#[test]
fn a_cache_past_its_ttl_is_refused_with_both_numbers() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("projection.json");
    save(&p, &a_projection("http://quipu.test", 1_000));

    // One second inside the TTL: served.
    assert!(load_servable(&p, "http://quipu.test", 60, 1_060).is_ok());
    // One second past it: refused, with the age and the ceiling.
    assert_eq!(
        load_servable(&p, "http://quipu.test", 60, 1_061).unwrap_err(),
        CacheMiss::Expired {
            age_secs: 61,
            ttl_secs: 60
        }
    );
}

/// A TTL of zero is the knob's OFF position: the file is fine, policy is what
/// refused it, and the record says so rather than reporting an absent cache.
#[test]
fn a_zero_ttl_disables_serving_without_claiming_the_cache_is_missing() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("projection.json");
    save(&p, &a_projection("http://quipu.test", 1_000));
    assert_eq!(
        load_servable(&p, "http://quipu.test", 0, 1_001).unwrap_err(),
        CacheMiss::Expired {
            age_secs: 1,
            ttl_secs: 0
        }
    );
    // ...but a zero-age cache under a zero TTL is still servable, so the knob
    // is a ceiling on AGE and never a special case that discards a live sync.
    assert!(load_servable(&p, "http://quipu.test", 0, 1_000).is_ok());
}

/// Serving another deployment's catalogue would enforce policy this quipu never
/// declared, while the verdict claimed to be enforcing this one's.
#[test]
fn a_cache_from_a_different_endpoint_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("projection.json");
    save(&p, &a_projection("http://other-deployment.test", 1_000));
    assert_eq!(
        load_servable(&p, "http://quipu.test", 3600, 1_001).unwrap_err(),
        CacheMiss::Endpoint("http://other-deployment.test".to_string())
    );
}

/// A trailing slash is the same deployment — `project::query` trims it when it
/// builds the URL, so treating it as a different endpoint would throw away a
/// perfectly good cache on a config cosmetic.
#[test]
fn a_trailing_slash_is_the_same_endpoint() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("projection.json");
    save(&p, &a_projection("http://quipu.test/", 1_000));
    assert!(load_servable(&p, "http://quipu.test", 3600, 1_001).is_ok());
}

#[test]
fn a_cache_from_another_format_version_is_refused_not_parsed_leniently() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("projection.json");
    let mut old = a_projection("http://quipu.test", 1_000);
    old.version = CACHE_VERSION + 1;
    save(&p, &old);
    assert_eq!(
        load_servable(&p, "http://quipu.test", 3600, 1_001).unwrap_err(),
        CacheMiss::Version(CACHE_VERSION + 1)
    );
}

/// A cache written before the memory-policy plane existed must keep the older
/// policy planes alive when the network projection fails. This is the exact
/// field-upgrade path used by version 1 caches deployed before version 2.
#[test]
fn a_version_one_cache_without_memory_policies_is_still_servable() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("projection.json");
    let mut value = serde_json::to_value(a_projection("http://quipu.test", 1_000)).unwrap();
    value["version"] = serde_json::json!(1);
    value.as_object_mut().unwrap().remove("memory_policies");
    std::fs::write(&p, serde_json::to_vec(&value).unwrap()).unwrap();

    let loaded = load_servable(&p, "http://quipu.test", 3600, 1_001).unwrap();
    assert_eq!(loaded.version, 1);
    assert_eq!(loaded.policies.len(), 1);
    assert_eq!(loaded.text_rules.len(), 1);
    assert!(loaded.memory_policies.is_empty());
}

#[test]
fn a_corrupt_cache_is_a_named_miss_and_never_a_panic() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("projection.json");
    std::fs::write(&p, b"{ this is not json").unwrap();
    assert!(matches!(
        load_servable(&p, "http://quipu.test", 3600, 1_001),
        Err(CacheMiss::Unreadable(_))
    ));
}

/// A backwards clock makes the age — and therefore the TTL check — untrustworthy.
/// Refuse in the conservative direction: an age we cannot trust is not a young one.
#[test]
fn a_future_dated_cache_is_refused_rather_than_treated_as_young() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("projection.json");
    save(&p, &a_projection("http://quipu.test", 2_000));
    assert_eq!(
        load_servable(&p, "http://quipu.test", 3600, 1_000).unwrap_err(),
        CacheMiss::FutureDated {
            written_at: 2_000,
            now: 1_000
        }
    );
}

/// The contract that outranks the rest: a cache write is bookkeeping about
/// enforcement, not enforcement. An unwritable path must not escape.
#[test]
fn an_unwritable_path_is_swallowed_whole() {
    let dir = tempfile::tempdir().unwrap();
    // A directory cannot be replaced by a file rename target here, and the
    // parent of the temp is the dir itself — the write must simply do nothing.
    save(dir.path(), &a_projection("http://quipu.test", 1_000));
    // reaching here IS the assertion (no panic, no error, no output)
}

/// A publish must never leave the process-unique temp file behind, or ~20
/// agents writing per edit would litter the state dir indefinitely.
#[test]
fn a_successful_save_leaves_no_temp_file_behind() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("projection.json");
    save(&p, &a_projection("http://quipu.test", 1_000));
    let leftovers: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n != "projection.json")
        .collect();
    assert!(leftovers.is_empty(), "stray files: {leftovers:?}");
}

/// A second save replaces the first — the cache is one slot, and its timestamp
/// answers "when did we last CONFIRM this", not "when did the policy change".
#[test]
fn a_later_save_replaces_the_earlier_one() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("projection.json");
    save(&p, &a_projection("http://quipu.test", 1_000));
    let mut later = a_projection("http://quipu.test", 2_000);
    later.policies = vec![a_policy("no-fixme")];
    save(&p, &later);

    let loaded = load_servable(&p, "http://quipu.test", 3600, 2_001).unwrap();
    assert_eq!(loaded.written_at, 2_000);
    assert_eq!(loaded.policies[0].rule.name, "no-fixme");
}

/// Every miss carries a stable label for grouping AND prose that names the
/// numbers. The label is what a dashboard groups on; the prose is what an
/// operator reads. Neither substitutes for the other.
#[test]
fn every_miss_has_a_stable_label_and_self_explaining_prose() {
    let cases = [
        (CacheMiss::Absent, "absent"),
        (CacheMiss::Unreadable("eof".into()), "unreadable"),
        (CacheMiss::Version(9), "version"),
        (CacheMiss::Endpoint("http://other".into()), "endpoint"),
        (
            CacheMiss::Expired {
                age_secs: 90,
                ttl_secs: 60,
            },
            "expired",
        ),
        (
            CacheMiss::FutureDated {
                written_at: 2,
                now: 1,
            },
            "future-dated",
        ),
    ];
    for (miss, label) in cases {
        assert_eq!(miss_label(&miss), label);
        assert!(
            !miss.to_string().is_empty(),
            "{label} must explain itself in prose"
        );
    }
    // The two that carry numbers must actually print them — a reason that says
    // "expired" without saying how expired sends nobody to a fix.
    assert!(CacheMiss::Expired {
        age_secs: 90,
        ttl_secs: 60
    }
    .to_string()
    .contains("90"));
    assert!(CacheMiss::Expired {
        age_secs: 90,
        ttl_secs: 60
    }
    .to_string()
    .contains("60"));
}

#[test]
fn trajectory_cache_absence_is_unknown_and_empty_is_an_answer() {
    let mut value = serde_json::to_value(a_projection("http://q", 10)).unwrap();
    value.as_object_mut().unwrap().remove("trajectory_policies");
    let old: CachedProjection = serde_json::from_value(value).unwrap();
    assert!(old.trajectory_policies.is_none());
    let mut current = a_projection("http://q", 10);
    current.trajectory_policies = Some(Vec::new());
    let current: CachedProjection =
        serde_json::from_value(serde_json::to_value(current).unwrap()).unwrap();
    assert_eq!(current.trajectory_policies, Some(Vec::new()));
}
