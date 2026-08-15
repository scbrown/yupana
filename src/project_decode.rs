//! The projection DECODE — SPARQL results in, [`Rule`]s out.
//!
//! Split from [`crate::project`] (which owns the registry, the network fetch
//! and evaluation) purely for size, but the seam is a real one: everything here
//! is a pure function over a canned JSON body, so the whole decode is testable
//! without a quipu.
//!
//! The two rules this module exists to hold, both learned from live incidents
//! and both restated at their call sites:
//!
//! 1. **An unrecognised value is an error, never a default.** A dropped rule is
//!    a guard that reports clean while enforcing nothing.
//! 2. **One entity is one rule.** SPARQL returns the cross product of the
//!    OPTIONALs, so a policy with two labels arrives as two rows.

use crate::constraint::{ConstraintClass, VerificationPoint};
use crate::errors::{Error, Result};
use crate::project::ProjectedPolicy;
use crate::rules::{MatchType, Rule};
use crate::textrules::{TextRule, TextTier};

/// Decode the [`TEXT_POLICY_QUERY`] result into text rules. Same contract as
/// [`decode_policies`]: a row missing a required binding, or carrying a tier
/// this build does not recognise, is an [`Error::Projection`] — never a
/// silently dropped rule.
pub fn decode_text_rules(sparql_json: &str) -> Result<Vec<TextRule>> {
    let value: serde_json::Value = serde_json::from_str(sparql_json)
        .map_err(|e| Error::Projection(format!("results are not JSON: {e}")))?;
    let bindings = rows_of(&value)?;

    let mut out = Vec::with_capacity(bindings.len());
    for (i, binding) in bindings.iter().enumerate() {
        let get = |key: &str| -> Option<String> { binding_value(binding, key) };
        let required = |key: &str| -> Result<String> {
            get(key).ok_or_else(|| {
                Error::Projection(format!(
                    "text-rule row {i}: missing required binding `{key}`"
                ))
            })
        };
        let tier = match required("tier")?.as_str() {
            "block" => TextTier::Block,
            "warn" => TextTier::Warn,
            other => {
                // An unrecognised tier blocks nothing silently and allows
                // nothing silently — it is a projection error the guard
                // surfaces as a loud fail-open (the conservative reading of a
                // governed decision this build cannot interpret).
                return Err(Error::Projection(format!(
                    "text-rule row {i}: unknown enforcementTier `{other}`"
                )));
            }
        };
        // The IRI tail is the stable rule name; verdicts cite it.
        let iri = required("s")?;
        let name = iri.rsplit('/').next().unwrap_or(&iri).to_string();
        let rule = TextRule {
            name,
            label: get("label"),
            pattern: required("regex")?,
            tier,
            class: get("class"),
            exempt_path_regex: get("exempt"),
            rationale: get("rationale"),
        };

        // ONE ENTITY IS ONE RULE. The query carries four OPTIONALs, and SPARQL
        // returns the CROSS PRODUCT of their bindings — so an entity with two
        // `rdfs:comment` values comes back as two rows and became two identical
        // rules. Measured on the live catalogue: 7 pattern entities projected as
        // 11 rules, 4 of them duplicates, purely because somebody had added a
        // second rationale to four of them.
        //
        // The cost lands on the model, twice over: the same violation is
        // reported twice for one edit, and the two copies carry DIFFERENT
        // rationales — for one of them, the second comment explained an
        // exemption, so the advisory argued with itself. An agent reading a
        // guard that contradicts itself learns to discount the guard.
        //
        // Multi-valued rdfs:comment is perfectly legal RDF and the catalogue is
        // right to have it. Collapsing it belongs HERE, in the decoder, not in a
        // convention nobody can enforce on graph authors.
        match out.iter_mut().find(|r: &&mut TextRule| r.name == rule.name) {
            None => out.push(rule),
            Some(existing) => {
                // A REQUIRED field that disagrees is not a duplicate to merge —
                // it is two different rules wearing one name, and picking either
                // one silently is how a guard enforces something nobody wrote.
                // Refuse, loudly, the same as an unknown tier.
                if existing.pattern != rule.pattern || existing.tier != rule.tier {
                    return Err(Error::Projection(format!(
                        "text-rule `{}` has conflicting definitions in the graph \
                         (regex or enforcementTier differ across its rows) — \
                         refusing to guess which one governs",
                        rule.name
                    )));
                }
                // Optionals: keep every DISTINCT value rather than picking one.
                // Dropping a rationale would lose the author's reasoning, and
                // choosing between them by row order would be arbitrary and
                // unstable across graph writes.
                merge_optional(&mut existing.rationale, rule.rationale);
                merge_optional(&mut existing.label, rule.label);
                merge_optional(&mut existing.class, rule.class);
                merge_optional(&mut existing.exempt_path_regex, rule.exempt_path_regex);
            }
        }
    }
    Ok(out)
}

