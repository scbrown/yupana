//! Walk the provenance chain in SINGLE-PATTERN steps, joining client-side.
//!
//! quipu answers one bound pattern in milliseconds and a multi-pattern BGP
//! over the same data at its 10 s query deadline: past the read-model cap it
//! routes a 2+ pattern BGP through an applicability check that counts every
//! current fact first (aegis-h9c0no, malcolm's diagnosis). Measured on the
//! live store, 2026-09-25: the four-pattern work-item scope join and the
//! co-occurrence join both 408 at 10.0 s, while each step below answers in
//! ~10-100 ms. A query that times out is not a small answer — it is the whole
//! rung silently absent, and a store read held for 10 s after the hook gave up.
//!
//! So every chain here is: resolve an identifier to its IRIs, then one hop per
//! link, each asked as a UNION of the pattern with its subject BOUND:
//! `{ BIND(<a> AS ?x) <a> p ?y } UNION { BIND(<b> AS ?x) <b> p ?y } ...`.
//! Not `VALUES ?x { <a> <b> } ?x p ?y`: quipu does not push a VALUES binding
//! into the pattern, so on a high-cardinality predicate it scans — measured on
//! the live store, 2026-09-25, eleven `rdfs:label` lookups took 3.6-5.2 s as
//! VALUES and 0.03 s as a UNION of bound subjects, same 10 rows. Results use
//! the SPARQL-JSON form [`crate::project::query`] asks for, which carries FULL
//! IRIs — the only form a later hop can bind.

use std::collections::BTreeSet;

use crate::errors::{Error, Result};

/// The `aegis:` prefix declaration, built from the one namespace constant.
fn prefix() -> String {
    format!("PREFIX aegis: <{}>", crate::export::ONTO)
}

/// Subjects per request. Wide enough that a normal item is one request per
/// hop; bounded so a hub entity cannot build a query body the store rejects.
const BATCH: usize = 64;

/// Upper bound on IRIs carried into any one hop. A hub (a file every item
/// touches) can have thousands of modifying commits; past this the hop keeps
/// the first `MAX_WIDTH`, which is still far beyond any count a caller reads.
const MAX_WIDTH: usize = 1024;

/// Escape a string for a SPARQL double-quoted literal.
pub(crate) fn literal(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

/// Expand a quipu PREFIXED name to a full IRI. `/search` and `/context` report
/// entities as `aegis:<local>`; wrapped in `<...>` as-is that is a different,
/// nonexistent IRI (measured: `<aegis:aegis-l50p>` matches 0 rows, the full
/// IRI matches 1). A value that is already absolute passes through.
pub(crate) fn full_iri(iri: &str) -> String {
    match iri.strip_prefix("aegis:") {
        Some(local) if !iri.contains("://") => {
            format!("{}{local}", crate::export::ONTO)
        }
        _ => iri.to_string(),
    }
}

/// Whether `iri` can ride inside `<...>` without breaking out of it.
fn safe_iri(iri: &str) -> bool {
    !iri.is_empty()
        && !iri
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || "<>\"{}|^`\\".contains(c))
}

/// `{ BIND(<a> AS ?from) <pattern, ?from := <a>> } UNION ...` over the safe
/// IRIs in `iris`; empty when none is safe. `pattern`'s terms are separated by
/// single spaces, so `?from` is replaced as a whole token, never as a prefix
/// of another variable.
fn bound_union(iris: &[String], from: &str, pattern: &str) -> String {
    let var = format!("?{from}");
    iris.iter()
        .filter(|i| safe_iri(i))
        .map(|i| {
            let iri = format!("<{}>", full_iri(i));
            let bound = pattern
                .split(' ')
                .map(|term| if term == var { iri.as_str() } else { term })
                .collect::<Vec<_>>()
                .join(" ");
            format!("{{ BIND({iri} AS {var}) {bound} }}")
        })
        .collect::<Vec<_>>()
        .join(" UNION ")
}

fn rows(sparql: &str, endpoint: &str) -> Result<Vec<serde_json::Value>> {
    let body = crate::project::query(endpoint, sparql)?;
    let value: serde_json::Value = serde_json::from_str(&body)
        .map_err(|e| Error::Projection(format!("results are not JSON: {e}")))?;
    value["results"]["bindings"]
        .as_array()
        .cloned()
        .ok_or_else(|| Error::Projection("missing results.bindings".into()))
}

fn value(row: &serde_json::Value, var: &str) -> Option<String> {
    row[var]["value"].as_str().map(str::to_string)
}

/// The IRIs carrying `aegis:identifier "<id>"`.
pub(crate) fn items_with_identifier(endpoint: &str, id: &str) -> Result<Vec<String>> {
    let sparql = format!(
        "{} SELECT DISTINCT ?w WHERE {{ ?w aegis:identifier \"{}\" }}",
        prefix(),
        literal(id)
    );
    Ok(rows(&sparql, endpoint)?
        .iter()
        .filter_map(|r| value(r, "w"))
        .collect())
}

/// One hop: for each `?<from>` in `iris`, the `(from, to)` pairs matching the
/// single `pattern`, which must mention exactly `?<from>` and `?<to>`.
pub(crate) fn hop(
    endpoint: &str,
    iris: &[String],
    from: &str,
    pattern: &str,
    to: &str,
) -> Result<Vec<(String, String)>> {
    let mut out = Vec::new();
    let distinct: BTreeSet<&String> = iris.iter().collect();
    if distinct.len() > MAX_WIDTH {
        // Loud, because a truncated hop is an incomplete answer that reads as
        // a complete one — the exact class of miss this module exists to end.
        eprintln!(
            "yupana: provenance hop `{pattern}` truncated to {MAX_WIDTH} of {} subjects; \
             results past the cap are omitted",
            distinct.len()
        );
    }
    let unique: Vec<String> = distinct.into_iter().take(MAX_WIDTH).cloned().collect();
    for batch in unique.chunks(BATCH) {
        let union = bound_union(batch, from, pattern);
        if union.is_empty() {
            continue;
        }
        let sparql = format!(
            "{} SELECT DISTINCT ?{from} ?{to} WHERE {{ {union} }}",
            prefix()
        );
        out.extend(
            rows(&sparql, endpoint)?
                .iter()
                .filter_map(|r| Some((value(r, from)?, value(r, to)?))),
        );
    }
    Ok(out)
}

/// [`hop`], keeping only the distinct targets.
pub(crate) fn hop_targets(
    endpoint: &str,
    iris: &[String],
    from: &str,
    pattern: &str,
    to: &str,
) -> Result<Vec<String>> {
    let targets: BTreeSet<String> = hop(endpoint, iris, from, pattern, to)?
        .into_iter()
        .map(|(_, t)| t)
        .collect();
    Ok(targets.into_iter().collect())
}

/// Entities modified by commits implementing the item `id`
/// (`Bead <-aegis:implements- Commit -aegis:modifies-> entity`).
pub(crate) fn entities_touched_by(endpoint: &str, id: &str) -> Result<Vec<String>> {
    let items = items_with_identifier(endpoint, id)?;
    let commits = hop_targets(endpoint, &items, "w", "?c aegis:implements ?w", "c")?;
    hop_targets(endpoint, &commits, "c", "?c aegis:modifies ?e", "e")
}

#[cfg(test)]
#[path = "sparql_steps_test.rs"]
mod tests;
