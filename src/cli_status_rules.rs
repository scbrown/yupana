//! The rule-set half of `yupana status`: the measurement, its five states, and
//! the content digest — split out of `cli_status` for size (yupana #83). The
//! rendering stays in `cli_status`; this module is the single measurement both
//! surfaces consume ("two renderers of one measurement, never two
//! measurements").

use crate::config::YupanaConfig;

/// What `yupana status` says about the RULE SET — measured, not asserted.
///
/// THIS LINE USED TO BE A STRING LITERAL. It printed
/// `rule set : none — never loaded (local config only)` unconditionally, on
/// every box, whatever the graph held. The intent was to report the absence of
/// the *signed resident cache*, which genuinely does not exist yet — but the
/// words it chose describe the RULE SET, and they were read the way they read.
///
/// The cost was not hypothetical. An operator ran `yupana status`, saw
/// "none — never loaded", and concluded the governed rule plane was empty and
/// that every claim of change-time enforcement in this deployment was false. A
/// P1 was filed to build what already existed. Measured at that moment: seven
/// rules in the graph, projected on every edit, verifiably firing.
///
/// This repo is careful about controls that report health they never measured.
/// This is the mirror image — a control reporting FAILURE it never measured —
/// and it is not the harmless direction. A false red burns the time of whoever
/// believes it, and teaches everyone else to discount the surface.
///
/// So: the signed-cache line stays (its absence is real and worth reporting),
/// but it says PROVENANCE, and the rule-set line now counts what is actually
/// loaded, through the same projection the guard itself uses. One reader, no
/// second opinion — a status that could disagree with the hook would be a third
/// thing to keep in sync.
pub(super) struct RuleSetStatus {
    pub local: usize,
    /// None = the graph plane is off for this build/config.
    pub projected: Option<usize>,
    pub structural: usize,
    pub text: usize,
    /// Set when the projection was attempted and FAILED — never conflated with
    /// a successful projection of zero rules.
    pub error: Option<String>,
    pub graph_enabled: bool,
    /// WHICH rule set is live, as a content digest over the projected rules
    /// (aegis-hac0 acceptance: "yupana status reports rule-set version").
    ///
    /// There is no signed cache yet, so there is no authoritative VERSION to
    /// report — and inventing a version number for an unauthenticated fetch
    /// would be the false-confidence direction this file already got burned on.
    /// A digest is the honest form of the same question: it answers "is the rule
    /// set I am enforcing the same one as an hour ago / on that other host?"
    /// without claiming provenance it does not have. When the signed cache
    /// lands it carries a real version and this becomes the thing that version
    /// is checked against.
    pub digest: Option<String>,
    /// How many projection attempts it took (see [`PROJECTION_ATTEMPTS`]).
    pub attempts: usize,
    /// Age in seconds of the DURABLE cache these rules were served from, when
    /// the live projection failed and the cache answered instead (aegis-0upyu).
    ///
    /// `None` means the rules are live (or that there are none). Its presence is
    /// what separates [`RuleSetState::Stale`] from [`RuleSetState::Degraded`]:
    /// rules in force but unconfirmed, versus no rules at all.
    pub cache_age_secs: Option<u64>,
}

/// What state the rule plane is in — the field an operator and `st doctor` gate
/// on, and the reason `yupana status` can exit non-zero (aegis-hac0).
///
/// These are kept DISTINCT for the reason the whole bead exists: "the graph
/// projected no rules" and "I could not reach the graph" produce the same
/// number of enforced rules (zero) and mean opposite things. Collapsing them is
/// how a policy layer goes green-and-dead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RuleSetState {
    /// The graph plane is off for this build/config — nothing is projected, and
    /// that is a configuration, not a fault.
    Off,
    /// Rules projected and in force.
    Loaded,
    /// The graph answered, and answered with nothing. Armed and empty.
    Empty,
    /// The graph could not be reached, but the DURABLE projection cache
    /// answered — so rules ARE in force, computed against a catalogue nobody
    /// has confirmed since (aegis-0upyu).
    ///
    /// Distinct from [`RuleSetState::Degraded`] for exactly the reason `Empty`
    /// is distinct from it: "enforcing an unconfirmed rule set" and "enforcing
    /// nothing" are opposite operational facts that a single red state would
    /// merge. This one does NOT exit non-zero — the guard is working.
    Stale,
    /// The graph could not be reached (or its answer did not decode) AND no
    /// cache could be served. The guard FAILS OPEN in this state, so it is a
    /// failure surface, not a note.
    Degraded,
}

impl RuleSetState {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            RuleSetState::Off => "off",
            RuleSetState::Loaded => "loaded",
            RuleSetState::Empty => "empty",
            RuleSetState::Stale => "stale",
            RuleSetState::Degraded => "degraded",
        }
    }
}

