//! Tests for the governed landing projection. Size-exempt (`_test.rs`).

use super::*;

fn body(rows: &str) -> String {
    format!(r#"{{"results":{{"bindings":[{rows}]}}}}"#)
}
fn v(key: &str, value: &str) -> String {
    format!(r#""{key}":{{"value":"{value}"}}"#)
}

fn one_repo() -> String {
    body(&format!(
        "{{{}}}",
        [
            v("repo", "http://aegis.gastown.local/ontology/repo_quipu"),
            v("label", "repo_quipu"),
            v("owner", "http://aegis.gastown.local/ontology/malcolm"),
            v("rule", "single-writer"),
            v("state", "RULED"),
        ]
        .join(",")
    ))
}

#[test]
fn a_declared_repo_decodes_with_its_owner_and_rule() {
    let repos = decode_landing_policies(&one_repo()).unwrap();
    assert_eq!(repos.len(), 1);
    assert_eq!(repos[0].owner.as_deref(), Some("malcolm"));
    assert_eq!(repos[0].rule, LandingRule::SingleWriter);
    assert_eq!(repos[0].ownership_state.as_deref(), Some("RULED"));
}

#[test]
fn an_undeclared_protected_ref_defaults_and_SAYS_it_defaulted() {
    let repos = decode_landing_policies(&one_repo()).unwrap();
    assert_eq!(repos[0].protected_refs, ["main"]);
    // The verdict must never present this assumption as a declared fact.
    assert!(!repos[0].protected_refs_declared);
}

#[test]
fn an_unrecognised_rule_REFUSES_rather_than_defaulting() {
    let rows = body(&format!(
        "{{{}}}",
        [
            v("repo", "aegis:repo_x"),
            v("label", "repo_x"),
            v("rule", "whatever-the-future-adds"),
        ]
        .join(",")
    ));
    let err = decode_landing_policies(&rows).unwrap_err().to_string();
    assert!(err.contains("does not understand"), "{err}");
}

#[test]
fn conflicting_owners_across_rows_REFUSE_the_projection() {
    let rows = body(&format!(
        "{{{}}},{{{}}}",
        [
            v("repo", "aegis:repo_x"),
            v("owner", "aegis:a"),
            v("rule", "single-writer"),
        ]
        .join(","),
        [
            v("repo", "aegis:repo_x"),
            v("owner", "aegis:b"),
            v("rule", "single-writer"),
        ]
        .join(",")
    ));
    let err = decode_landing_policies(&rows).unwrap_err().to_string();
    assert!(err.contains("conflicting"), "{err}");
}

#[test]
fn the_repo_PREFIX_convention_resolves_a_bare_name() {
    // The measured gap: aegis:repo_yupana carries the label `repo_yupana`
    // and NO `yupana` alias. A bare-name-only lookup reports a governed
    // repository as ungoverned, which is the silent un-guarding this test
    // exists to prevent.
    let repos = decode_landing_policies(&one_repo()).unwrap();
    assert!(matches!(
        resolve(&repos, "quipu"),
        LandingAuthority::Governed(_)
    ));
    assert!(matches!(
        resolve(&repos, "repo_quipu"),
        LandingAuthority::Governed(_)
    ));
}

#[test]
fn a_repo_absent_from_the_catalogue_is_UNGOVERNED_not_unknown() {
    let repos = decode_landing_policies(&one_repo()).unwrap();
    assert!(matches!(
        resolve(&repos, "bobbin"),
        LandingAuthority::Ungoverned { .. }
    ));
}

#[test]
fn protects_compares_short_ref_names() {
    let repos = decode_landing_policies(&one_repo()).unwrap();
    assert!(repos[0].protects("main"));
    assert!(repos[0].protects("refs/heads/main"));
    assert!(!repos[0].protects("wt/grant"));
}

/// The REAL body the live graph returns, captured 2026-09-05 immediately
/// after the policy facts were written.
///
/// It is a cross-product — 2 `rdfs:label` values x 4 `skos:altLabel` values
/// = 8 rows for ONE repository — and that shape is not something the
/// hand-built fixtures above exercise. A decoder that treated each row as a
/// repository would report eight governed repositories where there is one,
/// and every scalar would "conflict" with itself.
#[test]
fn the_LIVE_cross_product_decodes_to_exactly_one_repo() {
    let body = include_str!("../tests/fixtures/landing-policy-live.json");
    let repos = decode_landing_policies(body).expect("the live body decodes");
    assert_eq!(repos.len(), 1, "8 rows are one repository, not eight");
    let quipu = &repos[0];
    assert_eq!(quipu.owner.as_deref(), Some("malcolm"));
    assert_eq!(quipu.rule, LandingRule::SingleWriter);
    assert_eq!(quipu.ownership_state.as_deref(), Some("RULED"));
    // Captured from the LIVE graph after the override authority was written
    // to it (aegis-d7jpdw). This assertion is the proof that adding an
    // authority is a GRAPH WRITE and not a yupana build: nothing in this
    // repository names `wu`, and the only way this line passes is that the
    // projection carried the fact out of quipu.
    assert_eq!(quipu.override_authorities, ["wu"]);
    assert!(quipu.may_override("wu"));
    assert!(!quipu.may_override("grant"), "the control");
    // Repeated across all 8 rows of the cross-product, and must collapse.
    assert_eq!(quipu.override_authorities.len(), 1);
    // The repeated `protectedRef` across all 8 rows must collapse, not stack.
    assert_eq!(quipu.protected_refs, ["main"]);
    assert!(quipu.protected_refs_declared, "the graph DECLARED this ref");
    // Every alias the graph carries resolves, and so does the bare name.
    for name in ["quipu", "repo_quipu", "Quipu", "quipu-repo-github"] {
        assert!(
            matches!(resolve(&repos, name), LandingAuthority::Governed(_)),
            "`{name}` must resolve to the governed repository"
        );
    }
    assert!(matches!(
        resolve(&repos, "yupana"),
        LandingAuthority::Ungoverned { .. }
    ));
}

#[test]
fn override_authorities_ACCUMULATE_across_rows_and_strip_the_prefix() {
    // Multi-valued like `altLabel` and `protectedRef`: N authorities arrive
    // as N rows of the same cross-product and must collapse onto one repo.
    let rows = body(&format!(
        "{{{}}},{{{}}}",
        [
            v("repo", "aegis:repo_quipu"),
            v("label", "repo_quipu"),
            v("owner", "aegis:malcolm"),
            v("rule", "single-writer"),
            v(
                "overrideAuthority",
                "http://aegis.gastown.local/ontology/wu"
            ),
        ]
        .join(","),
        [
            v("repo", "aegis:repo_quipu"),
            v("label", "repo_quipu"),
            v("owner", "aegis:malcolm"),
            v("rule", "single-writer"),
            v("overrideAuthority", "aegis:sattler"),
        ]
        .join(",")
    ));
    let repos = decode_landing_policies(&rows).unwrap();
    assert_eq!(repos.len(), 1);
    assert_eq!(repos[0].override_authorities, ["wu", "sattler"]);
    assert!(repos[0].may_override("wu"));
    assert!(repos[0].may_override("sattler"));
    assert!(
        !repos[0].may_override("grant"),
        "the control: an agent the graph does not name"
    );
}

#[test]
fn an_override_authority_is_INERT_without_a_recorded_owner() {
    // `may_override` requires an owner, so the predicate cannot be used to
    // manufacture a writer for a repository whose ownership is missing.
    let rows = body(&format!(
        "{{{}}}",
        [
            v("repo", "aegis:repo_x"),
            v("label", "repo_x"),
            v("rule", "single-writer"),
            v("overrideAuthority", "aegis:wu"),
        ]
        .join(",")
    ));
    let repos = decode_landing_policies(&rows).unwrap();
    assert_eq!(repos[0].override_authorities, ["wu"]);
    assert!(repos[0].owner.is_none());
    assert!(!repos[0].may_override("wu"));
}

#[test]
fn a_repo_declaring_NO_override_authority_authorises_NOBODY() {
    // The default must be "nobody", which is the behaviour every governed
    // repository had before this field existed.
    let repos = decode_landing_policies(&one_repo()).unwrap();
    assert!(repos[0].override_authorities.is_empty());
    for agent in ["wu", "malcolm", "sattler", ""] {
        assert!(!repos[0].may_override(agent));
    }
}

#[test]
fn a_cache_written_BEFORE_this_plane_restores_as_authorising_nobody() {
    // `#[serde(default)]`: a durable projection cache predating the field
    // must load, and must load as "no authority" rather than failing or —
    // far worse — deserialising into something permissive.
    let old = r#"{"repo_iri":"aegis:repo_quipu","matched_name":"repo_quipu",
        "owner":"malcolm","rule":"single-writer","protected_refs":["main"],
        "protected_refs_declared":true,"ownership_state":"RULED"}"#;
    let restored: RepoLanding = serde_json::from_str(old).expect("an old cache still loads");
    assert!(restored.override_authorities.is_empty());
    assert!(!restored.may_override("wu"));
}

#[test]
fn a_declared_rule_with_NO_owner_cannot_be_satisfied_by_anyone() {
    let rows = body(&format!(
        "{{{}}}",
        [
            v("repo", "aegis:repo_x"),
            v("label", "repo_x"),
            v("rule", "single-writer"),
        ]
        .join(",")
    ));
    let repos = decode_landing_policies(&rows).unwrap();
    assert!(repos[0].owner.is_none());
    assert!(!repos[0].is_owner("anyone"));
    assert!(!repos[0].is_owner(""));
}
