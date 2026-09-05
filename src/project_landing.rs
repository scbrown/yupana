//! Governed landing policy, projected from Quipu.
//!
//! The rule is DATA (`docs/design/landing-policy.md`): who may land on a
//! repository's protected ref is a fact on the repository node, and extending it
//! to a second repository is a graph write rather than a deploy.
//!
//! This module holds the **owner-of predicate** — the one genuinely new piece of
//! predicate vocabulary the landing policy needs. Every other governed plane
//! evaluates evidence found in the command line or read locally
//! (`/proc/meminfo`, a filesystem). This one's evidence is a fact resolved from
//! the graph: the target repository's `aegis:owned_by`, compared against the
//! acting agent.
//!
//! ## Three values, and why none of them may be folded into another
//!
//! [`LandingAuthority`] is `Governed` / `Ungoverned` / `Unknown`, mirroring
//! [`crate::project_exposure::RepoExposure`]. The two collapses are both
//! tempting and both wrong in a different direction:
//!
//! * folding `Ungoverned` into `Unknown` refuses every landing on every
//!   repository the moment quipu blinks — which is how a guard gets removed
//!   rather than fixed;
//! * folding `Unknown` into `Ungoverned` makes an unreachable graph the bypass.
//!
//! ## Repo identity is resolved by EXACT match, never semantically
//!
//! A landing is refused on the strength of this lookup, so it binds exact
//! strings: `rdfs:label` and `skos:altLabel`, over both the bare name and the
//! `repo_`-prefixed convention. The prefixed candidate is not a guess — it is
//! this ontology's naming convention, and it is load-bearing: `aegis:repo_yupana`
//! carries the label `repo_yupana` and *no* `yupana` alias, so a lookup that
//! tried only the bare name would find nothing and report a governed repository
//! as ungoverned.

use serde::{Deserialize, Serialize};

use crate::errors::{Error, Result};

/// The per-repository landing rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LandingRule {
    /// Only the declared owner may land, and the owner must cite a work item.
    SingleWriter,
    /// Any agent may land, but must cite a work item.
    AnyOwnerWithBead,
}

impl LandingRule {
    /// Parse quipu's lexical form.
    ///
    /// `None` for an unrecognised value, which the caller turns into a
    /// projection ERROR — never a silent default. Defaulting an unknown rule to
    /// the permissive one would silently un-govern a repository whose owner
    /// declared something this build does not understand; defaulting it to the
    /// strict one would enforce a rule nobody wrote.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "single-writer" => Some(Self::SingleWriter),
            "any-owner-with-bead" => Some(Self::AnyOwnerWithBead),
            _ => None,
        }
    }

    /// The wire form, shared with the verdict record.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SingleWriter => "single-writer",
            Self::AnyOwnerWithBead => "any-owner-with-bead",
        }
    }

    /// Whether this rule restricts landing to the declared owner.
    #[must_use]
    pub fn owner_only(self) -> bool {
        matches!(self, Self::SingleWriter)
    }
}

/// The protected ref assumed when a governed repository declares none.
///
/// A default, and recorded as one: [`RepoLanding::protected_refs_declared`] says
/// whether the graph stated it, so a verdict can never present this assumption
/// as a declared fact.
pub const DEFAULT_PROTECTED_REF: &str = "main";

/// The landing facts declared for one repository.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoLanding {
    /// The resolved entity IRI.
    pub repo_iri: String,
    /// The name the lookup matched on.
    pub matched_name: String,
    /// The declared owner's agent name, without the ontology prefix. `None`
    /// when the repository declares a landing rule but no owner — which under
    /// an owner-only rule is a REFUSAL, not a pass: a rule naming an owner
    /// nobody recorded cannot be satisfied by anyone.
    pub owner: Option<String>,
    /// The declared rule.
    pub rule: LandingRule,
    /// The refs this rule protects.
    pub protected_refs: Vec<String>,
    /// Whether the graph declared those refs, or they are
    /// [`DEFAULT_PROTECTED_REF`].
    pub protected_refs_declared: bool,
    /// `aegis:ownershipState`, when declared — `RULED` marks an ownership a
    /// human settled rather than a projection inferred.
    pub ownership_state: Option<String>,
    /// Every other name this repository answers to (`rdfs:label`,
    /// `skos:altLabel`). Defaulted so a cache written before this plane existed
    /// still loads, restoring an empty alias set honestly.
    #[serde(default)]
    pub aliases: Vec<String>,
}

