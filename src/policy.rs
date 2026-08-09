//! Capability-scoped edit policy — the §5.8 trust boundary, made concrete.
//!
//! A capability-scoped agent (a polecat) is provisioned with a *scope*: the
//! paths it may write and how far a single edit may reach. This module holds
//! that policy and evaluates an edit against it (FR-25). It is deliberately
//! pure — no I/O, no graph building — so the rules are testable in isolation
//! and the [`crate::hook`] guard stays a thin adapter.
//!
//! Two things are checked, both against the *requesting tenant's* graph:
//!
//! 1. **Path scope** — is the edited file inside the tenant's writable scope?
//! 2. **Blast radius** — does the edit transitively affect more symbols or
//!    files than the scope permits (the FR-12 primitive, used as a guard)?
//!
//! Enforcement is opt-in ([`Mode::Off`] by default). A wrong hard-deny is worse
//! than no guard, so a scope should be staged in [`Mode::Advise`] first.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// How far the guard follows the call graph when sizing an edit's blast radius.
const DEFAULT_MAX_HOPS: u32 = 5;

/// What the guard does with the violations it finds.
///
/// This is a typed enum rather than the free-form string other config fields
/// use: a typo in `mode` must be a loud config error, never a silently inert
/// guard.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// The guard is inert. The default.
    #[default]
    Off,
    /// Compute and report violations, but never deny.
    Advise,
    /// Deny violations.
    Enforce,
}

impl Mode {
    /// Whether this mode weakens enforcement relative to `other`.
    #[must_use]
    pub fn is_lower_than(self, other: Self) -> bool {
        self.rank() < other.rank()
    }

    const fn rank(self) -> u8 {
        match self {
            Self::Off => 0,
            Self::Advise => 1,
            Self::Enforce => 2,
        }
    }

    /// The lowercase name, matching the `[yupana.policy] mode` config value.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Off => "off",
            Mode::Advise => "advise",
            Mode::Enforce => "enforce",
        }
    }
}

impl std::fmt::Display for Mode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The `[yupana.policy]` table.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PolicyConfig {
    /// What to do with violations.
    pub mode: Mode,
    /// Wall-clock budget for the whole guard, in milliseconds (FR-31). On
    /// expiry the guard abandons its analysis and allows the edit.
    pub deadline_ms: u64,
    /// Emit a user-visible `systemMessage` the first time the guard fails open
    /// in a session.
    pub notify_on_fail_open: bool,
    /// How far to follow the call graph when sizing an edit.
    pub max_hops: u32,
    /// Per-tenant capability scopes, keyed by tenant/role id. A tenant with no
    /// entry here is unconstrained.
    pub scopes: BTreeMap<String, Scope>,
    /// Structural (tree-sitter-tier) rules applied to the text an edit
    /// introduces (`[[yupana.policy.rules]]`). Unlike [`Self::scopes`], these are
    /// not per-tenant: a rule like "no ticket id in a comment" governs the code,
    /// not who wrote it. See [`crate::rules`].
    pub rules: Vec<crate::rules::Rule>,
}

impl Default for PolicyConfig {
    fn default() -> Self {
        Self {
            mode: Mode::Off,
            deadline_ms: 100,
            notify_on_fail_open: true,
            max_hops: DEFAULT_MAX_HOPS,
            scopes: BTreeMap::new(),
            rules: Vec::new(),
        }
    }
}

impl PolicyConfig {
    /// The scope governing `tenant`, or `None` when the tenant is
    /// unconstrained (no entry) or the guard is [`Mode::Off`].
    #[must_use]
    pub fn scope_for(&self, tenant: Option<&str>) -> Option<&Scope> {
        if self.mode == Mode::Off {
            return None;
        }
        self.scopes.get(tenant?)
    }

