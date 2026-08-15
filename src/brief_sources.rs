//! Where the briefing's facts come from — the suite, reused, never
//! reimplemented: the shared policy projection (scope map, rules), quipu's
//! `/context` pipeline (the Bobbin-integration surface: ranked entities with
//! facts), quipu's `/project` personalized pagerank (the central entities
//! around the item's ground), plain SPARQL for labels/outcomes/provenance,
//! and yupana's own structural graph. Bobbin itself is deliberately NOT
//! called from here — its own hooks inject semantic code context, and
//! nesting them would inject the same context twice (see `crate::brief`).
//!
//! Split from [`crate::brief`] for file size: that module owns what a
//! briefing SAYS; this one owns where each fact CAME FROM.

use std::path::Path;

use crate::brief::{Brief, GroundPath, SimilarItem};
use crate::config::YupanaConfig;

/// Only ids shaped like tracker ids ride into SPARQL literals.
fn sanitized(item: &str) -> String {
    item.chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect()
}

/// Decode a single-variable SELECT into its values.
fn values(sparql_json: &str, var: &str) -> Vec<String> {
    serde_json::from_str::<serde_json::Value>(sparql_json)
        .ok()
        .and_then(|v| v["results"]["bindings"].as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|b| b[var]["value"].as_str().map(str::to_string))
        .collect()
}

/// POST a JSON body to a quipu endpoint, returning the parsed response.
/// Every caller treats `None` as "this section stays empty" — a briefing
/// source failing must never fail the briefing.
fn post(endpoint: &str, route: &str, body: &serde_json::Value) -> Option<serde_json::Value> {
    let text = ureq::post(&format!("{}{route}", endpoint.trim_end_matches('/')))
        .timeout(crate::project::HTTP_TIMEOUT)
        .set("Content-Type", "application/json")
        .send_string(&body.to_string())
        .ok()?
        .into_string()
        .ok()?;
    serde_json::from_str(&text).ok()
}

/// The item's `rdfs:label`, if the graph has one.
pub(crate) fn label_of(endpoint: &str, item: &str) -> Option<String> {
    let query = format!(
        "PREFIX aegis: <http://aegis.gastown.local/ontology/> \
         PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#> \
         SELECT ?label WHERE {{ ?w a aegis:WorkItem ; aegis:identifier \"{}\" ; \
         rdfs:label ?label }}",
        sanitized(item)
    );
    values(
        &crate::project::query(endpoint, &query).unwrap_or_default(),
        "label",
    )
    .into_iter()
    .next()
}

/// Items co-occurring with `item` — sharing a touched entity through the
/// commit-provenance chain (quipu shapes/provenance.ttl).
pub(crate) fn related_items(endpoint: &str, item: &str) -> Vec<String> {
    if ablated("provenance") {
        return Vec::new();
    }
    let id = sanitized(item);
    let query = format!(
        "PREFIX aegis: <http://aegis.gastown.local/ontology/> \
         SELECT DISTINCT ?other WHERE {{ \
         ?c1 aegis:implements ?w ; aegis:modifies ?e . \
         ?w aegis:identifier \"{id}\" . \
         ?c2 aegis:modifies ?e ; aegis:implements ?o . \
         ?o aegis:identifier ?other . \
         FILTER(?other != \"{id}\") }}"
    );
    let mut related = values(
        &crate::project::query(endpoint, &query).unwrap_or_default(),
        "other",
    );
    related.sort();
    related.truncate(8);
    related
}

/// The IRIs of entities the item's prior commits modified — the pagerank
/// seeds for "central around THIS work", and honest ones: they come from the
/// same provenance chain as the observed scope.
fn ground_entity_iris(endpoint: &str, item: &str) -> Vec<String> {
    let query = format!(
        "PREFIX aegis: <http://aegis.gastown.local/ontology/> \
         SELECT DISTINCT ?e WHERE {{ ?c aegis:implements ?w ; aegis:modifies ?e . \
         ?w aegis:identifier \"{}\" }}",
        sanitized(item)
    );
    values(
        &crate::project::query(endpoint, &query).unwrap_or_default(),
        "e",
    )
}

