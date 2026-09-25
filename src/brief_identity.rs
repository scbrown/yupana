//! Bounded identity reads for briefing candidates. Missing identities remain
//! absent; a failed batch never fans out into per-entity retry requests.

use std::collections::{BTreeSet, HashMap};

type Identity = (String, Option<String>, Option<String>);
const BATCH_SIZE: usize = 32;

/// Resolve each distinct, safe candidate once while keeping requests bounded.
/// VALUES binds the subject before the optional outcome/label joins.
pub(super) fn fetch<'a>(
    endpoint: &str,
    iris: impl IntoIterator<Item = &'a str>,
) -> HashMap<String, Identity> {
    let iris: Vec<_> = iris
        .into_iter()
        .filter(|iri| valid_iri(iri))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    // `/search` and `/context` name entities by PREFIXED name (`aegis:x`).
    // Sent as `<aegis:x>` that is a different, nonexistent IRI, so every
    // candidate matched nothing (aegis-h9c0no, measured: 0 rows vs 1 for the
    // full IRI). Ask about the full IRI; key the answer by the caller's own
    // spelling, because that is what the caller looks up.
    let by_full: HashMap<String, &str> = iris
        .iter()
        .map(|iri| (crate::sparql_steps::full_iri(iri), *iri))
        .collect();
    // Sorted, so batch composition is deterministic rather than hash order.
    let full: Vec<String> = by_full
        .keys()
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    // One bound pattern per field. The previous single query (identifier plus
    // two OPTIONALs over a VALUES block) took ~4 s on the live store, against
    // ~10 ms per single pattern; a missing batch is simply a candidate with no
    // identity, as before.
    let field = |pattern: &str, var: &str| -> Vec<(String, String)> {
        let mut out = Vec::new();
        for batch in full.chunks(BATCH_SIZE) {
            if let Ok(pairs) = crate::sparql_steps::hop(endpoint, batch, "entity", pattern, var) {
                out.extend(pairs);
            }
        }
        out
    };
    let mut identities: HashMap<String, Identity> = HashMap::new();
    for (entity, id) in field("?entity aegis:identifier ?id", "id") {
        if let Some(requested) = by_full.get(&entity) {
            identities
                .entry((*requested).to_string())
                .or_insert((id, None, None));
        }
    }
    let first_value = |pairs: Vec<(String, String)>| -> HashMap<String, String> {
        let mut first = HashMap::new();
        for (entity, v) in pairs {
            first.entry(entity).or_insert(v);
        }
        first
    };
    let outcomes = first_value(field("?entity aegis:outcome ?outcome", "outcome"));
    let labels = first_value(field(
        "?entity <http://www.w3.org/2000/01/rdf-schema#label> ?label",
        "label",
    ));
    for (full_iri, requested) in &by_full {
        if let Some(entry) = identities.get_mut(*requested) {
            entry.1 = outcomes.get(full_iri).cloned();
            entry.2 = labels.get(full_iri).cloned();
        }
    }
    identities
}

fn valid_iri(iri: &str) -> bool {
    !iri.is_empty()
        && !iri
            .chars()
            .any(|c| c.is_control() || c.is_whitespace() || "<>\"{}|^`\\".contains(c))
}

#[cfg(test)]
#[path = "brief_identity_test.rs"]
mod tests;
