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
        .timeout(crate::project::http_timeout())
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

/// Entities touched by more distinct work items than this are HUBS — the
/// justfile-shaped files everyone's work brushes. Co-occurrence through a
/// hub says "you both work in this repo", not "you work on the same thing",
/// so hub entities contribute nothing to relatedness. Measured by the
/// `hub-entity-trap` probe in `just e2e f1`.
const HUB_DEGREE_CAP: usize = 5;

/// Items co-occurring with `item` — sharing a touched entity through the
/// commit-provenance chain (quipu shapes/provenance.ttl), excluding
/// co-occurrence through hub entities (see [`HUB_DEGREE_CAP`]).
pub(crate) fn related_items(endpoint: &str, item: &str) -> Vec<String> {
    if ablated("provenance") {
        return Vec::new();
    }
    let id = sanitized(item);
    // `(entity, other)` pairs, SELF ROWS INCLUDED, so the per-entity degree
    // count sees every tapper — a hub is a hub whether or not we're one of
    // its five hundred visitors.
    let query = format!(
        "PREFIX aegis: <http://aegis.gastown.local/ontology/> \
         SELECT ?e ?other WHERE {{ \
         ?c1 aegis:implements ?w ; aegis:modifies ?e . \
         ?w aegis:identifier \"{id}\" . \
         ?c2 aegis:modifies ?e ; aegis:implements ?o . \
         ?o aegis:identifier ?other }}"
    );
    let body = crate::project::query(endpoint, &query).unwrap_or_default();
    let mut per_entity: std::collections::BTreeMap<String, std::collections::BTreeSet<String>> =
        std::collections::BTreeMap::new();
    for (entity, other) in pair_values(&body, "e", "other") {
        per_entity.entry(entity).or_default().insert(other);
    }
    let mut related: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for items in per_entity.into_values() {
        if items.len() <= HUB_DEGREE_CAP {
            related.extend(items.into_iter().filter(|other| *other != id));
        }
    }
    let mut related: Vec<String> = related.into_iter().collect();
    related.truncate(8);
    related
}

/// Decode a two-variable SELECT into its value pairs; partial rows dropped.
fn pair_values(sparql_json: &str, a: &str, b: &str) -> Vec<(String, String)> {
    serde_json::from_str::<serde_json::Value>(sparql_json)
        .ok()
        .and_then(|v| v["results"]["bindings"].as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|row| {
            Some((
                row[a]["value"].as_str()?.to_string(),
                row[b]["value"].as_str()?.to_string(),
            ))
        })
        .collect()
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
    related: &[String],
) -> Vec<SimilarItem> {
    // One probe per angle, then CORROBORATION: the full label (exact-phrasing
    // hits, and the semantic path when the store has embeddings/FTS) plus the
    // label's most distinctive terms — because the store's fallback matcher
    // is a whole-substring CONTAINS, under which two items about the same
    // thing in different words never meet. Sources then VOTE: a phrase hit
    // counts 2, each term hit 1, a provenance co-occurrence 2 — and only
    // items with >= 2 votes survive when anything does, because a single
    // shared term ("boundary", "cache") is how a lexical distractor sneaks
    // in wearing a similar item's score. Measured by `just e2e f1`: the
    // threshold trades no recall for the precision the FPs were costing.
    if ablated("context") {
        return Vec::new();
    }
    // Semantic first, because its availability decides the lexical strategy:
    // term probes exist to compensate the store's whole-substring fallback
    // matcher, and on a semantic store they retire — measured by the F1
    // ablations, the collisions they admit there cost more than the recall
    // they add (the same mid-band inseparability that rules out a veto).
    let hits = semantic_hits(endpoint, query_text);
    let mut entities: Vec<serde_json::Value> = Vec::new();
    let mut order: Vec<String> = Vec::new();
    let mut votes: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
    for (index, probe) in probes(query_text, hits.is_empty()).iter().enumerate() {
        let Some(response) = post(
            endpoint,
            "/context",
            &serde_json::json!({ "query": probe, "max_entities": 12, "expand_links": true }),
        ) else {
            continue;
        };
        let weight = if index == 0 { 2 } else { 1 };
        for entity in response["entities"].as_array().cloned().unwrap_or_default() {
            let iri = entity["iri"].as_str().unwrap_or_default().to_string();
            *votes.entry(iri.clone()).or_insert(0) += weight;
            if !order.contains(&iri) {
                order.push(iri);
                entities.push(entity);
            }
        }
    }
    // The semantic source supports (2 votes) the hits standing clearly at
    // the top of ITS OWN ranking for this query — which is how it introduces
    // the paraphrased sibling no lexical probe can see. Self is excluded
    // from setting the scale, or every probe would be its own ceiling.
    let mut semantic: std::collections::HashMap<String, f64> = std::collections::HashMap::new();
    if !hits.is_empty() {
        let self_iri = item_iri(endpoint, item).unwrap_or_default();
        let top = hits
            .iter()
            .filter(|(iri, _)| *iri != self_iri)
            .map(|(_, score)| *score)
            .fold(0.0_f64, f64::max);
        if top >= SEMANTIC_MIN_TOP {
            for (iri, score) in &hits {
                if *iri != self_iri && *score >= top * SEMANTIC_BAND {
                    *votes.entry(iri.clone()).or_insert(0) += 2;
                    if !order.contains(iri) {
                        order.push(iri.clone());
                        entities.push(serde_json::json!({ "iri": iri }));
                    }
                }
                semantic.insert(iri.clone(), *score);
            }
        }
    }
    let mut similar: Vec<(u32, SimilarItem)> = Vec::new();
    for entity in &entities {
        let iri = entity["iri"].as_str().unwrap_or_default();
        let is_work_item = entity["types"].as_array().is_some_and(|t| {
            t.iter().any(|v| {
                v.as_str()
                    .is_some_and(|s| s.ends_with("WorkItem") || s.ends_with("Bead"))
            })
        });
        // Semantic-only candidates carry no type facts; `identity_of` is the
        // filter there — nothing without a tracker id gets through.
        if !is_work_item && !semantic.contains_key(iri) {
            continue;
        }
        let Some((id, outcome, label)) = identity_of(endpoint, iri) else {
            continue;
        };
        if id == item {
            continue;
        }
        let mut vote = votes.get(iri).copied().unwrap_or(0);
        if related.contains(&id) {
            vote += 2;
        }
        let context_score = entity["score"].as_f64().unwrap_or(0.0);
        similar.push((
            vote,
            SimilarItem {
                ground: scopes
                    .and_then(|s| s.scope_for(&id))
                    .map(|scope| scope.allow_paths)
                    .unwrap_or_default(),
                id,
                label: entity["label"].as_str().map(str::to_string).or(label),
                outcome,
                score: context_score.max(semantic.get(iri).copied().unwrap_or(0.0)),
            },
        ));
    }
    // The threshold prunes, never blanks: when nothing is corroborated, the
    // single-vote candidates are still the best available answer.
    if similar.iter().any(|(vote, _)| *vote >= 2) {
        similar.retain(|(vote, _)| *vote >= 2);
    }
    similar.sort_by(|a, b| {
        b.0.cmp(&a.0).then(
            b.1.score
                .partial_cmp(&a.1.score)
                .unwrap_or(std::cmp::Ordering::Equal),
        )
    });
    similar.truncate(5);
    similar.into_iter().map(|(_, item)| item).collect()
}