/// SIMILAR work items, via quipu's `/context` pipeline — the same
/// Bobbin-integration surface `unified_search` serves, so similarity here is
/// whatever the store can honestly do (text-ranked always; vector-ranked when
/// the deployment has embeddings). Successful items are the reuse signal, so
/// each carries its declared outcome and its ground from the scope map.
pub(crate) fn similar_items(
    endpoint: &str,
    item: &str,
    query_text: &str,
    scopes: Option<&crate::policy::WorkItemScopes>,
) -> Vec<SimilarItem> {
    // One probe per angle, merged: the full label (exact-phrasing hits, and
    // the semantic path when the store has embeddings/FTS) plus the label's
    // most distinctive terms — because the store's fallback matcher is a
    // whole-substring CONTAINS, under which two items about the same thing
    // in different words never meet.
    if ablated("context") {
        return Vec::new();
    }
    let mut entities: Vec<serde_json::Value> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    for probe in probes(query_text) {
        let Some(response) = post(
            endpoint,
            "/context",
            &serde_json::json!({ "query": probe, "max_entities": 12, "expand_links": true }),
        ) else {
            continue;
        };
        for entity in response["entities"].as_array().cloned().unwrap_or_default() {
            let iri = entity["iri"].as_str().unwrap_or_default().to_string();
            if !seen.contains(&iri) {
                seen.push(iri);
                entities.push(entity);
            }
        }
    }
    let mut similar: Vec<SimilarItem> = Vec::new();
    for entity in &entities {
        let is_work_item = entity["types"].as_array().is_some_and(|t| {
            t.iter().any(|v| {
                v.as_str()
                    .is_some_and(|s| s.ends_with("WorkItem") || s.ends_with("Bead"))
            })
        });
        if !is_work_item {
            continue;
        }
        let iri = entity["iri"].as_str().unwrap_or_default();
        let Some((id, outcome)) = identity_of(endpoint, iri) else {
            continue;
        };
        if id == item {
            continue;
        }
        similar.push(SimilarItem {
            ground: scopes
                .and_then(|s| s.scope_for(&id))
                .map(|scope| scope.allow_paths)
                .unwrap_or_default(),
            id,
            label: entity["label"].as_str().map(str::to_string),
            outcome,
            score: entity["score"].as_f64().unwrap_or(0.0),
        });
        if similar.len() >= 5 {
            break;
        }
    }
    similar
}

/// The probe set for similarity: the full text, then its most distinctive
/// (longest, non-stopword-ish) terms.
fn probes(query_text: &str) -> Vec<String> {
    let mut probes = vec![query_text.to_string()];
    if ablated("term-probes") {
        return probes;
    }
    let mut terms: Vec<&str> = query_text
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|t| t.len() >= 5)
        .collect();
    terms.sort_by_key(|t| std::cmp::Reverse(t.len()));
    terms.dedup();
    probes.extend(terms.into_iter().take(3).map(str::to_string));
    probes
}

/// An entity's tracker id and declared outcome, when it has them.
fn identity_of(endpoint: &str, iri: &str) -> Option<(String, Option<String>)> {
    if iri.contains(['<', '>', '"', ' ']) {
        return None;
    }
    let query = format!(
        "PREFIX aegis: <http://aegis.gastown.local/ontology/> \
         SELECT ?id ?outcome WHERE {{ <{iri}> aegis:identifier ?id . \
         OPTIONAL {{ <{iri}> aegis:outcome ?outcome }} }}"
    );
    let body = crate::project::query(endpoint, &query).ok()?;
    let id = values(&body, "id").into_iter().next()?;
    Some((id, values(&body, "outcome").into_iter().next()))
}

