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
pub fn items_touching_path(endpoint: &str, rel: &str) -> Vec<(String, Option<String>)> {
    let query = format!(
        "PREFIX aegis: <http://aegis.gastown.local/ontology/> \
         SELECT DISTINCT ?id ?outcome WHERE {{ \
           ?e aegis:filePath \"{}\" . \
           ?c aegis:modifies ?e ; aegis:implements ?w . \
           ?w aegis:identifier ?id . \
           OPTIONAL {{ ?w aegis:outcome ?outcome }} \
         }} LIMIT 5",
        path_literal(rel)
    );
    let json = crate::project::query(endpoint, &query).unwrap_or_default();
    serde_json::from_str::<serde_json::Value>(&json)
        .ok()
        .and_then(|v| v["results"]["bindings"].as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|b| {
            let id = b["id"]["value"].as_str()?.to_string();
            Some((id, b["outcome"]["value"].as_str().map(str::to_string)))
        })
        .collect()
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
}
