//! Tests for `policy` — scope matching, blast-radius ceilings, and violation
//! reporting. Child module of `policy` (`super::*` reaches its private
//! helpers); size-exempt (`_test.rs`).

use super::*;

fn scope() -> Scope {
    Scope {
        allow_paths: vec!["src/**".to_string(), "tests/**".to_string()],
        deny_paths: vec!["src/config.rs".to_string()],
        max_impacted_symbols: Some(5),
        max_impacted_files: Some(2),
    }
}

#[test]
fn allows_a_path_inside_scope() {
    assert!(scope().check_path("src/graph/blast.rs", "t").is_none());
    // A direct child of the allowed prefix, not just a nested one.
    assert!(scope().check_path("src/policy.rs", "t").is_none());
}

#[test]
fn denies_a_path_outside_scope() {
    let violation = scope()
        .check_path("docs/yupana-spec.md", "polecat-3")
        .unwrap();
    assert_eq!(violation.kind, ViolationKind::PathOutOfScope);
    // The reason names the path, the tenant, and what is allowed.
    assert!(violation.message.contains("docs/yupana-spec.md"));
    assert!(violation.message.contains("polecat-3"));
    assert!(violation.message.contains("src/**"));
}

#[test]
fn deny_paths_beat_allow_paths() {
    let violation = scope().check_path("src/config.rs", "t").unwrap();
    assert_eq!(violation.kind, ViolationKind::PathOutOfScope);
    assert!(violation.message.contains("deny_paths"));
}

#[test]
fn empty_allow_paths_permits_anything() {
    let open = Scope {
        deny_paths: vec!["secrets/**".to_string()],
        ..Scope::default()
    };
    assert!(open.check_path("anywhere/at/all.rs", "t").is_none());
    assert!(open.check_path("secrets/key.rs", "t").is_some());
}

#[test]
fn blast_radius_within_ceilings_is_allowed() {
    let radius = BlastRadius {
        symbols: 5,
        files: 2,
    };
    assert!(scope().check_blast(radius, "src/a.rs", "t").is_none());
}

#[test]
fn blast_radius_over_ceiling_is_denied_with_numbers() {
    let radius = BlastRadius {
        symbols: 47,
        files: 12,
    };
    let violation = scope()
        .check_blast(radius, "src/a.rs", "polecat-3")
        .unwrap();
    assert_eq!(violation.kind, ViolationKind::BlastRadiusExceeded);
    // The model needs the actual and the ceiling to act on the refusal.
    assert!(violation.message.contains("47 symbols (ceiling 5)"));
    assert!(violation.message.contains("12 files (ceiling 2)"));
    assert!(violation.message.contains("polecat-3"));
}

#[test]
fn absent_ceilings_never_trip() {
    let unbounded = Scope::default();
    let radius = BlastRadius {
        symbols: 9999,
        files: 9999,
    };
    assert!(unbounded.check_blast(radius, "src/a.rs", "t").is_none());
}

#[test]
fn mode_off_yields_no_scope_even_when_one_is_configured() {
    let mut config = PolicyConfig {
        mode: Mode::Off,
        ..PolicyConfig::default()
    };
    config.scopes.insert("t".to_string(), scope());
    assert!(config.scope_for(Some("t")).is_none());
    config.mode = Mode::Enforce;
    assert!(config.scope_for(Some("t")).is_some());
    // An unknown or absent tenant is unconstrained.
    assert!(config.scope_for(Some("other")).is_none());
    assert!(config.scope_for(None).is_none());
}

#[test]
fn malformed_globs_are_reported_and_never_match() {
    let broken = Scope {
        allow_paths: vec!["src/[".to_string()],
        ..Scope::default()
    };
    assert_eq!(broken.glob_errors().len(), 1);
    // Non-empty allow_paths that cannot match => everything is out of scope,
    // rather than the pattern silently widening it.
    assert!(broken.check_path("src/a.rs", "t").is_some());
}

#[test]
fn mode_parses_from_toml_and_rejects_typos() {
    #[derive(Deserialize)]
    struct Wrapper {
        mode: Mode,
    }
    let ok: Wrapper = toml::from_str("mode = \"enforce\"").unwrap();
    assert_eq!(ok.mode, Mode::Enforce);
    // A typo must be a loud error, not a silently inert guard.
    assert!(toml::from_str::<Wrapper>("mode = \"enfroce\"").is_err());
}

fn config_with_scope(mode: Mode) -> PolicyConfig {
    let mut config = PolicyConfig {
        mode,
        ..PolicyConfig::default()
    };
    config.scopes.insert("weaver".to_string(), scope());
    config
}

#[test]
fn status_reports_a_configured_scope_with_its_ceilings() {
    let status = config_with_scope(Mode::Enforce).status_for(Some("weaver"));
    assert_eq!(status.mode, Mode::Enforce);
    assert!(status.scope_configured);
    assert!(!status.enforcing_without_scope);
    let summary = status.scope.unwrap();
    // scope() allows src/** and tests/**, denies src/config.rs, files ≤ 2.
    assert_eq!(summary.allow_paths, 2);
    assert_eq!(summary.deny_paths, 1);
    assert_eq!(summary.max_impacted_files, Some(2));
}

#[test]
fn status_flags_enforce_with_no_scope_for_the_tenant() {
    // A scope exists, but not for this tenant: armed-looking, inert.
    let status = config_with_scope(Mode::Enforce).status_for(Some("someone-else"));
    assert!(!status.scope_configured);
    assert!(
        status.enforcing_without_scope,
        "enforce + no tenant scope must be flagged, not read as healthy"
    );
    assert!(status.scope.is_none());
}

#[test]
fn status_shows_a_scope_even_when_mode_is_off() {
    // `scope_for` hides the scope under Mode::Off; status must not, or an
    // operator cannot tell "off with a scope staged" from "nothing set".
    let status = config_with_scope(Mode::Off).status_for(Some("weaver"));
    assert_eq!(status.mode, Mode::Off);
    assert!(status.scope_configured);
    // Off is not "enforcing without scope" — it is not enforcing at all.
    assert!(!status.enforcing_without_scope);
}

#[test]
fn status_without_a_tenant_configures_no_scope() {
    let status = config_with_scope(Mode::Enforce).status_for(None);
    assert!(!status.scope_configured);
    // No tenant means nothing to enforce against — flagged under enforce.
    assert!(status.enforcing_without_scope);
}