impl RepoLanding {
    /// Whether `git_ref` is one of the protected refs. Compares the short name,
    /// so `refs/heads/main` and `main` are the same ref.
    #[must_use]
    pub fn protects(&self, git_ref: &str) -> bool {
        let short = git_ref.rsplit('/').next().unwrap_or(git_ref);
        self.protected_refs
            .iter()
            .any(|r| r == git_ref || r.rsplit('/').next().unwrap_or(r) == short)
    }

    /// Whether `agent` is the declared owner. `false` when no owner is declared
    /// — see [`RepoLanding::owner`].
    #[must_use]
    pub fn is_owner(&self, agent: &str) -> bool {
        self.owner.as_deref().is_some_and(|o| o == agent)
    }
}

/// What the graph could tell us about a repository's landing rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LandingAuthority {
    /// The graph declares a landing rule for this repository.
    Governed(Box<RepoLanding>),
    /// The graph answered and this repository declares no landing rule. Allow.
    Ungoverned {
        /// The name that was looked up, for the record.
        name: String,
    },
    /// The graph could not be asked, or answered unusably. Carries the reason,
    /// because "refused" and "refused because the projection timed out" are
    /// different findings and only the second tells an operator what to fix.
    Unknown(String),
}

/// The catalogue query: every repository that declares a landing rule.
///
/// Projected as a set rather than looked up per command, so it rides the same
/// refresh + durable cache cycle as every other governed plane and a landing
/// never pays for a live round trip.
pub const LANDING_POLICY_QUERY: &str = "\
PREFIX aegis: <http://aegis.gastown.local/ontology/>
PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>
PREFIX skos: <http://www.w3.org/2004/02/skos/core#>
SELECT ?repo ?label ?alt ?owner ?rule ?protectedRef ?state WHERE {
  ?repo a aegis:GitRepo ;
        aegis:landingPolicy ?rule .
  OPTIONAL { ?repo rdfs:label ?label }
  OPTIONAL { ?repo skos:altLabel ?alt }
  OPTIONAL { ?repo aegis:owned_by ?owner }
  OPTIONAL { ?repo aegis:protectedRef ?protectedRef }
  OPTIONAL { ?repo aegis:ownershipState ?state }
}";

/// Strip the ontology prefix from an IRI or prefixed name, leaving the local
/// part. `aegis:grant` and `http://…/ontology/grant` both give `grant`.
fn local_name(value: &str) -> String {
    value
        .rsplit(['/', '#', ':'])
        .next()
        .unwrap_or(value)
        .to_string()
}

/// Fetch the governed landing catalogue.
pub fn fetch_landing_policies(endpoint: &str) -> Result<Vec<RepoLanding>> {
    decode_landing_policies(&crate::project::query(endpoint, LANDING_POLICY_QUERY)?)
}

