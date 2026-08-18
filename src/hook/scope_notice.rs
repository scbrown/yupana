//! The out-of-ground notice, delivered where the agent can read it.
//!
//! ## Why this is not in `scope_arm`
//!
//! `scope_arm` is the pre-edit half of the capability ladder and it stays that
//! way. But at `[yupana.policy] work_item_scope = "advise"` its advisory goes
//! out as `systemMessage`, and a `systemMessage` surfaces in the OPERATOR's
//! pane — it does not enter the model's context. That is measured, not
//! inferred: the same violating edit was run in both modes on a live pane and
//! `advise` returned an ordinary success to the model while `enforce` returned
//! the reason (`docs/book/src/reference/policy-guard.md`, "An advise run is
//! visible to the operator, not to the agent").
//!
//! So the first rung of a ladder whose whole purpose is to TELL an agent where
//! its work ends was telling nobody who could act on it. "Agents behaved no
//! differently under advise" was never evidence they had seen anything.
//!
//! `PostToolUse` `additionalContext` is the channel that reaches the model
//! without blocking, and `post_edit` already speaks it. Hence this module: the
//! edit has landed, nothing is being prevented, and the agent is told — before
//! its next action — that it stepped outside the ground its work item defines.
//! That is precisely what the Post-Action Auditor is for (`paa`): judge
//! completed state, change what happens next.
//!
//! ## Division of labour with `scope_arm`, so there is one definition of "outside"
//!
//! Both call [`crate::policy::Scope::check_path`] against the same projected
//! [`crate::policy::WorkItemScopes`]. Neither owns a second notion of the
//! boundary. `scope_arm` keeps the `enforce` deny (where the reason DOES reach
//! the model) and the once-per-session unknown-scope advisory; this module
//! carries the `advise` rung's message.
//!
//! ## Cost
//!
//! This projects, which `scope_arm` deliberately refuses to do — the pre-edit
//! path already costs 157–322 ms against a 100 ms deadline and the scope arm
//! rides the shared refresh rather than adding a round-trip. Post-edit has no
//! such budget: the edit is already written and nothing waits on this. That is
//! the second reason the notice lives here rather than there.
//!
//! Silent whenever it cannot speak honestly: no plate, no scope for the item,
//! an unreachable store, or an edit inside the ground all produce nothing. An
//! unknown scope is UNKNOWN — it is not a deviation, and saying so here would
//! turn the most common state into noise.

use std::path::{Path, PathBuf};

use super::HookInput;

use crate::config::YupanaConfig;
use crate::policy::Mode;

/// Resolve the payload to a repo-relative path and config, then ask
/// [`notice`] whether this edit left its work item's ground.
///
/// Kept beside the other section producers and, like them, independent of
/// them: the blast-radius advisory bails on anything non-Rust, and scope has no
/// business inheriting that exit — an edit that leaves the ground leaves it
/// whatever the file's extension is.
#[must_use]
pub(super) fn for_payload(input_json: &str, default_root: &Path) -> Option<String> {
    let input = HookInput::parse(input_json)?;
    let file = PathBuf::from(input.tool_input.file_path.clone()?);
    let root = input.root(default_root);
    let config = crate::config::YupanaConfig::resolve(None, &root).ok()?;
    let rel = crate::hook::measure::relative(&file, &root);
    notice(&rel, &root, &config)
}

/// The `additionalContext` section for an edit that landed outside its work
/// item's observed ground, or `None` when there is nothing honest to say.
#[cfg(feature = "quipu")]
#[must_use]
pub(super) fn notice(rel: &str, root: &Path, config: &YupanaConfig) -> Option<String> {
    if effective_rung(config) == Mode::Off {
        return None;
    }
    let item = crate::plate::current()?;

    let mut registry = crate::project::ProjectionRegistry::new(&config.quipu.endpoint);
    let cache_age = match registry.refresh_or_cached(
        crate::projection_cache::cache_path().as_deref(),
        config.quipu.projection_cache_ttl_secs,
        crate::projection_cache::now_secs(),
    ) {
        Ok(crate::project::ProjectionSource::Live) => None,
        Ok(crate::project::ProjectionSource::Cache { age_secs, .. }) => Some(age_secs),
        // A store we could not read tells us nothing about the scope. Silence,
        // NOT a deviation: the pre-edit path already reports projection failure
        // as its own `fail_open` record, and inventing an out-of-scope notice
        // from an absent projection would be the same fabrication the spool
        // refuses everywhere else.
        Err(_) => return None,
    };

    let scope = registry.work_item_scopes()?.scope_for(&item)?;
    let violation = scope.check_path(rel, &item)?;
    let _ = root;

    crate::metrics::emit(
        "scope_notice",
        &[
            ("item", item.clone().into()),
            ("rule", violation.rule.clone().into()),
            ("point", "PAA".into()),
        ],
    );

    let ground: Vec<&str> = scope.allow_paths.iter().map(String::as_str).collect();
    let staleness = cache_age.map_or_else(String::new, |age| {
        format!(" (observed scope served from a projection cached {age}s ago)")
    });

    // DEVIATION PACES DISCLOSURE. The symmetry work-scoped-governance.md §3
    // names — "if the graph can predict what an agent may access, it can
    // predict what that agent will need to read, and those are nearly the same
    // query" — is otherwise only exploited at assignment time. Here it is
    // seeded on the path the agent just stepped onto, which is the moment the
    // answer is most useful and the moment nobody was serving it.
    //
    // Affordable here and not at the gate: the edit has landed, nothing waits
    // on this, and the pre-edit budget that forbids `scope_arm` a round-trip
    // does not apply. Empty on any failure — a notice must degrade to the
    // advisory it already was, never to an error.
    let prior = crate::brief_deviation::items_touching_path(&config.quipu.endpoint, rel);
    let prior_line = if prior.is_empty() {
        String::new()
    } else {
        let mut line = String::from("\n\nWhat the graph knows about `");
        line.push_str(rel);
        line.push_str("` — prior work that touched it:\n");
        for (id, outcome) in &prior {
            match outcome.as_deref() {
                Some("done") => line.push_str(&format!(
                    "- `{id}` — outcome: done. SUCCESSFUL prior work here: read it before \
                     inventing an approach.\n"
                )),
                Some(other) => line.push_str(&format!("- `{id}` — outcome: {other}.\n")),
                None => line.push_str(&format!(
                    "- `{id}` — still open; coordinate rather than overlap.\n"
                )),
            }
        }
        line
    };

    Some(format!(
        "## Outside your work item's ground (yupana)\n\n\
         This edit to `{rel}` landed OUTSIDE the paths prior work on `{item}` has \
         touched{staleness}. Nothing was blocked — the edit stands.\n\n\
         The ground for `{item}` is: {}\n\n\
         Scope provenance is OBSERVED, which means it is what work on this item HAS \
         touched, not everything it MAY touch — so a genuinely new file will read as \
         outside until its first commit lands. Two legitimate ways forward, and \
         which one applies is yours to judge: if this change belongs to `{item}`, \
         carry on and the ground will grow to include it; if it belongs to \
         DIFFERENT work, update your tracked item before going further, so the \
         record attributes it to the work that actually caused it.{prior_line}",
        ground
            .iter()
            .map(|p| format!("`{p}`"))
            .collect::<Vec<_>>()
            .join(", ")
    ))
}