impl RuleSetStatus {
    pub(super) fn state(&self) -> RuleSetState {
        if !self.graph_enabled {
            return RuleSetState::Off;
        }
        // A live failure that the cache answered is STALE, not degraded. Before
        // the durable cache existed these were the same event; conflating them
        // now would make `status` assert the guard fails open on every edit at
        // exactly the moments it does not.
        if self.error.is_some() && self.cache_age_secs.is_some() {
            return RuleSetState::Stale;
        }
        match (self.projected, &self.error) {
            (_, Some(_)) => RuleSetState::Degraded,
            (Some(0), None) => RuleSetState::Empty,
            (Some(_), None) => RuleSetState::Loaded,
            // Defensive only: when the graph plane is enabled, the retry loop
            // below always records either `projected` on success or `error` on
            // failure. Keep this loud fallback rather than pretending an
            // impossible no-result is healthy if a future refactor violates
            // that invariant.
            (None, None) => RuleSetState::Degraded,
        }
    }
}

/// MEASURE the rule set. Split from rendering so the JSON and human surfaces
/// cannot disagree — two renderers of one measurement, never two measurements.
pub(super) fn measure_rule_set(config: &YupanaConfig) -> RuleSetStatus {
    let local = config.policy.rules.len();
    // `mut` is used only under the quipu feature; without it the graph plane
    // does not exist and the struct is returned as built.
    #[allow(unused_mut)]
    let mut st = RuleSetStatus {
        local,
        projected: None,
        structural: 0,
        text: 0,
        error: None,
        graph_enabled: false,
        digest: None,
        attempts: 0,
        cache_age_secs: None,
    };

    #[cfg(feature = "quipu")]
    {
        if !config.quipu.enabled || config.quipu.endpoint.is_empty() {
            return st;
        }
        st.graph_enabled = true;
        // The SAME path the guard takes (hook::rule_planes::governed_check), so
        // this can never report a rule set the guard would not use.
        for attempt in 1..=PROJECTION_ATTEMPTS {
            st.attempts = attempt;
            let mut registry = crate::project::ProjectionRegistry::new(&config.quipu.endpoint);
            match registry.refresh() {
                Ok(()) => {
                    st.text = registry.text_rules().len();
                    st.structural = registry.policies().len();
                    st.projected = Some(st.text + st.structural);
                    st.digest = Some(rule_set_digest(&registry));
                    st.error = None;
                    break;
                }
                Err(e) => {
                    st.error = Some(e.to_string());
                    if attempt < PROJECTION_ATTEMPTS {
                        std::thread::sleep(PROJECTION_BACKOFF);
                    }
                }
            }
        }
        // Every live attempt failed. Ask the SAME question the guard now asks
        // (`ProjectionRegistry::refresh_or_cached`): is there a servable cache?
        // If there is, the guard is still enforcing, and reporting `degraded`
        // here would tell an operator the fleet is unguarded while it is not.
        // The live error is KEPT, because "why could we not confirm this" is
        // the actionable half — the cache is the mitigation, not the fix.
        if st.error.is_some() {
            if let Some(path) = crate::projection_cache::cache_path() {
                let now = crate::projection_cache::now_secs();
                if let Ok(cached) = crate::projection_cache::load_servable(
                    &path,
                    &config.quipu.endpoint,
                    config.quipu.projection_cache_ttl_secs,
                    now,
                ) {
                    st.cache_age_secs = Some(cached.age_secs(now));
                    st.text = cached.text_rules.len();
                    st.structural = cached.policies.len();
                    st.projected = Some(st.text + st.structural);
                }
            }
        }
    }
    st
}

/// How many times `yupana status` retries the projection before calling the plane
/// DEGRADED — and why `status` retries when the hook deliberately does not.
///
/// MEASURED 2026-08-01 on this deployment: the governed text-rule query returns
/// in 0.19s median (15 samples, max 0.35s), but quipu intermittently stalls past
/// the guard's 2s ceiling — two spikes of 2.7s and 2.8s were caught inside one
/// short session, and the first `yupana status` run of that session reported
/// COULD NOT TELL purely because it landed on one.
///
/// The hook must NOT retry: it runs on every Edit/Write/MultiEdit across the
/// fleet and its whole latency budget is the reason the ceiling exists. `status`
/// is a human/`st doctor` surface off the hot path, so it can afford three tries
/// over a couple of seconds — and it must, because this command is about to gate
/// an exit code. A red that appears ~1 run in 10 from a transient blip is the
/// failure mode this repo keeps writing down: a control that cries wolf gets
/// routed around, and then the real red is invisible too.
///
/// Note what this does NOT do: it does not hide the flap. `attempts` is
/// reported, so "it took three tries" is visible rather than smoothed away.
#[cfg(feature = "quipu")]
const PROJECTION_ATTEMPTS: usize = 3;
#[cfg(feature = "quipu")]
const PROJECTION_BACKOFF: std::time::Duration = std::time::Duration::from_millis(250);

