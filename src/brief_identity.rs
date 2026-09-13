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
    let mut identities = HashMap::new();
    for batch in iris.chunks(BATCH_SIZE) {
        let candidates = batch
            .iter()
            .map(|iri| format!("<{iri}>"))
            .collect::<Vec<_>>()
            .join(" ");
        let query = super::IDENTITY_QUERY.replace("$CANDIDATES", &candidates);
        let Ok(body) = crate::project::query(endpoint, &query) else {
            continue;
        };
        let Ok(response) = serde_json::from_str::<serde_json::Value>(&body) else {
            continue;
        };
        let Some(rows) = response["results"]["bindings"].as_array() else {
            continue;
        };
        for row in rows {
            let value = |key: &str| row[key]["value"].as_str().map(str::to_string);
            let (Some(entity), Some(id)) = (value("entity"), value("id")) else {
                continue;
            };
            if !batch.contains(&entity.as_str()) {
                continue;
            }
            let entry = identities.entry(entity).or_insert((id, None, None));
            // Match the old decoder's first available value for each field,
            // independent of result ordering between different subjects.
            if entry.1.is_none() {
                entry.1 = value("outcome");
            }
            if entry.2.is_none() {
                entry.2 = value("label");
            }
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
