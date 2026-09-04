//! `yupana status` and its policy rendering, split out of `cli` for size
//! (yupana #83). See `cli_analyze` for why this is a child module.

use super::*;
use crate::types::Tier;

use super::cli_status_rules::{measure_rule_set, RuleSetState, RuleSetStatus};

impl Cli {
    /// Perform the slow network projection outside the latency-sensitive edit
    /// path and atomically replace the durable hook cache on success.
    #[cfg(feature = "quipu")]
    pub(super) fn refresh_projection(&self) -> anyhow::Result<()> {
        let root = std::env::current_dir()?;
        let config = self.load_config(&root)?;
        anyhow::ensure!(
            !config.quipu.endpoint.is_empty(),
            "[yupana.quipu] endpoint is empty; no projection source is configured"
        );
        let cache = crate::projection_cache::cache_path()
            .ok_or_else(|| anyhow::anyhow!("no projection cache path could be resolved"))?;
        let now = crate::projection_cache::now_secs();
        let mut registry = crate::project::ProjectionRegistry::new(&config.quipu.endpoint);
        registry.refresh_and_persist(&cache, now)?;

        let projected = registry.policies().len() + registry.text_rules().len();
        if self.json {
            println!(
                "{}",
                serde_json::json!({
                    "outcome": "refreshed",
                    "cache": cache,
                    "written_at": now,
                    "projected": projected,
                    "structural": registry.policies().len(),
                    "text": registry.text_rules().len(),
                })
            );
        } else if !self.quiet {
            println!(
                "projection cache refreshed: {projected} rule(s) -> {}",
                cache.display()
            );
        }
        Ok(())
    }

    /// Print base ref, tier availability, and config.
    pub(super) fn status(&self) -> anyhow::Result<()> {
        let root = std::env::current_dir()?;
        let config = self.load_config(&root)?;
        let mode_provenance = YupanaConfig::policy_mode_provenance(&root, config.policy.mode)?;
        let tenant = self.tenant.as_deref().unwrap_or("(single-tenant)");
        // Resolve the configured base ref to a concrete commit (None outside a
        // repo / unresolved ref — degrade, never fail).
        let base_commit = crate::git::resolve_commit(&root, &config.base_ref);

        let policy = config.policy.status_for(self.tenant.as_deref());

        let rule_set = measure_rule_set(&config);
        if self.json {
            let out = serde_json::json!({
                "base_ref": config.base_ref,
                "base_commit": base_commit,
                "tenant": tenant,
                "tiers": Tier::served(),
                // WHICH LANGUAGES THIS BUILD CAN PARSE (aegis-ah0q1). A build
                // without `langs-extra` extracts Rust and silently skips the
                // other five: `export`/`promote` still exit 0 with a valid
                // document, so "no symbols" is indistinguishable from "no code
                // in that language" from the outside. This is the field that
                // makes a Rust-only deploy detectable without a probe repo.
                "languages": crate::extract::languages(),
                "quipu": { "enabled": config.quipu.enabled, "branch_model": config.quipu.branch_model },
                "policy": policy,
                "policy_mode_provenance": mode_provenance,
                // Whether guard records will carry their subject (yupana #77). An
                // operator has to be able to confirm this from OUTSIDE the
                // process: "recording is on" believed but untrue looks exactly
                // like "nothing was denied" in the spool.
                "metrics": { "record_paths": config.metrics.record_paths },
                // MEASURED, not asserted — see measure_rule_set. This object used
                // to be four hardcoded literals claiming nothing was ever loaded,
                // on every box, whatever the graph held.
                "rule_set": {
                    "local": rule_set.local,
                    "graph_enabled": rule_set.graph_enabled,
                    "projected": rule_set.projected,
                    "structural": rule_set.structural,
                    "text": rule_set.text,
                    "error": rule_set.error,
                    // The field `st doctor` gates on. `degraded` is the state in
                    // which the guard FAILS OPEN, and it is deliberately not the
                    // same value as `empty` (aegis-hac0) nor as `stale`
                    // (aegis-0upyu: rules in force from the durable cache).
                    "state": rule_set.state().as_str(),
                    // WHICH rule set, not just how many.
                    "digest": rule_set.digest,
                    "attempts": rule_set.attempts,
                    // Age of the cache these rules came from, when live
                    // projection failed and the cache answered. Null when live.
                    // A consumer that treats `stale` as fine without reading
                    // this cannot tell a 30-second lag from a week-old
                    // catalogue, and those warrant opposite reactions.
                    "cache_age_secs": rule_set.cache_age_secs,
                    // No signed cache => nothing is verified. Said out loud
                    // rather than omitted, so the field a signed cache will fill
                    // exists and reads honestly today.
                    "verification": "unsigned",
                },
                // The SIGNED resident cache still does not exist.
                // Its absence is real and stays reported — but as PROVENANCE of
                // the rules above, which is a different fact from "there are no
                // rules". Conflating the two is the bug this replaced.
                "signed_rule_set": { "loaded": false, "state": "never-loaded",
                    "note": "rules above are an unsigned live projection; the resident signed cache is not yet available" },
            });
            println!("{}", serde_json::to_string_pretty(&out)?);
        } else {
            let commit = base_commit.as_deref().map_or_else(
                || "(unresolved — not a git repo or ref absent)".to_string(),
                |c| c[..c.len().min(12)].to_string(),
            );
            println!("{}", "yupana status".bold());
            println!("  base ref    : {}", config.base_ref);
            println!("  base commit : {commit}");
            println!("  tenant      : {tenant}");
            println!("  tiers       : {}", Tier::served().join(", "));
            // A PARTIAL grammar set is the loud case, not the quiet one
            // (aegis-ah0q1). A Rust-only build answers every Python/TS/Go/Java/
            // C++ question with a confident empty — same shape, exit 0, valid
            // output — so the deficit has to announce itself here or it is
            // invisible until someone builds a probe repo to find it.
            let languages = crate::extract::languages();
            print!("  languages   : {}", languages.join(", "));
            if languages.contains(&"python") {
                println!();
            } else {
                println!(
                    "{}",
                    "  ← PARTIAL: built without `langs-extra`; every other \
                     language extracts ZERO symbols, silently"
                        .yellow()
                );
            }
            println!(
                "  quipu       : enabled={} branch_model={}",
                config.quipu.enabled, config.quipu.branch_model
            );
            println!(
                "  audit       : record_paths={}",
                match config.metrics.record_paths {
                    crate::audit::PathRecording::Off => "off (guard records carry no path)",
                    crate::audit::PathRecording::Relative => "relative",
                    crate::audit::PathRecording::Absolute => "absolute",
                }
            );
            print_policy_status(&policy, &mode_provenance);
            print_rule_set_status(&config, &rule_set);
        }

        // THE RULE SET IS A FAILURE SURFACE, NOT A LINE OF PROSE (aegis-hac0).
        //
        // `degraded` means the guard could not fetch the rules it enforces and
        // therefore FAILS OPEN on every edit until the graph answers again. That
        // is precisely the state the ruling refuses to let anyone learn from
        // silence — "you write to quipu, believe the rule is live, and cannot
        // tell". Before this, `yupana status` printed COULD NOT TELL in red and
        // exited 0, so no script could gate on it and no human had to notice.
        //
        // Only `degraded` exits non-zero. `empty` does not: an empty graph is a
        // true, quiet answer about policy, not a fault of this deployment, and
        // failing on it would make the exit code useless in every tree that has
        // no governed rules yet.
        if rule_set.state() == RuleSetState::Degraded {
            std::process::exit(EXIT_RULE_SET_DEGRADED);
        }
        if mode_provenance.lowered_by_project {
            std::process::exit(EXIT_POLICY_MODE_LOWERED);
        }
        Ok(())
    }
}