/// Fold an additional optional binding into one a previous row already supplied.
///
/// Distinct values are joined rather than replaced: these are explanatory
/// strings, and the whole reason a second one exists is that somebody had more
/// to say. Identical values collapse, so the common case stays clean.
fn merge_optional(existing: &mut Option<String>, incoming: Option<String>) {
    let Some(add) = incoming else { return };
    match existing {
        None => *existing = Some(add),
        Some(have) => {
            if !have.split(" — ").any(|part| part == add) {
                have.push_str(" — ");
                have.push_str(&add);
            }
        }
    }
}

/// The `results.bindings` array of a W3C SPARQL-results body, or a projection
/// error naming what was malformed.
fn rows_of(value: &serde_json::Value) -> Result<&Vec<serde_json::Value>> {
    value
        .get("results")
        .and_then(|r| r.get("bindings"))
        .and_then(|b| b.as_array())
        .ok_or_else(|| Error::Projection("results have no `results.bindings` array".to_string()))
}

/// One binding's `.value` string, if present.
fn binding_value(binding: &serde_json::Value, key: &str) -> Option<String> {
    binding
        .get(key)
        .and_then(|v| v.get("value"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

/// Decode a W3C `application/sparql-results+json` body (the result of
/// [`POLICY_QUERY`]) into projected policies.
///
/// Pure and testable without a live quipu. A row missing a required binding
/// (`language`/`query`/`pattern`/`matchType`) is a malformed projection and is an
/// [`Error::Projection`] — never silently dropped, so a broken sync cannot look
/// like "quipu has no policies".
pub fn decode_policies(sparql_json: &str) -> Result<Vec<ProjectedPolicy>> {
    let value: serde_json::Value = serde_json::from_str(sparql_json)
        .map_err(|e| Error::Projection(format!("results are not JSON: {e}")))?;
    let bindings = value
        .get("results")
        .and_then(|r| r.get("bindings"))
        .and_then(|b| b.as_array())
        .ok_or_else(|| Error::Projection("results have no `results.bindings` array".to_string()))?;

    let mut out: Vec<ProjectedPolicy> = Vec::with_capacity(bindings.len());
    // Identities already emitted, parallel to `out` — see the collapse below.
    let mut seen: Vec<String> = Vec::with_capacity(bindings.len());
    for (i, binding) in bindings.iter().enumerate() {
        let required = |key: &str| -> Result<String> {
            binding
                .get(key)
                .and_then(|v| v.get("value"))
                .and_then(|v| v.as_str())
                .map(str::to_string)
                .ok_or_else(|| {
                    Error::Projection(format!("row {i}: missing required binding `{key}`"))
                })
        };
        let optional = |key: &str| -> Option<String> {
            binding
                .get(key)
                .and_then(|v| v.get("value"))
                .and_then(|v| v.as_str())
                .map(str::to_string)
        };

        let match_type = match required("matchType")?.as_str() {
            "must-match" => MatchType::MustMatch,
            "must-not-match" => MatchType::MustNotMatch,
            "must-exist" => MatchType::MustExist,
            other => {
                return Err(Error::Projection(format!(
                    "row {i}: unknown matchType `{other}`"
                )))
            }
        };
        let language = required("language")?;
        let query = required("query")?;
        let pattern = required("pattern")?;
        // A policy with no label still needs a stable name for its verdicts.
        let name = optional("name").unwrap_or_else(|| format!("quipu-policy-{i}"));
        let effect = optional("effect").unwrap_or_else(|| "warn".to_string());

        // SARC metadata. Absent is fine — a quipu whose catalog predates
        // Q-SARC-CLASS projects policies with no class, and those behave exactly
        // as they did before the field existed. An UNRECOGNISED value is not
        // fine: defaulting it to `soft` would silently downgrade a hard
        // constraint, and defaulting it to `hard` would block on a typo, so it
        // is a projection error like an unknown `matchType`.
        let class = match optional("constraintClass") {
            None => None,
            Some(s) => Some(ConstraintClass::parse(&s).ok_or_else(|| {
                Error::Projection(format!("row {i}: unknown constraintClass `{s}`"))
            })?),
        };
        let verification_point = match optional("verificationPoint") {
            None => None,
            Some(s) => Some(VerificationPoint::parse(&s).ok_or_else(|| {
                Error::Projection(format!("row {i}: unknown verificationPoint `{s}`"))
            })?),
        };
        let latency_budget_ms = optional("latencyBudgetMs").and_then(|s| s.parse::<u64>().ok());
        // The CLAIM about where this is enforced (I6). Unrecognised is an error
        // for the same reason an unknown class is: the only value quipu's
        // vocabulary deliberately omits is "prompt", and silently dropping it to
        // `None` would turn the one claim I6 forbids into no claim at all.
        let hosted_at_layer = match optional("hostedAtLayer") {
            None => None,
            Some(s) => Some(crate::hosting::HostingLayer::parse(&s).ok_or_else(|| {
                Error::Projection(format!("row {i}: unknown hostedAtLayer `{s}`"))
            })?),
        };

        let policy = ProjectedPolicy {
            rule: Rule {
                name,
                language,
                query,
                gate: optional("gate"),
                match_type,
                pattern,
                applies_to: Vec::new(),
                message: None,
                class,
                verification_point,
                backoff_formula: optional("backoffFormula"),
            },
            effect,
            latency_budget_ms,
            hosted_at_layer,
        };

        // ONE POLICY IS ONE RULE — the same collapse [`decode_text_rules`]
        // performs, and for the same measured reason: SPARQL returns the CROSS
        // PRODUCT of the OPTIONALs, so an entity carrying two `rdfs:label`s came
        // back as two rows and became two identical rules. That already cost the
        // text catalogue 4 duplicate rules out of 11, each reported twice to the
        // model with different rationales.
        //
        // This decoder was exposed to it the whole time and the SARC fields make
        // it likelier, not less: three more OPTIONALs, each a fresh multiplier.
        // Identity is the policy IRI, not the label — an unlabelled policy falls
        // back to a row-indexed name, so keying on the name would give every row
        // a distinct identity and collapse nothing at all.
        let iri = optional("policy");
        let key = iri.clone().unwrap_or_else(|| policy.rule.name.clone());
        match seen.iter().position(|k| *k == key) {
            None => {
                seen.push(key);
                out.push(policy);
            }
            Some(at) => {
                // A DISAGREEING required field is not a duplicate to merge; it is
                // two different policies wearing one identity. Refuse rather than
                // pick, exactly as the text decoder does.
                let existing = &out[at];
                if existing.rule.pattern != policy.rule.pattern
                    || existing.rule.match_type != policy.rule.match_type
                    || existing.rule.class != policy.rule.class
                    || existing.rule.verification_point != policy.rule.verification_point
                    || existing.effect != policy.effect
                {
                    return Err(Error::Projection(format!(
                        "policy `{key}` has conflicting definitions in the graph \
                         (pattern, matchType, constraintClass, verificationPoint \
                         or effect differ across its rows) — refusing to guess \
                         which one governs"
                    )));
                }
            }
        }
    }
    Ok(out)
}

/// Decode the entity-grounded rule catalogue (bobbin-tvn) from
/// `application/sparql-results+json`.
pub fn decode_grounded_rules(sparql_json: &str) -> Result<Vec<crate::grounding::GroundedRule>> {
    let value: serde_json::Value = serde_json::from_str(sparql_json)
        .map_err(|e| Error::Projection(format!("results are not JSON: {e}")))?;
    let bindings = rows_of(&value)?;

    let mut out = Vec::with_capacity(bindings.len());
    for (i, binding) in bindings.iter().enumerate() {
        let get = |key: &str| -> Option<String> { binding_value(binding, key) };
        let match_type = match get("matchType").as_deref() {
            Some("must-ground") => crate::grounding::GroundMatch::MustGround,
            Some("must-not-ground") => crate::grounding::GroundMatch::MustNotGround,
            other => {
                // A grounded decision this build cannot interpret is a
                // projection error, surfaced loudly — never a silent skip.
                return Err(Error::Projection(format!(
                    "grounded-rule row {i}: unknown matchType {other:?}"
                )));
            }
        };
        let tier = match get("tier").as_deref() {
            Some("block") => crate::textrules::TextTier::Block,
            // Undeclared severity advises — the conservative direction in the
            // blocking sense: nothing hard-denies without an explicit tier.
            None | Some("warn") => crate::textrules::TextTier::Warn,
            Some(other) => {
                return Err(Error::Projection(format!(
                    "grounded-rule row {i}: unknown enforcementTier `{other}`"
                )));
            }
        };
        let iri = get("pred").ok_or_else(|| {
            Error::Projection(format!("grounded-rule row {i}: missing binding `pred`"))
        })?;
        let name = get("name")
            .unwrap_or_else(|| iri.rsplit('/').next().unwrap_or(&iri).to_string());
        out.push(crate::grounding::GroundedRule {
            name,
            label: get("label"),
            match_type,
            tier,
            rationale: get("rationale"),
        });
    }
    Ok(out)
}

/// Decode the projected work-item id set (the grounding query's rows).
pub fn decode_grounding_ids(sparql_json: &str) -> Result<Vec<String>> {
    let value: serde_json::Value = serde_json::from_str(sparql_json)
        .map_err(|e| Error::Projection(format!("results are not JSON: {e}")))?;
    Ok(rows_of(&value)?
        .iter()
        .filter_map(|b| binding_value(b, "id"))
        .collect())
}