/// A stable content digest of the projected rule set.
///
/// Sorted and built from the fields that decide ENFORCEMENT — a rule's identity,
/// what it matches, how hard it bites, and where it is exempt. Two hosts
/// enforcing the same rules produce the same digest; a rule whose regex or tier
/// was edited in the graph produces a different one, which is exactly the
/// question "did the policy change under me?" that the projection alone cannot
/// answer. Deliberately NOT a hash of the raw HTTP body: that would also change
/// on binding order or a rationale typo, and a digest that changes for reasons
/// nobody cares about is one nobody watches.
#[cfg(feature = "quipu")]
fn rule_set_digest(registry: &crate::project::ProjectionRegistry) -> String {
    use sha2::{Digest, Sha256};
    let mut ids: Vec<String> = registry
        .text_rules()
        .iter()
        .map(|r| {
            format!(
                "text\u{1f}{}\u{1f}{}\u{1f}{:?}\u{1f}{}",
                r.name,
                r.pattern,
                r.tier,
                r.exempt_path_regex.as_deref().unwrap_or("")
            )
        })
        .collect();
    ids.extend(registry.policies().iter().map(|p| {
        format!(
            "struct\u{1f}{}\u{1f}{}\u{1f}{}",
            p.rule.name, p.rule.pattern, p.effect
        )
    }));
    ids.sort();
    format!("sha256:{}", hex::encode(Sha256::digest(ids.join("\u{1e}"))))
}

#[cfg(all(test, feature = "quipu"))]
mod rule_set_state_test {
    //! Test names shout the word the assertion turns on — the same convention,
    //! and the same scoped allow, as `hook::pre_edit::pre_edit_test`. Here the
    //! shouted words ARE the finding: STALE and DEGRADED are the two states
    //! this module exists to keep apart.
    #![allow(non_snake_case)]
    use super::*;

    fn status(projected: Option<usize>, error: Option<&str>, cache: Option<u64>) -> RuleSetStatus {
        RuleSetStatus {
            local: 0,
            projected,
            structural: 0,
            text: 0,
            error: error.map(str::to_string),
            graph_enabled: true,
            digest: None,
            attempts: 1,
            cache_age_secs: cache,
        }
    }

    /// The distinction the durable cache exists to preserve (aegis-0upyu),
    /// carried onto the surface `st doctor` gates on: a live-projection failure
    /// that the cache ANSWERED is not the state in which the guard fails open.
    /// Reporting it as `degraded` would tell an operator the fleet is unguarded
    /// at precisely the moments it is guarded.
    #[test]
    fn a_failed_projection_with_a_servable_cache_is_STALE_not_DEGRADED() {
        let st = status(Some(7), Some("timed out"), Some(30));
        assert_eq!(st.state(), RuleSetState::Stale);
        assert_ne!(
            st.state(),
            RuleSetState::Degraded,
            "the guard is enforcing; only a non-enforcing plane is degraded"
        );
    }

    /// ...and the same failure WITHOUT a cache stays degraded. This is the half
    /// that must not be softened: no cache means no rules, which means the guard
    /// really is failing open.
    #[test]
    fn a_failed_projection_with_no_cache_is_still_DEGRADED() {
        assert_eq!(
            status(None, Some("timed out"), None).state(),
            RuleSetState::Degraded
        );
    }

    /// Only `degraded` exits non-zero. `stale` must not, or every slow-quipu
    /// minute would fail an `st doctor` gate on a fleet that is enforcing
    /// correctly — and a gate that cries wolf gets switched off.
    #[test]
    fn only_DEGRADED_is_the_non_zero_exit_state() {
        assert!(status(Some(7), Some("timed out"), Some(30)).state() != RuleSetState::Degraded);
        assert!(status(None, Some("timed out"), None).state() == RuleSetState::Degraded);
        assert!(status(Some(0), None, None).state() != RuleSetState::Degraded);
        assert!(status(Some(3), None, None).state() != RuleSetState::Degraded);
    }

    /// The states stay distinguishable by NAME on the JSON surface — a consumer
    /// that gates on the string must be able to tell all five apart.
    #[test]
    fn every_state_has_its_own_name() {
        let names = [
            RuleSetState::Off.as_str(),
            RuleSetState::Loaded.as_str(),
            RuleSetState::Empty.as_str(),
            RuleSetState::Stale.as_str(),
            RuleSetState::Degraded.as_str(),
        ];
        let mut sorted = names.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), names.len(), "two states share a name");
    }
}
