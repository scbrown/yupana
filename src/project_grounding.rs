//! Grounding projection — the hot-plane half of quipu's entity-grounded
//! predicates (bobbin-tvn; quipu `docs/design/semantic-grounded-edit-policies.md`
//! Design A, sequencing step 2).
//!
//! Yupana cannot run SPARQL per keystroke, so the grounding query runs at
//! PROJECTION time: the authoritative work-item id set is projected into the
//! hot plane alongside the rules, under the existing machinery — same
//! freshness declaration, same durable cache with age-in-verdict. Membership
//! at evaluation time is then O(1) per token, inside the 5 ms budget.
//!
//! Split out of [`crate::project`] for file size, like `project_queries` and
//! `project_exposure`.

use crate::errors::Result;
use crate::grounding::{GroundedRule, GroundingSet};
use crate::project::ProjectionRegistry;
use crate::project_decode::{
    decode_grounded_rules, decode_identifier_pairs, decode_work_item_subjects,
    intersect_grounding_ids,
};
use crate::project_queries::{
    GROUNDED_POLICY_QUERY, GROUNDING_IDENTIFIERS_QUERY, GROUNDING_WORK_ITEMS_QUERY,
};

/// Fetch and decode the entity-grounded rule catalogue over HTTP. A quipu
/// whose catalog predates the vocabulary returns zero rows — an empty
/// catalogue, not an error, so older stores keep projecting cleanly.
pub fn fetch_grounded_rules(endpoint: &str) -> Result<Vec<GroundedRule>> {
    decode_grounded_rules(&crate::project::query(endpoint, GROUNDED_POLICY_QUERY)?)
}

/// Fetch the authoritative work-item id set, or `None` when it cannot be
/// projected — LOUDLY when any grounded rule needs it, because that rule is
/// now unevaluated. Never an error: a failed grounding projection must not
/// disable the planes that did project.
///
/// Skipped entirely (silently `None`) when no grounded rule exists: the set
/// serves the rules, and querying for a set nothing reads would add a network
/// round-trip to every edit for no verdict.
pub fn fetch_grounding_set(endpoint: &str, rules: &[GroundedRule]) -> Option<GroundingSet> {
    if rules.is_empty() {
        return None;
    }
    // Two queries, intersected here: the single join times out in quipu
    // (see GROUNDING_WORK_ITEMS_QUERY). Either failing fails the whole set.
    let set = crate::project::query(endpoint, GROUNDING_WORK_ITEMS_QUERY)
        .and_then(|body| decode_work_item_subjects(&body))
        .and_then(|items| {
            let pairs = decode_identifier_pairs(&crate::project::query(
                endpoint,
                GROUNDING_IDENTIFIERS_QUERY,
            )?)?;
            Ok(GroundingSet::new(intersect_grounding_ids(&items, &pairs)))
        });
    match set {
        Ok(set) => Some(set),
        Err(e) => {
            eprintln!(
                "yupana: grounding set could not be projected ({e}) — \
                 {} grounded rule(s) will be UNEVALUATED, not empty-satisfied",
                rules.len()
            );
            None
        }
    }
}

impl ProjectionRegistry {
    /// The projected entity-grounded rules — same freshness contract as
    /// [`ProjectionRegistry::policies`].
    #[must_use]
    pub fn grounded_rules(&self) -> &[GroundedRule] {
        &self.grounded_rules
    }

    /// The projected work-item id set, or `None` when the grounding
    /// projection is missing/failed (grounded rules are then unevaluated,
    /// loudly — see [`crate::grounding::evaluate`]).
    #[must_use]
    pub fn grounding(&self) -> Option<&GroundingSet> {
        self.grounding.as_ref()
    }

    /// Install grounded rules and their set directly (test/daemon seam),
    /// like `set_policies` / `set_text_rules`.
    pub fn set_grounding(&mut self, rules: Vec<GroundedRule>, set: Option<GroundingSet>) {
        self.grounded_rules = rules;
        self.grounding = set;
    }
}

#[cfg(test)]
mod tests {
    use crate::project_decode::{
        decode_identifier_pairs, decode_work_item_subjects, intersect_grounding_ids,
    };

    fn rows(rows: &serde_json::Value) -> String {
        serde_json::json!({ "results": { "bindings": rows } }).to_string()
    }

    #[test]
    fn only_identifiers_of_work_items_ground() {
        let items = decode_work_item_subjects(&rows(&serde_json::json!([
            {"w": {"value": "aegis:aegis-l50p"}},
            {"w": {"value": "aegis:aegis-mpdmd"}},
        ])))
        .unwrap();
        let pairs = decode_identifier_pairs(&rows(&serde_json::json!([
            {"w": {"value": "aegis:aegis-l50p"}, "id": {"value": "aegis-l50p"}},
            {"w": {"value": "aegis:aegis-mpdmd"}, "id": {"value": "aegis-mpdmd"}},
            // An identifier on something that is NOT a work item must not
            // ground: the intersection is the join's semantics, not a superset.
            {"w": {"value": "aegis:some-host"}, "id": {"value": "host-7"}},
        ])))
        .unwrap();
        let mut ids = intersect_grounding_ids(&items, &pairs);
        ids.sort();
        assert_eq!(ids, ["aegis-l50p", "aegis-mpdmd"]);
    }

    #[test]
    fn a_pair_row_missing_a_binding_is_dropped_not_misattributed() {
        let pairs = decode_identifier_pairs(&rows(&serde_json::json!([
            {"id": {"value": "orphan-1"}},
            {"w": {"value": "aegis:x"}},
        ])))
        .unwrap();
        assert!(pairs.is_empty());
    }

    #[test]
    fn no_work_items_means_an_empty_set_not_every_identifier() {
        let pairs = vec![("aegis:x".to_string(), "x-1".to_string())];
        assert!(intersect_grounding_ids(&[], &pairs).is_empty());
    }
}