/// Decode Quipu's W3C SPARQL-results response into the landing catalogue.
///
/// `alt` and `protectedRef` are genuinely multi-valued: N aliases arrive as N
/// rows and are ACCUMULATED. A scalar field disagreeing across one repository's
/// rows is a conflicting declaration and refuses the whole projection rather
/// than resolving on row order — a landing guard that silently picks one of two
/// declared owners is worse than one that says it cannot tell.
pub fn decode_landing_policies(body: &str) -> Result<Vec<RepoLanding>> {
    let value: serde_json::Value = serde_json::from_str(body)
        .map_err(|e| Error::Projection(format!("landing-policy results are not JSON: {e}")))?;
    let rows = crate::project_decode::rows_of(&value)?;

    let mut order: Vec<String> = Vec::new();
    let mut acc: std::collections::HashMap<String, RepoLanding> = std::collections::HashMap::new();

    for (i, row) in rows.iter().enumerate() {
        let get = |key: &str| crate::project_decode::binding_value(row, key);
        let iri = get("repo")
            .ok_or_else(|| Error::Projection(format!("landing-policy row {i}: missing `repo`")))?;
        let raw_rule = get("rule").ok_or_else(|| {
            Error::Projection(format!("landing-policy row {i}: missing `landingPolicy`"))
        })?;
        let rule = LandingRule::parse(&raw_rule).ok_or_else(|| {
            Error::Projection(format!(
                "repo `{iri}` declares landing policy `{raw_rule}`, which this build does not \
                 understand; refusing the projection rather than guessing a rule"
            ))
        })?;
        let owner = get("owner").map(|o| local_name(&o));
        let state = get("state");

        if !order.contains(&iri) {
            order.push(iri.clone());
        }
        let entry = acc.entry(iri.clone()).or_insert_with(|| RepoLanding {
            repo_iri: iri.clone(),
            matched_name: local_name(&iri),
            owner: owner.clone(),
            rule,
            protected_refs: Vec::new(),
            protected_refs_declared: false,
            ownership_state: state.clone(),
            aliases: Vec::new(),
        });

        conflict_check(&entry.repo_iri, "landingPolicy", entry.rule == rule)?;
        conflict_check(
            &entry.repo_iri,
            "owned_by",
            owner.is_none() || entry.owner.is_none() || entry.owner == owner,
        )?;
        if entry.owner.is_none() {
            entry.owner = owner;
        }
        if entry.ownership_state.is_none() {
            entry.ownership_state = state;
        }
        // Aliases and protected refs accumulate.
        for (key, sink) in [("label", 0u8), ("alt", 0), ("protectedRef", 1)] {
            let Some(v) = get(key) else { continue };
            if sink == 1 {
                if !entry.protected_refs.contains(&v) {
                    entry.protected_refs.push(v);
                }
                entry.protected_refs_declared = true;
            } else {
                let name = local_name(&v);
                if !entry.aliases_contains(&name) {
                    entry.push_alias(name);
                }
            }
        }
    }

    let mut out: Vec<RepoLanding> = order
        .into_iter()
        .filter_map(|iri| acc.remove(&iri))
        .collect();
    for repo in &mut out {
        if repo.protected_refs.is_empty() {
            repo.protected_refs.push(DEFAULT_PROTECTED_REF.to_string());
            repo.protected_refs_declared = false;
        }
    }
    Ok(out)
}

fn conflict_check(iri: &str, field: &str, consistent: bool) -> Result<()> {
    if consistent {
        return Ok(());
    }
    Err(Error::Projection(format!(
        "repo `{iri}` declares conflicting `{field}` values across rows; refusing the projection \
         rather than resolving on row order"
    )))
}

/// Name matching, kept beside the accumulator that fills the alias set.
impl RepoLanding {
    fn aliases_contains(&self, name: &str) -> bool {
        self.matched_name == name || self.aliases.iter().any(|a| a == name)
    }
    fn push_alias(&mut self, name: String) {
        self.aliases.push(name);
    }
    /// Whether `name` identifies this repository, by IRI local part, label or
    /// alias — over both the bare form and the `repo_` convention.
    #[must_use]
    pub fn answers_to(&self, name: &str) -> bool {
        let prefixed = format!("repo_{name}");
        [name, prefixed.as_str()]
            .iter()
            .any(|candidate| self.aliases_contains(candidate))
    }
}

/// Resolve one repository name against a projected catalogue.
///
/// The catalogue holds only repositories that DECLARE a rule, so a name absent
/// from it is `Ungoverned` — provided the catalogue itself was projected. The
/// caller supplies that distinction; this function cannot invent it.
#[must_use]
pub fn resolve(catalogue: &[RepoLanding], name: &str) -> LandingAuthority {
    match catalogue.iter().find(|r| r.answers_to(name)) {
        Some(repo) => LandingAuthority::Governed(Box::new(repo.clone())),
        None => LandingAuthority::Ungoverned {
            name: name.to_string(),
        },
    }
}

impl crate::project::ProjectionRegistry {
    /// The governed landing catalogue, or `None` when this registry cannot
    /// speak about landings at all.
    ///
    /// The `Option` carries the distinction the guard turns on: `None` is
    /// "never asked, or restored from a cache predating this plane" and
    /// resolves as [`LandingAuthority::Unknown`]; `Some(&[])` is "asked, and no
    /// repository declares a rule", which resolves as
    /// [`LandingAuthority::Ungoverned`] and allows.
    #[must_use]
    pub fn landing_policies(&self) -> Option<&[RepoLanding]> {
        self.landing_policies.as_deref()
    }
}

#[cfg(test)]
#[allow(non_snake_case)]
mod tests {
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
}