/// `yupana status` exit code for a rule plane that could not be loaded.
///
/// A dedicated code, not `1`: `1` is "the command itself failed" and `2` is
/// clap's argument error (and Claude Code's hook-deny), so a caller that gates
/// on this can tell "yupana could not tell me about the rules" apart from "yupana
/// did not run". The whole point is that the two stop looking alike.
pub(super) const EXIT_RULE_SET_DEGRADED: i32 = 3;

/// `yupana status` exit code when a workspace lowers the user's policy mode.
pub(super) const EXIT_POLICY_MODE_LOWERED: i32 = 4;

/// Render the policy section of `yupana status`.
///
/// Shows the enforcement mode, whether a scope applies to this tenant and its
/// ceilings, and — loudly — two states an operator must never learn from
/// silence: an `enforce` mode with no scope for the tenant (armed-looking, inert),
/// and the absence of a signed rule set (aegis-hac0).
fn print_policy_status(
    policy: &crate::policy::PolicyStatus,
    provenance: &crate::config::PolicyModeProvenance,
) {
    let scope = match &policy.scope {
        Some(s) => {
            let ceiling = |c: Option<usize>| c.map_or_else(|| "—".to_string(), |n| n.to_string());
            format!(
                "configured (allow={} deny={} sym≤{} files≤{})",
                s.allow_paths,
                s.deny_paths,
                ceiling(s.max_impacted_symbols),
                ceiling(s.max_impacted_files),
            )
        }
        None => "none for this tenant".to_string(),
    };
    println!("  policy      : mode={}  scope={scope}", policy.mode);
    println!("  mode source : {}", provenance.source);
    println!(
        "  tamper state: TAMPER-EVIDENT, NOT TAMPER-PROOF — a local agent can alter policy; a clean report is no evidence that tampering was prevented"
    );
    if provenance.lowered_by_project {
        println!(
            "  {} policy mode was LOWERED from {} to {} by workspace config — refusing healthy status",
            "⚠".red().bold(),
            provenance.user_mode.expect("lowered mode requires a user mode"),
            provenance.effective,
        );
    }

    if policy.enforcing_without_scope {
        println!(
            "  {} enforce mode but NO scope for this tenant — nothing is enforced",
            "⚠".yellow().bold()
        );
    }
}

