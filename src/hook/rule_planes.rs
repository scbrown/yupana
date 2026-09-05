//! Local and Quipu-governed rule planes, lifted from `pre_edit` for size.
//! Both evaluate introduced text and return one [`Decision`] per edit.

use super::*;

/// Evaluate the structural rule set against the text this edit introduces.
///
/// Returns `Some(outcome)` when the rules had something to say — a Deny/Notify
/// for a violation under the active mode, or a loud fail-open when the rule set
/// itself does not compile — and `None` when they were clean or inapplicable, so
/// the caller falls through to the capability-scope checks.
///
/// Rules read only the text the edit ADDS (the `Write` content, or the
/// `new_string`s of an `Edit`/`MultiEdit`), so an agent is answerable for what it
/// writes, not for pre-existing debt elsewhere in the file — the same "size the
/// touched region" discipline the blast-radius path uses.
pub(super) fn rule_check(config: &YupanaConfig, input: &HookInput, rel: &str) -> Option<Decision> {
    // Mode::Off disarms the whole guard, rules included.
    if config.policy.mode == Mode::Off {
        return None;
    }
    let rules = &config.policy.rules;
    if rules.is_empty() {
        return None;
    }

    // A rule set that does not compile is misconfigured; fail open LOUDLY rather
    // than quietly under-enforce (the malformed-glob discipline, for rules).
    let errors = crate::rules::errors(rules);
    if !errors.is_empty() {
        let detail: Vec<String> = errors
            .iter()
            .map(|(name, why)| format!("`{name}` ({why})"))
            .collect();
        return Some(
            fail_open(
                input,
                "rules",
                &format!("policy rules do not compile: {}", detail.join(", ")),
            )
            .into(),
        );
    }

    // Rules are language-specific: a file this build cannot parse simply has no
    // applicable rule, so there is nothing to evaluate (not a gap to report).
    let ext = Path::new(rel).extension().and_then(OsStr::to_str)?;
    let language = language_for_extension(ext)?;
    let introduced = introduced_text(input)?;

    let violations = crate::rules::evaluate(rules, &introduced, language, rel);
    if violations.is_empty() {
        return None;
    }
    // FR-3 freshness: a rule verdict declares how current it is. Rules loaded from
    // local config are the authoritative source (not a cache), and evidence is the
    // exact proposed edit — so the verdict is genuinely FRESH. When rules instead
    // come from a projected quipu registry (Phase 4), the registry's own sync state
    // is threaded here so a verdict computed against a stale projection is tagged
    // STALE, never silently fresh.
    let message = rule_verdict_message(&violations, Freshness::Fresh);
    // Record every rule that fired, not just the first: an edit can break
    // several at once, and a record that dropped the rest would send an operator
    // to fix one and hit the next.
    let outcome = decide(config.policy.mode, message);
    let response = crate::trace::Response::of(&outcome);
    let evaluations = evaluations_for(
        violations.iter().map(|v| v.rule.as_str()),
        response,
        |name| {
            rules
                .iter()
                .find(|r| r.name == name)
                .map(|r| (r.class, r.verification_point))
        },
    );
    // Local config is the authoritative source, not a cache, and the evidence
    // is the exact proposed edit — so this verdict is genuinely fresh.
    Some(Decision::evaluated(outcome, evaluations, Freshness::Fresh))
}

/// Build one [`ConstraintEvaluation`] per DISTINCT rule that fired, in stable
/// order, attaching each rule's declared class and verification point via
/// `placement`.
///
/// Sorted and deduplicated for the same reason the `+`-joined field was: an
/// edit can trip one rule at several sites, and a record whose contents varied
/// with evaluation order is one an operator cannot group by.
fn evaluations_for<'a>(
    ids: impl Iterator<Item = &'a str>,
    response: crate::trace::Response,
    placement: impl Fn(
        &str,
    ) -> Option<(
        Option<crate::constraint::ConstraintClass>,
        Option<crate::constraint::VerificationPoint>,
    )>,
) -> Vec<crate::trace::ConstraintEvaluation> {
    let mut seen: Vec<&str> = ids.collect();
    seen.sort_unstable();
    seen.dedup();
    seen.into_iter()
        .map(|name| {
            let (class, point) = placement(name).unwrap_or((None, None));
            crate::trace::ConstraintEvaluation::new(
                name,
                crate::trace::Outcome::Unsatisfied,
                response,
            )
            .placed(class, point)
            // The layer that ACTUALLY evaluated it. Stamped here rather than
            // copied from the policy's own aegis:hostedAtLayer, because a
            // record that echoed the claim would let an overclaim survive the
            // audit by being asserted twice (SARC I6).
            .hosted_at(crate::hosting::YUPANA_HOSTS_AT)
        })
        .collect()
}

