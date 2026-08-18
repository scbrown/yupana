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
    /// Run the FR-23 buffer verifier as an arm of the pre-edit guard: reject
    /// edits that introduce references resolving to nothing (hallucinated
    /// identifiers, wrong arity, unresolved `mod` imports). Opt-in and off by
    /// default; rides inside `deadline_ms` and fails open like every other arm.
    pub verify: bool,
    /// Enforcement level for the work-item (observed) scope rung — the ladder
    /// fallback when a tenant has no `[yupana.policy.scopes.*]` entry. The
    /// rung both influences and constrains: its message always says what the
    /// right action is, and at `enforce` an out-of-scope edit is denied
    /// rather than advised. Off by default (opt-in, like `verify`): arming it
    /// also arms the unknown-scope advisory, and a deployment should choose
    /// that noise deliberately. Stage `advise` before `enforce` — an observed
    /// scope grows with use, and an incomplete one hard-denying legitimate
    /// work is an outage. The ambient `mode` stays a ceiling on top.
    pub work_item_scope: Mode,
    /// Enforcement level for the ACTION scope — the `allow_targets` /
    /// `deny_targets` halves of a declared scope, checked against the
    /// `(class, target)` a shell command resolves to.
    ///
    /// Off by default and staged exactly like `work_item_scope`, for a sharper
    /// reason: `pre-bash` has been RECORD-ONLY since it shipped, and a hook
    /// that starts refusing because someone set a knob it did not know existed
    /// is enforcement arriving without its gate. `advise` first, then `enforce`
    /// once the spool says what it would have refused. The ambient `mode` is a
    /// ceiling on top, as everywhere else.
    pub action_scope: Mode,
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
            verify: false,
            work_item_scope: Mode::Off,
            action_scope: Mode::Off,
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
    /// Number of `allow_targets` globs (0 = any action target permitted).
    pub allow_targets: usize,
    /// Number of `deny_targets` globs.
    pub deny_targets: usize,
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
            allow_targets: scope.allow_targets.len(),
            deny_targets: scope.deny_targets.len(),
            max_impacted_symbols: scope.max_impacted_symbols,
            max_impacted_files: scope.max_impacted_files,
        }
    }
}

/// Where a capability scope came from — the work-scoped-governance trust
/// ladder (docs/work-scoped-governance.md). The rung decides the ceiling: only
/// a `Declared` scope may hard-deny; `Derived` and `Observed` scopes advise.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ScopeProvenance {
    /// Explicit scope edges — today, the operator's static TOML table.
    Declared,
    /// Derived from labels, parent work, or the structural graph. (No
    /// producer yet; named so records can carry it when one lands.)
    Derived,
    /// What prior sessions on the work item actually touched, projected from
    /// the graph's commit-provenance chain. Grows with use; advises only.
    Observed,
}

impl ScopeProvenance {
    /// The lowercase rung name, for records and messages.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Declared => "declared",
            Self::Derived => "derived",
            Self::Observed => "observed",
        }
    }
}

// The projected work-item maps live in `policy_items` (file-size ratchet), and
// are re-exported here because `crate::policy::WorkItemScopes` is the path
// every caller, cache field and test already names. Moving a type for size is
// not a reason to make thirty call sites say where it sleeps.
pub use crate::policy_items::{WorkItemParents, WorkItemScopes};

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
    /// Globs of `class:target` this tenant may ACT on through a shell command
    /// — `host:build-01`, `service:metrics`, `container:*`. Empty = any target.
    ///
    /// The action surface, unlike the path one, has no observed rung: nothing
    /// in the graph records which hosts an item's prior work touched. So this
    /// is `declared` and only `declared`, which is also the only provenance
    /// the trust ladder permits to hard-deny (see [`ScopeProvenance`]).
    ///
    /// MATCHED ONLY WHEN THE RESOLVER RESOLVED. `crate::action` abstains on
    /// anything whose target is not unambiguous from syntax, and an abstention
    /// is never a violation: a scope that refused what it could not identify
    /// would deny every pipeline and script on the host.
    pub allow_targets: Vec<String>,
    /// Globs of `class:target` this tenant may not act on. Beats
    /// [`Scope::allow_targets`], the same precedence `deny_paths` has.
    pub deny_targets: Vec<String>,
}

/// Why an edit was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViolationKind {
    /// The edited path is outside the tenant's writable scope.
    PathOutOfScope,
    /// The edit reaches further than the scope permits.
    BlastRadiusExceeded,
    /// The shell command acts on a target outside the tenant's scope.
    TargetOutOfScope,
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
    /// Check a resolved action target — `class:target`, e.g. `host:build-01` —
    /// against this scope's target globs.
    ///
    /// Same precedence and same empty-means-any rule as [`Scope::check_path`],
    /// so an operator who has learned one has learned both.
    ///
    /// THE CALLER MUST NOT PASS AN ABSTENTION. `crate::action` answers
    /// `Unknown` for every command whose target is not unambiguous from syntax
    /// — a pipeline, a shell function, a script that ssh's internally — and
    /// those must reach here as "no check performed", never as a target named
    /// "unknown" that a glob could match. A scope that refused what it could
    /// not identify would refuse most of the shell.
    #[must_use]
    pub fn check_target(&self, class: &str, target: &str, tenant: &str) -> Option<Violation> {
        let subject = format!("{class}:{target}");
        if let Some(pattern) = self.deny_targets.iter().find(|p| glob_matches(p, &subject)) {
            return Some(Violation {
                kind: ViolationKind::TargetOutOfScope,
                message: format!(
                    "yupana: `{subject}` is explicitly denied to tenant `{tenant}` (matches \
                     deny_targets pattern `{pattern}`). This target is outside your capability \
                     scope — do not retry it; if the action genuinely belongs there, ask for a \
                     wider scope."
                ),
                rule: format!("deny_targets:{pattern}"),
            });
        }
        if self.allow_targets.is_empty()
            || self.allow_targets.iter().any(|p| glob_matches(p, &subject))
        {
            return None;
        }
        Some(Violation {
            kind: ViolationKind::TargetOutOfScope,
            message: format!(
                "yupana: `{subject}` is outside the capability scope of tenant `{tenant}` \
                 (allow_targets: {}). Act on a target within that scope, or ask an operator to \
                 widen it — do not retry this one unchanged.",
                self.allow_targets.join(", ")
            ),
            rule: "allow_targets".to_string(),
        })
    }

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
#[path = "policy_test.rs"]
mod policy_test;