/// Without the `quipu` feature there is no projected scope to check against, so
/// the notice is structurally absent rather than silently empty.
#[cfg(not(feature = "quipu"))]
#[must_use]
pub(super) fn notice(_rel: &str, _root: &Path, _config: &YupanaConfig) -> Option<String> {
    None
}

/// The rung actually in force: `work_item_scope`, ceilinged by the ambient
/// `mode`. Same composition `scope_arm` and `brief::posture_line` use — a
/// deployment must not be able to arm the notice by raising one knob alone.
#[cfg_attr(not(feature = "quipu"), allow(dead_code))]
fn effective_rung(config: &YupanaConfig) -> Mode {
    if config
        .policy
        .work_item_scope
        .is_lower_than(config.policy.mode)
    {
        config.policy.work_item_scope
    } else {
        config.policy.mode
    }
}

#[cfg(all(test, feature = "quipu"))]
// Test names shout the invariant they turn on, the same convention `pre_edit_test`
// states and scopes to tests (yupana #83).
#[allow(non_snake_case)]
mod tests {
    use super::*;
    use crate::policy::{Scope, WorkItemScopes};

    /// The rung composition, which is what decides whether the notice speaks at
    /// all. Both knobs must be raised: a deployment that sets `work_item_scope`
    /// while the ambient mode is still `off` has armed nothing, and the ceiling
    /// is what makes that true here as it is at the gate.
    #[test]
    fn the_ambient_mode_is_a_CEILING_not_a_default() {
        let mut config = YupanaConfig::default();
        config.policy.mode = Mode::Off;
        config.policy.work_item_scope = Mode::Enforce;
        assert_eq!(
            effective_rung(&config),
            Mode::Off,
            "work_item_scope must not be able to outrank the ambient mode"
        );

        config.policy.mode = Mode::Advise;
        config.policy.work_item_scope = Mode::Enforce;
        assert_eq!(effective_rung(&config), Mode::Advise);

        config.policy.mode = Mode::Enforce;
        config.policy.work_item_scope = Mode::Advise;
        assert_eq!(effective_rung(&config), Mode::Advise);
    }

    /// RED. A path outside the item's observed ground is a deviation.
    #[test]
    fn a_path_outside_the_items_ground_is_a_violation() {
        let scopes = WorkItemScopes::from_rows([
            ("aegis-1".to_string(), "src/weave.rs".to_string()),
            ("aegis-1".to_string(), "src/loom.rs".to_string()),
        ]);
        let scope: Scope = scopes.scope_for("aegis-1").expect("item has a ground");
        assert!(
            scope.check_path("src/elsewhere.rs", "aegis-1").is_some(),
            "an edit outside the ground must register as a violation"
        );
    }

    /// GREEN, and the control that makes the RED case mean something. Without
    /// it the same assertions would pass against a boundary that called
    /// everything a deviation — which is the failure mode that trains agents to
    /// tune the notice out.
    #[test]
    fn a_path_INSIDE_the_items_ground_is_silent() {
        let scopes =
            WorkItemScopes::from_rows([("aegis-1".to_string(), "src/weave.rs".to_string())]);
        let scope: Scope = scopes.scope_for("aegis-1").expect("item has a ground");
        assert!(
            scope.check_path("src/weave.rs", "aegis-1").is_none(),
            "an edit inside the ground must produce nothing at all"
        );
    }

    /// UNKNOWN IS NOT A DEVIATION. An item with no observed paths has no scope,
    /// and the notice must stay silent rather than reporting every edit as
    /// out-of-ground — which is what an empty-scope-denies-everything reading
    /// would do, and it is the most common state for a fresh work item.
    #[test]
    fn an_item_with_no_observed_ground_has_no_scope_to_violate() {
        let scopes =
            WorkItemScopes::from_rows([("aegis-1".to_string(), "src/weave.rs".to_string())]);
        assert!(
            scopes.scope_for("aegis-2").is_none(),
            "an item with no observed paths is UNKNOWN scope, never an empty one"
        );
    }
}