/// Combine rule violations into one model-facing verdict and DECLARE its freshness
/// (FR-3). The freshness is the policy registry's currency: `Fresh` for the local
/// config source, or the projection's real sync state once Phase 4 wires it — never
/// a silent assumption of `fresh`.
fn rule_verdict_message(
    violations: &[crate::rules::RuleViolation],
    freshness: Freshness,
) -> String {
    let body = violations
        .iter()
        .map(|v| v.message.clone())
        .collect::<Vec<_>>()
        .join("\n");
    let note = match freshness {
        Freshness::Fresh => {
            "verdict freshness: fresh (evaluated against the exact proposed \
             edit and the loaded policy set)"
        }
        Freshness::Stale => {
            "verdict freshness: STALE — the projected policy registry could not \
             be refreshed from quipu, so this verdict may not reflect the latest governed policy"
        }
        Freshness::Recomputing => {
            "verdict freshness: recomputing — the policy registry is \
             mid-refresh"
        }
    };
    format!("{body}\n({note})")
}

/// Evaluate the governed policies projected from quipu (Phase 4) — both
/// planes, one registry refresh, ONE verdict per edit.
///
/// TEXT plane first (aegis-mqnl's `InternalIdentifierPattern` catalogue):
/// deliberately NOT extension- or language-gated — that gate is the reason the
/// first governed rule never ran, since the identifiers it exists to stop
/// leaked through `.md`/`.yml` files no grammar covers. A block-tier text rule
/// blocks only when the REPO IS PUBLIC, and publicness is asked of the graph
/// itself (the governed policy's own `/policy/check`, three-valued): a repo
/// the graph does not know gets a warning THAT SAYS SO — never a block on a
/// guess, never silence (mqnl's design constraint, verbatim).
///
/// STRUCTURAL plane second (`boundary:"action"` Selector/Predicate policies),
/// language-gated as before; a governed `deny` effect blocks under
/// [`Mode::Enforce`].
///
/// Violations from both planes merge into one message; the edit blocks if ANY
/// violation blocks under the active mode. [`Mode::Advise`] never blocks (the
/// local advise-first ceiling), and a projection that cannot be fetched fails
/// OPEN loudly — governed policy that could not be projected must be visible,
/// never a silent pass.
///
/// The verdict declares the projection's [`Freshness`] — a successful sync is
/// `Fresh`. NB: this fetches per edit; the resident/async cache that makes it
/// cheap (FR-31) is the daemon-side form of `H-PROJECTION` / the hac0 signed
/// cache, and that seam is deliberately untouched here.
#[cfg(feature = "quipu")]
pub(super) fn governed_check(
    config: &YupanaConfig,
    input: &HookInput,
    root: &Path,
    rel: &str,
    scope_plane: &mut Option<super::scope_arm::ScopePlane>,
) -> Option<Decision> {
    use crate::project::RepoExposure;

    if config.policy.mode == Mode::Off || !config.quipu.enabled || config.quipu.endpoint.is_empty()
    {
        return None;
    }
    let introduced = introduced_text(input)?;

    // Project from quipu, falling back to the DURABLE last-known catalogue when
    // the live query cannot complete (aegis-0upyu). Before the cache existed
    // this was a bare `refresh()` whose only failure branch was `fail_open`, and
    // it fired on 5.2% of all pre-edit invocations — 19% on one measured day —
    // because quipu serves /query effectively one at a time while every agent
    // issues one per edit. Loading the graph disabled the guard that reads the
    // graph, so the guard was least available exactly when it was most needed.
    //
    // A cache hit still ENFORCES; it is only STALE, and every verdict below
    // declares that plus the cache's age. Only a projection failure with no
    // servable cache degrades to allow.
    let mut registry = crate::project::ProjectionRegistry::new(&config.quipu.endpoint);
    let cache_path = crate::projection_cache::cache_path();
    let now = crate::projection_cache::now_secs();
    // aegis-x894x2: the resident daemon first when one is expected. `None` means
    // no usable daemon answer and the live path below is unchanged.
    let projected = match crate::hook::daemon_projection::from_daemon(config, &mut registry, now) {
        Some(result) => result,
        None => registry.refresh_or_cached(
            cache_path.as_deref(),
            config.quipu.projection_cache_ttl_secs,
            now,
        ),
    };
    let source = match projected {
        Ok(source) => source,
        Err(reason) => {
            return Some(
                fail_open(
                    input,
                    "projection",
                    &format!("could not project governed policy from quipu: {reason}"),
                )
                .into(),
            );
        }
    };
    // `served_from_cache` is its OWN record kind, not `fail_open`: one enforces
    // last-known policy, the other is the guard not running. Collapsing them is
    // the ambiguity aegis-tv9ri removed from the sibling hook, and it is what
    // would let the aegis-mqnl advise-soak count unguarded edits as clean ones
    // a second time.
    let cache_age = match &source {
        crate::project::ProjectionSource::Live => None,
        crate::project::ProjectionSource::FreshCache { age_secs } => Some(*age_secs),
        crate::project::ProjectionSource::Cache { age_secs, error } => {
            // stderr always, on the same reasoning as every other degradation
            // here: the guard did its job, and the operator still needs to know
            // it did so on a catalogue it could not confirm.
            eprintln!(
                "yupana: governed policy served from CACHE ({age_secs}s old) — \
                 live projection failed: {error}"
            );
            crate::metrics::emit(
                "served_from_cache",
                &[
                    ("age_secs", (*age_secs).into()),
                    ("ttl_secs", config.quipu.projection_cache_ttl_secs.into()),
                    ("reason", error.clone().into()),
                ],
            );
            Some(*age_secs)
        }
    };
    // Hand the scope arm its plane from THIS registry — the scope ladder must
    // never add a projection round-trip of its own (work-scoped-governance:
    // the guard's latency budget is why the plane rides the shared refresh).
    *scope_plane =
        registry
            .work_item_scopes()
            .cloned()
            .map(|scopes| super::scope_arm::ScopePlane {
                scopes,
                // The derived rung's parent map rides the SAME refresh, for the
                // same reason the scope map does: the ladder must not add a
                // projection round-trip of its own to the guard's budget.
                parents: registry.work_item_parents().cloned(),
                cache_age,
            });

    // A projected rule set that does not compile is a broken sync; fail open
    // loudly — both planes, same discipline.
    let rules: Vec<crate::rules::Rule> =
        registry.policies().iter().map(|p| p.rule.clone()).collect();
    let mut errors = crate::rules::errors(&rules);
    errors.extend(crate::textrules::errors(registry.text_rules()));
    if !errors.is_empty() {
        let detail: Vec<String> = errors
            .iter()
            .map(|(name, why)| format!("`{name}` ({why})"))
            .collect();
        return Some(
            fail_open(
                input,
                "projection-rules",
                &format!("projected policies do not compile: {}", detail.join(", ")),
            )
            .into(),
        );
    }

    let mut messages: Vec<String> = Vec::new();
    let mut any_blocking = false;
    let mut structural_violations: Vec<crate::project::ProjectedViolation> = Vec::new();

    // --- TEXT plane: every file, no grammar required ------------------------
    //
    // The governed plane judges the ARTIFACT, so both the path it matches path
    // scopes against and the exposure it grades tiers by come from the repo the
    // edited FILE belongs to — not from `root`, which is the session's cwd.
    // When the two coincide (an agent editing the tree it is standing in) this
    // is identical to the old behaviour; when they diverge it is the difference
    // between grading the edit and grading the session. See
    // [`crate::git::repo_root_containing`] for the measured failure.
    let target = target_file(input, root);
    let target_root = target.as_deref().and_then(crate::git::repo_root_containing);
    let target_rel = match (&target, &target_root) {
        (Some(file), Some(tr)) => relative(file, tr),
        _ => rel.to_string(),
    };
    let text_violations =
        crate::textrules::evaluate(registry.text_rules(), &introduced, &target_rel);
    // Carried onto the `governed` metrics line so the advise-mode soak is
    // ADJUDICABLE. `blocking` alone says an edit WOULD have been denied but not
    // whether that denial would have been right, and the spool records no path
    // (audit.record_paths is off by default), so a reviewer had nothing to
    // judge against. The repo NAME and the exposure verdict are the two facts
    // that decide it, and neither is a path.
    let mut exposure_label = "n/a";
    let mut target_repo: Option<String> = None;
    if !text_violations.is_empty() {
        // Exposure is resolved ONCE per edit, from the graph, via the governed
        // policy itself — so yupana and every other consumer of rule #1 share one
        // definition of "public". Any failure to ask IS the Unknown answer.
        let repo = target_root
            .as_deref()
            .and_then(crate::git::origin_repo_name);
        let exposure = match (&target_root, &repo) {
            (Some(_), Some(repo)) => crate::hook::daemon_projection::exposure_for(config, repo),
            (Some(tr), None) => RepoExposure::Unknown(format!(
                "the tree containing this file ({}) has no `origin` remote, so \
                 its exposure cannot be resolved",
                tr.display()
            )),
            (None, _) => RepoExposure::Unknown(
                "this file is not inside a git work tree, so its exposure cannot \
                 be resolved"
                    .into(),
            ),
        };
        exposure_label = match exposure {
            RepoExposure::Public => "public",
            RepoExposure::Internal => "internal",
            RepoExposure::Unknown(_) => "unknown",
        };
        target_repo = repo;
        let (text_messages, text_blocks) = text_plane(&text_violations, &exposure);
        messages.extend(text_messages);
        any_blocking |= text_blocks;
    }

    // --- STRUCTURAL plane: language-gated, as before ------------------------
    if let Some(language) = Path::new(rel)
        .extension()
        .and_then(OsStr::to_str)
        .and_then(language_for_extension)
    {
        // The mode is passed in rather than applied afterwards because the
        // DECLARED CLASS now decides blocking (SARC §3.1), and only
        // `evaluate_projected` can see which policy produced which violation.
        // `Mode::Advise` stays a ceiling on top of it — an advise-mode
        // deployment never blocks, whatever a hard constraint says, which is
        // what makes staging a new one safe.
        let violations = crate::project::evaluate_projected(
            registry.policies(),
            &introduced,
            language,
            rel,
            config.policy.mode,
        );
        for v in &violations {
            if v.blocking {
                any_blocking = true;
            }
            messages.push(v.message.clone());
        }
        structural_violations = violations;
    }

    for v in super::tripwire_arm::governed_plane(&registry, config, root, &target_rel) {
        any_blocking |= v.blocking;
        messages.push(v.message.clone());
        structural_violations.push(v);
    }

    // --- GROUNDED plane: membership against the projected work-item set ------
    let (grounded_messages, grounded_blocking, grounded_names) =
        super::grounded_plane::grounded_check(&registry, &introduced);
    messages.extend(grounded_messages);
    any_blocking |= grounded_blocking;

    if messages.is_empty() {
        return None;
    }
    // The leverage headline (aegis-0nng): WHICH governed rules fire, by name.
    // Text rules carry names; structural violations ride as a count (their
    // names live only inside composed messages today).
    let repo_label = target_repo.unwrap_or_else(|| "unresolved".to_string());
    let rule_names: Vec<serde_json::Value> = text_violations
        .iter()
        .map(|v| v.rule.clone())
        .chain(grounded_names.iter().cloned())
        .map(serde_json::Value::from)
        .collect();
    crate::metrics::emit(
        "governed",
        &[
            ("rules", rule_names.into()),
            (
                "mode",
                match config.policy.mode {
                    Mode::Off => "off",
                    Mode::Advise => "advise",
                    Mode::Enforce => "enforce",
                }
                .into(),
            ),
            ("structural", (structural_violations.len() as u64).into()),
            ("blocking", any_blocking.into()),
            ("exposure", exposure_label.into()),
            ("repo", repo_label.clone().into()),
            // WHICH catalogue said so. A soak that groups governed firings
            // without this cannot tell a verdict from the current policy set
            // from one computed against a cached catalogue, and those are not
            // equally good evidence about the current policy.
            (
                "policy_source",
                if cache_age.is_some() { "cache" } else { "live" }.into(),
            ),
            ("policy_age_secs", cache_age.unwrap_or(0).into()),
            // WHAT MATCHED (aegis-mqnl) and WHICH FILE (aegis-x894x2), so a block is
            // adjudicable from the record rather than by re-reading the repos. The
            // plain path stays behind `metrics.record_paths`; the digest keys a file
            // 1:1 without exporting it. Rationale in `audit::path_digest`.
            (
                "matched",
                crate::textrules::distinct_matched(&text_violations).into(),
            ),
            ("path_digest", crate::audit::path_digest(rel).into()),
        ],
    );
    let blocks = config.policy.mode == Mode::Enforce && any_blocking;
    let message = super::grounded_plane::rule_verdict_message_from(
        &messages.join("\n"),
        registry.freshness(),
        &source,
        config.quipu.projection_cache_ttl_secs,
    );
    // The same rule names the `governed` line already reports, carried onto the
    // `guard` line too (yupana #77). Without this a governed deny is the one deny
    // an operator has to JOIN two spool lines to attribute — and the join is by
    // timestamp, which is exactly the correlation that failed during the
    // incident that motivated the issue. Structural violations contribute a
    // count, not names (their names live only inside composed messages today),
    // so they are named as such rather than silently omitted.
    let outcome = if blocks {
        Outcome::Deny(message)
    } else {
        Outcome::Notify(format!("yupana (governed, not blocking): {message}"))
    };
    let response = crate::trace::Response::of(&outcome);

    // Text-plane rules have no declared class (the aegis-mqnl catalogue predates
    // the SARC fields and carries its own `enforcementTier` instead), so they
    // record with class absent rather than a class inferred from that tier —
    // the tiers are not the same vocabulary and mapping one onto the other would
    // assert a placement nobody declared.
    let mut evaluations = evaluations_for(
        text_violations
            .iter()
            .map(|v| v.rule.as_str())
            .chain(grounded_names.iter().map(String::as_str)),
        response,
        |_| None,
    );
    // Structural violations now carry their OWN names and declared placement.
    // They used to reach the spool as the literal string "governed-structural"
    // plus a count, so an operator could see that three governed rules fired and
    // not which three — the names lived only inside the composed message. That
    // is the unattributable-record shape the audit field exists to prevent, and
    // it survived inside the very field added to fix it.
    evaluations.extend(evaluations_for(
        structural_violations.iter().map(|v| v.id.as_str()),
        response,
        |name| {
            structural_violations
                .iter()
                .find(|v| v.id == name)
                .map(|v| (v.class, v.verification_point))
        },
    ));
    // The projection's REAL sync state, not an assumption: a verdict computed
    // against a registry that could not be refreshed is stale, and saying so is
    // the whole point of carrying the field.
    let mut decision = Decision::evaluated(outcome, evaluations, registry.freshness());
    decision.governed_context = Some((exposure_label, repo_label));
    Some(decision)
}