    /// A snapshot of the policy layer for `yupana status`, resolved for `tenant`.
    ///
    /// Observability is the whole point (aegis-hac0): an operator must be able
    /// to see whether the guard is armed for a tenant and against what. The
    /// scope is read straight from the table here — NOT via [`Self::scope_for`],
    /// which hides it under [`Mode::Off`] — so status can distinguish "mode off but a
    /// scope exists" from "no scope configured at all".
    #[must_use]
    pub fn status_for(&self, tenant: Option<&str>) -> PolicyStatus {
        let scope = tenant.and_then(|t| self.scopes.get(t));
        PolicyStatus {
            mode: self.mode,
            scope_configured: scope.is_some(),
            // Enforce with no scope for this tenant is armed in appearance and
            // inert in effect — the disarm-that-reads-as-healthy shape of #36
            // and aegis-ll3p. It is a caveat, never a clean state.
            enforcing_without_scope: self.mode == Mode::Enforce && scope.is_none(),
            scope: scope.map(ScopeSummary::of),
        }
    }
}

/// A `yupana status` view of the policy layer, resolved for one tenant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PolicyStatus {
    /// Enforcement mode in force.
    pub mode: Mode,
    /// Whether a capability scope is configured for the queried tenant.
    pub scope_configured: bool,
    /// Path-rule and ceiling summary, when a scope is configured.
    pub scope: Option<ScopeSummary>,
    /// Configured to enforce, but with no scope for this tenant.
    pub enforcing_without_scope: bool,
}

/// The shape and ceilings of a scope, without its contents — enough for an
/// operator to confirm the guard is looking at what they expect.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ScopeSummary {
    /// Number of `allow_paths` globs (0 = any path permitted).
    pub allow_paths: usize,
    /// Number of `deny_paths` globs.
    pub deny_paths: usize,
    /// The symbol blast-radius ceiling, if set.
    pub max_impacted_symbols: Option<usize>,
    /// The file blast-radius ceiling, if set.
    pub max_impacted_files: Option<usize>,
}

impl ScopeSummary {
    fn of(scope: &Scope) -> Self {
        Self {
            allow_paths: scope.allow_paths.len(),
            deny_paths: scope.deny_paths.len(),
            max_impacted_symbols: scope.max_impacted_symbols,
            max_impacted_files: scope.max_impacted_files,
        }
    }
}

/// One tenant's capability scope.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Scope {
    /// Globs of repo-relative paths this tenant may write. Empty = any path.
    pub allow_paths: Vec<String>,
    /// Globs this tenant may not write. Beats [`Scope::allow_paths`].
    pub deny_paths: Vec<String>,
    /// Most symbols a single edit may transitively affect.
    pub max_impacted_symbols: Option<usize>,
    /// Most files a single edit may transitively affect.
    pub max_impacted_files: Option<usize>,
}

/// Why an edit was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViolationKind {
    /// The edited path is outside the tenant's writable scope.
    PathOutOfScope,
    /// The edit reaches further than the scope permits.
    BlastRadiusExceeded,
}

/// A single policy violation, with the text shown to the model.
#[derive(Debug, Clone)]
pub struct Violation {
    /// Which rule was broken.
    pub kind: ViolationKind,
    /// Model-facing explanation: what was exceeded, by how much, what to do.
    pub message: String,
    /// Stable id of what actually fired, for the audit record (yupana #77): the
    /// matching `deny_paths` glob, `allow_paths` when nothing matched, or the
    /// exceeded ceiling. `kind` says which CLASS denied; this says which rule —
    /// the field that tells a wrongly-scoped rule from a correct one.
    pub rule: String,
}

/// The size of an edit's transitive impact, as measured against the graph.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BlastRadius {
    /// Distinct symbols transitively affected.
    pub symbols: usize,
    /// Distinct files transitively affected.
    pub files: usize,
}

