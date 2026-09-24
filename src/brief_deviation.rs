//! The DEVIATION-seeded context source — what the graph knows about a path an
//! agent has just stepped onto, outside its work item's ground.
//!
//! Sibling of [`crate::brief_sources`], and split from it deliberately rather
//! than by file size alone. Those sources are seeded on an item's GROUND at
//! assignment time; this one is seeded on a single path at the moment the agent
//! leaves that ground, which is a different question asked at a different
//! enforcement point ([`crate::hook::scope_notice`], the Post-Action Auditor).
//!
//! It is the other half of the symmetry docs/work-scoped-governance.md §3
//! names: if the graph can predict what an agent may ACCESS, it can predict
//! what that agent will need to READ, and those are nearly the same query.
//! Assignment time was already exploiting it. Deviation time was not.

/// Who else has worked on a PATH: the work items whose commits modified it,
/// with each item's declared outcome where the graph has one.
///
/// The deviation half of the scope/context symmetry
/// (docs/work-scoped-governance.md §3): the same provenance chain that answers
/// "may this agent touch this" also answers "who touched it before, and how did
/// that go". `brief_sources` asks it seeded on the item's GROUND at assignment
/// time; `crate::hook::scope_notice` asks it seeded on the path the agent just
/// stepped onto, which is the moment the answer is most useful.
///
/// Returns `(item id, outcome)` pairs, outcome absent when the item is open.
/// Empty on any failure — a notice source that cannot answer must not turn a
/// scope advisory into an error.
///
/// ASKED AS SINGLE-PATTERN STEPS, NOT ONE JOIN (aegis-h9c0no). The natural
/// form — one `SELECT` joining filePath -> modifies -> implements -> identifier
/// — hit quipu's 10s query timeout on EVERY call (measured 2026-09-24 against
/// quipu 0.8.1, 5 of 5; the first two patterns alone time out too), while each
/// pattern on its own answers in 4-60ms. Every out-of-scope post-edit therefore
/// held a quipu read for the full 10s after the hook's own deadline had given
/// up on it: `yupana-hook` held/e2e was 1.74 over the hour it was measured,
/// with every other client at or under 1.0. So each step here binds its subject
/// with `VALUES` and names its predicate — the shape quipu serves from an index.
/// An unbound predicate (`?w ?p ?o`) or a second `VALUES` block times out even
/// with a bound subject, which is why identifier and outcome are two queries.
pub fn items_touching_path(endpoint: &str, rel: &str) -> Vec<(String, Option<String>)> {
    let entities = column(
        endpoint,
        &format!(
            "{PREFIX} SELECT DISTINCT ?e WHERE {{ ?e aegis:filePath \"{}\" }} LIMIT {FAN_OUT}",
            path_literal(rel)
        ),
        "e",
    );
    let commits = step(endpoint, "e", &entities, "?c aegis:modifies ?e", "c");
    let items = step(endpoint, "c", &commits, "?c aegis:implements ?w", "w");
    let ids = pairs(endpoint, &items, "aegis:identifier");
    let outcomes = pairs(endpoint, &items, "aegis:outcome");
    let mut found: Vec<(String, Option<String>)> = ids
        .into_iter()
        .map(|(w, id)| {
            let outcome = outcomes
                .iter()
                .find(|(o, _)| *o == w)
                .map(|(_, v)| v.clone());
            (id, outcome)
        })
        .collect();
    found.sort();
    found.dedup();
    found.truncate(5);
    found
}

const PREFIX: &str = "PREFIX aegis: <http://aegis.gastown.local/ontology/>";

/// Bound on every step's width. A path touched by more commits than this gets
/// a notice from the first `FAN_OUT` of them — the notice names at most five
/// items, so completeness past this point buys nothing but query size.
const FAN_OUT: usize = 64;

/// `VALUES ?<var> { <iri>... } <pattern>` -> the distinct `?<out>` IRIs.
fn step(endpoint: &str, var: &str, iris: &[String], pattern: &str, out: &str) -> Vec<String> {
    if iris.is_empty() {
        return Vec::new();
    }
    column(
        endpoint,
        &format!(
            "{PREFIX} SELECT DISTINCT ?{out} WHERE {{ VALUES ?{var} {{ {} }} {pattern} }} LIMIT {FAN_OUT}",
            values_block(iris)
        ),
        out,
    )
}

/// `(item IRI, literal)` for one named predicate over the given items.
fn pairs(endpoint: &str, items: &[String], predicate: &str) -> Vec<(String, String)> {
    if items.is_empty() {
        return Vec::new();
    }
    let query = format!(
        "{PREFIX} SELECT ?w ?v WHERE {{ VALUES ?w {{ {} }} ?w {predicate} ?v }}",
        values_block(items)
    );
    bindings(endpoint, &query)
        .iter()
        .filter_map(|b| {
            Some((
                b["w"]["value"].as_str()?.to_string(),
                b["v"]["value"].as_str()?.to_string(),
            ))
        })
        .collect()
}

fn column(endpoint: &str, query: &str, var: &str) -> Vec<String> {
    bindings(endpoint, query)
        .iter()
        .filter_map(|b| b[var]["value"].as_str().map(str::to_string))
        .collect()
}

fn bindings(endpoint: &str, query: &str) -> Vec<serde_json::Value> {
    let json = crate::project::query(endpoint, query).unwrap_or_default();
    serde_json::from_str::<serde_json::Value>(&json)
        .ok()
        .and_then(|v| v["results"]["bindings"].as_array().cloned())
        .unwrap_or_default()
}

/// IRIs into a `VALUES` body. Anything that could close the `<...>` early is
/// dropped rather than escaped: these come back from the store as IRIs, so a
/// character that cannot appear in one means the value is not one.
pub(crate) fn values_block(iris: &[String]) -> String {
    iris.iter()
        .filter(|i| {
            !i.chars()
                .any(|c| matches!(c, '<' | '>' | '"' | ' ' | '{' | '}' | '\\'))
        })
        .map(|i| format!("<{i}>"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Escape a repo-relative path for a double-quoted SPARQL literal.
///
/// Separate from [`sanitized`], which STRIPS anything not alphanumeric — right
/// for a tracker id, and wrong for a path, where it would silently delete every
/// `/` and `.` and turn `src/a.rs` into a query for `srcars`. A query that
/// quietly asks a different question than the caller intended is worse than one
/// that fails.
pub(crate) fn path_literal(rel: &str) -> String {
    rel.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
mod path_literal_tests {
    use super::path_literal;

    /// A path must survive escaping intact. `sanitized` — right for a tracker
    /// id — strips everything non-alphanumeric, which would turn `src/a.rs`
    /// into `srcars` and silently ask a different question than the caller
    /// intended. A query that quietly answers the wrong question is worse than
    /// one that fails, so the two escapers are deliberately separate.
    #[test]
    fn a_path_keeps_its_separators_and_extension() {
        assert_eq!(
            path_literal("src/hook/scope_notice.rs"),
            "src/hook/scope_notice.rs"
        );
    }

    /// And it must not be able to break out of the double-quoted literal.
    #[test]
    fn a_quote_or_backslash_is_escaped_not_passed_through() {
        assert_eq!(path_literal(r#"a"b"#), r#"a\"b"#);
        assert_eq!(path_literal(r"a\b"), r"a\\b");
    }

    /// A value that could close the IRI early never reaches the query.
    #[test]
    fn values_block_drops_anything_that_is_not_an_iri() {
        let got = super::values_block(&[
            "http://x/a".to_string(),
            "http://x/b> } ?s ?p ?o {".to_string(),
        ]);
        assert_eq!(got, "<http://x/a>");
    }
}