/// The absolute path of the file this edit actually writes to, which is the
/// subject the governed plane grades. A relative `file_path` is resolved
/// against the session root — the only thing it can sensibly mean.
#[cfg(feature = "quipu")]
fn target_file(input: &HookInput, root: &Path) -> Option<PathBuf> {
    let raw = input.tool_input.file_path.as_deref()?;
    let path = Path::new(raw);
    Some(if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    })
}

/// The text plane's decision, PURE: which messages, and whether anything
/// blocks, given the violations and the repo's exposure. Separated from
/// [`governed_check`] so the tier x exposure matrix — the part of this whole
/// circuit that must never be wrong in the blocking direction — is testable
/// without a network.
///
/// The matrix (mqnl's seam, verbatim):
///   block tier + Public   -> BLOCKS (this is the leak the rule exists for)
///   block tier + Internal -> warning, downgraded and saying why
///   block tier + Unknown  -> warning that SAYS the repo is unknown — never
///                            block on a guess, never be silent on ignorance
///   warn tier  + anything -> warning
#[cfg(feature = "quipu")]
pub(super) fn text_plane(
    violations: &[crate::textrules::TextViolation],
    exposure: &crate::project::RepoExposure,
) -> (Vec<String>, bool) {
    use crate::project::RepoExposure;
    use crate::textrules::TextTier;

    let mut messages: Vec<String> = Vec::new();
    let mut any_blocking = false;
    for v in violations {
        if v.tier == TextTier::Block && *exposure == RepoExposure::Public {
            any_blocking = true;
        }
        messages.push(v.message.clone());
    }
    match exposure {
        RepoExposure::Public => messages.push(
            "[exposure: this repo has a PUBLIC remote (per the graph), so \
             block-tier rules block]"
                .to_string(),
        ),
        RepoExposure::Internal => messages.push(
            "[exposure: downgraded to a warning — the graph knows this repo and \
             it has no public remote, so the token leaks nowhere; the same edit \
             would BLOCK in a public repo]"
                .to_string(),
        ),
        RepoExposure::Unknown(reason) => messages.push(format!(
            "[exposure: warning, NOT blocking — the repo's exposure is unknown \
             ({reason}), and a governed rule never blocks on a guess. Add this \
             repo's `repo_<name>` entity and remote facts to quipu to pin it.]"
        )),
    }
    (messages, any_blocking)
}