/// CENTRAL entities around the item's ground: quipu's personalized pagerank
/// (`/project`, seeded on the entities the item's commits modified). The
/// graph's own answer to "what does this neighborhood hang off" — reused,
/// not recomputed here.
pub(crate) fn central_entities(endpoint: &str, item: &str) -> Vec<(String, f64)> {
    if ablated("pagerank") {
        return Vec::new();
    }
    let seeds = ground_entity_iris(endpoint, item);
    if seeds.is_empty() {
        return Vec::new();
    }
    let Some(response) = post(
        endpoint,
        "/project",
        &serde_json::json!({ "algorithm": "pagerank", "seeds": seeds, "limit": 5 }),
    ) else {
        return Vec::new();
    };
    response["results"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|r| {
            Some((
                r["entity"].as_str()?.to_string(),
                r["score"].as_f64().unwrap_or(0.0),
            ))
        })
        .collect()
}

/// The structural neighborhood of the ground paths, from this tree's own
/// graph. Best-effort: an unparseable tree yields paths without symbols.
pub(crate) fn ground_of(root: &Path, paths: &[String]) -> Vec<GroundPath> {
    let graph = crate::graph::CodeGraph::build(root).ok();
    paths
        .iter()
        .take(12)
        .map(|path| {
            let mut gp = GroundPath {
                path: path.clone(),
                ..GroundPath::default()
            };
            if let Some(graph) = &graph {
                let symbols = graph.symbols_in(path);
                gp.symbols = symbols.iter().take(10).map(|s| s.name.clone()).collect();
                let mut felt: Vec<String> = symbols
                    .iter()
                    .flat_map(|s| graph.caller_files_of(&s.name))
                    .filter(|f| !paths.contains(f))
                    .collect();
                felt.sort();
                felt.dedup();
                felt.truncate(6);
                gp.felt_from = felt;
            }
            gp
        })
        .collect()
}

/// Whether `feature` is ablated via `$YUPANA_BRIEF_ABLATE` (comma-separated:
/// `term-probes`, `context`, `provenance`, `pagerank`).
///
/// AN EVAL SURFACE, not a config: the F1/ablation harness
/// (`scripts/e2e/eval_f1.py`) removes one retrieval source at a time to
/// measure what each contributes. It rides an env var so the ablated run is
/// the SHIPPED binary with a feature off — never a reimplementation of the
/// retrieval in the eval script, which would measure the copy.
fn ablated(feature: &str) -> bool {
    std::env::var("YUPANA_BRIEF_ABLATE").is_ok_and(|v| v.split(',').any(|f| f.trim() == feature))
}

/// Gather the briefing for the current plate item, or `None` when no item is
/// tracked or the quipu seam is off — a briefing must never guess whose work
/// it is describing.
pub fn gather(config: &YupanaConfig, root: &Path) -> Option<Brief> {
    let item = crate::plate::current()?;
    if !config.quipu.enabled || config.quipu.endpoint.is_empty() {
        return None;
    }
    let endpoint = config.quipu.endpoint.clone();

    let mut registry = crate::project::ProjectionRegistry::new(&endpoint);
    let cache_path = crate::projection_cache::cache_path();
    let now = crate::projection_cache::now_secs();
    let cache_age = match registry.refresh_or_cached(
        cache_path.as_deref(),
        config.quipu.projection_cache_ttl_secs,
        now,
    ) {
        Ok(crate::project::ProjectionSource::Live) => None,
        Ok(crate::project::ProjectionSource::Cache { age_secs, .. }) => Some(age_secs),
        // No projection, no cache: say so in the one honest line, rather than
        // a briefing that silently omits the governed half.
        Err(_) => Some(u64::MAX),
    };

    let paths: Vec<String> = registry
        .work_item_scopes()
        .and_then(|scopes| scopes.scope_for(&item))
        .map(|scope| scope.allow_paths)
        .unwrap_or_default();

    let label = label_of(&endpoint, &item);
    let query_text = label.clone().unwrap_or_else(|| item.clone());

    Some(Brief {
        ground: ground_of(root, &paths),
        related: related_items(&endpoint, &item),
        similar: similar_items(&endpoint, &item, &query_text, registry.work_item_scopes()),
        central: central_entities(&endpoint, &item),
        rules: crate::brief::rules_in_force(&registry),
        posture: crate::brief::posture_line(config),
        item,
        label,
        cache_age,
    })
}