/// Render the rule-set section from an ALREADY-TAKEN measurement.
///
/// It takes `&RuleSetStatus` rather than re-measuring, and that is a fix, not a
/// refactor: `status()` measured once for the JSON surface and this function
/// measured AGAIN for the text surface, so a text-mode `yupana status` ran the
/// projection twice — two HTTP round-trips per invocation, and two measurements
/// that could disagree. The struct's own doc comment forbids exactly that ("two
/// renderers of one measurement, never two measurements"); the second call had
/// quietly reintroduced it. On a plane that flaps (see `PROJECTION_ATTEMPTS`)
/// the two could genuinely differ, and the one printed was not the one the
/// exit code would have been computed from.
fn print_rule_set_status(config: &YupanaConfig, st: &RuleSetStatus) {
    let local = st.local;

    {
        if !st.graph_enabled {
            println!(
                "  rule set    : {local} local structural rule(s); graph plane OFF \
                 (quipu disabled or not built in — nothing is projected)"
            );
            println!("  rule provenance: local config only");
            return;
        }
        // The SAME path the guard takes (hook::rule_planes::governed_check), so
        // this can never report a rule set the guard would not use.
        match (st.projected, &st.error) {
            // Live projection failed but the DURABLE cache answered: rules ARE
            // in force. Reporting COULD NOT TELL here would be the false
            // direction — it would tell an operator the fleet is unguarded at
            // the moments it is guarded, which is the same class of error as
            // the one the cache was built to fix (aegis-0upyu).
            (_, Some(e)) if st.cache_age_secs.is_some() => {
                let age = st.cache_age_secs.unwrap_or_default();
                println!(
                    "  rule set    : {} — {} projected from the LAST-KNOWN cache, \
                     confirmed {age}s ago ({} structural, {} text) + {local} local",
                    "STALE".yellow().bold(),
                    st.projected.unwrap_or_default(),
                    st.structural,
                    st.text,
                );
                // The consequence, stated the same way the degraded branch
                // states its own: what IS true, then what is unconfirmed.
                println!(
                    "  {} the guard is ENFORCING these rules, but could not confirm them \
                     against {} ({e}) — a retired rule could still be firing",
                    "⚠".yellow().bold(),
                    config.quipu.endpoint,
                );
            }
            (_, Some(e)) => {
                // Fail LOUD, same discipline as the guard's fail-open: a rule
                // set we could not fetch is never reported as a rule set we do
                // not have.
                println!(
                    "  rule set    : {} — could not project from {} after {} attempt(s) ({e}); \
                     {local} local rule(s) only",
                    "COULD NOT TELL".red().bold(),
                    config.quipu.endpoint,
                    st.attempts,
                );
                // Name the CONSEQUENCE, not just the fault. "could not project"
                // is a fact about yupana; "every edit is sailing through" is the
                // fact the reader has to act on.
                println!(
                    "  {} the guard is FAILING OPEN for governed rules until this clears \
                     (exit {EXIT_RULE_SET_DEGRADED})",
                    "⚠".red().bold()
                );
            }
            (Some(0), None) => {
                // Genuinely empty IS sayable — loudly, and only when true.
                println!(
                    "  rule set    : {}",
                    format!("0 projected from quipu + {local} local").yellow()
                );
                println!(
                    "  {} the graph projected NO rules — the plane is armed and empty",
                    "⚠".yellow().bold()
                );
            }
            (Some(total), None) => {
                println!(
                    "  rule set    : {}",
                    format!(
                        "{total} projected from quipu ({} structural, {} text) + {local} local",
                        st.structural, st.text
                    )
                    .green()
                );
                // WHICH rule set. Two hosts that disagree here are enforcing
                // different policy, which is invisible in the counts alone.
                if let Some(d) = &st.digest {
                    println!("  rule digest : {} (unsigned)", &d[..d.len().min(23)]);
                }
            }
            (None, None) => {}
        }
    }

    // The SIGNED resident cache still does not exist, and its
    // absence is still worth reporting — but as what it is: the provenance of
    // the rules above is an unauthenticated fetch, not a verified cache. That is
    // a real caveat and a different sentence from "there are no rules".
    println!(
        "  rule provenance: unsigned live projection — the signed resident cache \
         does not exist yet, so rules are trusted on transport alone"
    );
}