impl Scope {
    /// Check `rel` (a repo-relative path) against this scope's path globs.
    ///
    /// `deny_paths` wins over `allow_paths`; an empty `allow_paths` permits any
    /// path. An unparseable glob never matches — a malformed pattern must not
    /// silently widen or narrow the scope, and [`Scope::glob_errors`] surfaces
    /// it to the operator instead.
    #[must_use]
    pub fn check_path(&self, rel: &str, tenant: &str) -> Option<Violation> {
        if let Some(pattern) = self.deny_paths.iter().find(|p| glob_matches(p, rel)) {
            return Some(Violation {
                kind: ViolationKind::PathOutOfScope,
                message: format!(
                    "yupana: `{rel}` is explicitly denied to tenant `{tenant}` (matches deny_paths \
                     pattern `{pattern}`). This path is outside your capability scope — do not \
                     retry it; if the change genuinely belongs there, ask for a wider scope."
                ),
                rule: format!("deny_paths:{pattern}"),
            });
        }

        if self.allow_paths.is_empty() || self.allow_paths.iter().any(|p| glob_matches(p, rel)) {
            return None;
        }

        Some(Violation {
            kind: ViolationKind::PathOutOfScope,
            message: format!(
                "yupana: `{rel}` is outside the writable capability scope of tenant `{tenant}` \
                 (allowed: {}). Make the change inside your scope, or ask for a wider one.",
                self.allow_paths.join(", ")
            ),
            // No single pattern matched, so the allow LIST is the rule that
            // fired. Naming the list (not a pattern) keeps the record honest
            // about why: nothing matched, rather than something did.
            rule: "allow_paths".to_string(),
        })
    }

    /// Check a measured [`BlastRadius`] against this scope's ceilings.
    #[must_use]
    pub fn check_blast(&self, radius: BlastRadius, rel: &str, tenant: &str) -> Option<Violation> {
        let symbols_over = self
            .max_impacted_symbols
            .is_some_and(|max| radius.symbols > max);
        let files_over = self
            .max_impacted_files
            .is_some_and(|max| radius.files > max);
        if !symbols_over && !files_over {
            return None;
        }

        let mut exceeded = Vec::new();
        // Which ceiling(s) fired, for the audit record. A list, not one name:
        // an edit can breach both, and reporting only the first would send an
        // operator to widen one ceiling and hit the other.
        let mut fired = Vec::new();
        if let (true, Some(max)) = (symbols_over, self.max_impacted_symbols) {
            exceeded.push(format!("{} symbols (ceiling {max})", radius.symbols));
            fired.push("max_impacted_symbols");
        }
        if let (true, Some(max)) = (files_over, self.max_impacted_files) {
            exceeded.push(format!("{} files (ceiling {max})", radius.files));
            fired.push("max_impacted_files");
        }

        Some(Violation {
            rule: fired.join("+"),
            kind: ViolationKind::BlastRadiusExceeded,
            message: format!(
                "yupana: editing `{rel}` reaches {} — beyond the blast radius allowed for tenant \
                 `{tenant}`. Split this into a narrower change that touches fewer callers, or ask \
                 for a wider capability scope. (tree-sitter tier: the reach is an approximation.)",
                exceeded.join(" and ")
            ),
        })
    }

    /// Patterns in this scope that are not valid globs, as
    /// `(pattern, reason)`. A scope with malformed globs is misconfigured and
    /// the guard says so rather than quietly under-enforcing.
    #[must_use]
    pub fn glob_errors(&self) -> Vec<(String, String)> {
        self.allow_paths
            .iter()
            .chain(self.deny_paths.iter())
            .filter_map(|pattern| {
                glob::Pattern::new(pattern)
                    .err()
                    .map(|e| (pattern.clone(), e.to_string()))
            })
            .collect()
    }
}

/// Whether `rel` matches glob `pattern`. An invalid pattern never matches.
///
/// `foo/**` is normalized to also cover `foo`'s direct children, so the natural
/// reading of `src/**` ("everything under src") holds regardless of how the
/// underlying glob engine treats a trailing `**`.
fn glob_matches(pattern: &str, rel: &str) -> bool {
    let direct = glob::Pattern::new(pattern).is_ok_and(|p| p.matches(rel));
    if direct {
        return true;
    }
    match pattern.strip_suffix("/**") {
        Some(prefix) => glob::Pattern::new(&format!("{prefix}/*")).is_ok_and(|p| p.matches(rel)),
        None => false,
    }
}

#[cfg(test)]
mod tests {
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
}