/// The probe set for similarity: the full text, then — only on a store with
/// no semantic surface (`with_terms`) — its most distinctive (longest,
/// non-stopword-ish) terms.
fn probes(query_text: &str, with_terms: bool) -> Vec<String> {
    let mut probes = vec![query_text.to_string()];
    if !with_terms || ablated("term-probes") {
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

/// An entity's tracker id, declared outcome, and label, when it has them.
/// The id requirement doubles as the work-item filter: commits, classes and
/// code entities carry no `aegis:identifier` and resolve to `None`.
fn identity_of(endpoint: &str, iri: &str) -> Option<(String, Option<String>, Option<String>)> {
    if iri.contains(['<', '>', '"', ' ']) {
        return None;
    }
    let query = format!(
        "PREFIX aegis: <http://aegis.gastown.local/ontology/> \
         PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#> \
         SELECT ?id ?outcome ?label WHERE {{ <{iri}> aegis:identifier ?id . \
         OPTIONAL {{ <{iri}> aegis:outcome ?outcome }} \
         OPTIONAL {{ <{iri}> rdfs:label ?label }} }}"
    );
    let body = crate::project::query(endpoint, &query).ok()?;
    let id = values(&body, "id").into_iter().next()?;
    Some((
        id,
        values(&body, "outcome").into_iter().next(),
        values(&body, "label").into_iter().next(),
    ))
}

/// The semantic scale is QUERY-RELATIVE, not absolute: on quipu's `/search`
/// surface (bare query label against entity text with type/provenance
/// boilerplate) even self-similarity ranges ~0.2–0.75 per query, so a fixed
/// cosine floor misfires both ways — measured on the eval corpus before this
/// design. A hit SUPPORTS (2 votes) when it scores within
/// [`SEMANTIC_BAND`] of the query's top non-self hit and that top clears
/// [`SEMANTIC_MIN_TOP`] (below it, "nearest" is just the least-far noise).
/// There is deliberately NO veto: mid-band true and false positives measure
/// indistinguishably on this surface (0.35 vs 0.38), so a low semantic
/// score is not evidence against a lexically-corroborated candidate.
const SEMANTIC_MIN_TOP: f64 = 0.40;

/// Support band: a hit within this fraction of the top non-self score.
const SEMANTIC_BAND: f64 = 0.8;

/// The probe item's own IRI, so self-similarity never sets the scale.
fn item_iri(endpoint: &str, item: &str) -> Option<String> {
    let query = format!(
        "PREFIX aegis: <http://aegis.gastown.local/ontology/> \
         SELECT ?w WHERE {{ ?w aegis:identifier \"{}\" }}",
        sanitized(item)
    );
    values(&crate::project::query(endpoint, &query).ok()?, "w")
        .into_iter()
        .next()
}

/// SEMANTIC neighbors of the query text, via quipu `/search` — the endpoint
/// embeds the query server-side when the store has an embedding provider,
/// and errors when it does not, which lands here as an empty list: the
/// semantic source simply contributes no votes on a lexical-only store.
fn semantic_hits(endpoint: &str, query_text: &str) -> Vec<(String, f64)> {
    if ablated("semantic") {
        return Vec::new();
    }
    let Some(response) = post(
        endpoint,
        "/search",
        &serde_json::json!({ "query": query_text, "limit": 10 }),
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
/// `term-probes`, `context`, `provenance`, `pagerank`, `semantic`).
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
        Ok(crate::project::ProjectionSource::FreshCache { age_secs }) => Some(age_secs),
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

    let related = related_items(&endpoint, &item);
    Some(Brief {
        ground: ground_of(root, &paths),
        similar: similar_items(
            &endpoint,
            &item,
            &query_text,
            registry.work_item_scopes(),
            &related,
        ),
        related,
        central: central_entities(&endpoint, &item),
        rules: crate::brief::rules_in_force(&registry),
        posture: crate::brief::posture_line(config),
        item,
        label,
        cache_age,
    })
}
