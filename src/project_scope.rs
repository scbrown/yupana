//! Tenant-bound observed work-item scope projection — the bottom rung of the
//! work-scoped-governance trust ladder (docs/work-scoped-governance.md),
//! projected into the hot plane like every other governed catalogue.
//!
//! The rung needs no new graph vocabulary: "what did prior work on this item
//! actually touch" is quipu's deterministic provenance chain
//! (`Bead <-aegis:implements- Commit -aegis:modifies-> entity`) joined to the
//! entities' file paths. It projects only the current item and its direct
//! parent: those are the only two grounds the evaluator can consume, and an
//! exact identifier keeps quipu off the former unbounded whole-store join.
//! The projection rides the existing refresh + durable
//! cache cycle — never a fetch per edit — and the guard consults it only when
//! the static (declared) scope table has no entry for the tenant.
//!
//! Split out of [`crate::project`] for file size, like `project_grounding`.

use crate::policy::{WorkItemParents, WorkItemScopes};
use crate::project::ProjectionRegistry;
use crate::sparql_steps::{entities_touched_by, hop_targets, items_with_identifier};

/// The paths prior work on `item` touched, walked one bound pattern per hop.
/// The same chain as [`crate::project_queries::WORK_ITEM_SCOPE_QUERY`], which
/// as ONE four-pattern join hits quipu's 10 s deadline on the live store
/// (aegis-h9c0no, measured 2026-09-25) — so the observed rung was never
/// projected at all, and said so only on stderr.
fn scope_paths(endpoint: &str, item: &str) -> crate::errors::Result<Vec<String>> {
    let entities = entities_touched_by(endpoint, item)?;
    hop_targets(endpoint, &entities, "e", "?e aegis:filePath ?path", "path")
}

/// The identifiers of the items that `aegis:contains` `item`.
fn parent_ids(endpoint: &str, item: &str) -> crate::errors::Result<Vec<String>> {
    let items = items_with_identifier(endpoint, item)?;
    let parents = hop_targets(endpoint, &items, "w", "?p aegis:contains ?w", "p")?;
    hop_targets(
        endpoint,
        &parents,
        "p",
        "?p aegis:identifier ?parent",
        "parent",
    )
}

/// Fetch the current item's observed scope (plus direct-parent ground), or
/// `None` when it cannot be
/// projected — loudly, because a scope that cannot be projected leaves an
/// undeclared tenant UNGUARDED by scope (advisory), never silently in-scope.
/// Never an error: a failed scope projection must not disable the planes
/// that did project.
///
/// Skipped entirely (silently `None`) when no work-item tracker is wired
/// (`$SHANTY_ROOT`/`$SHANTY_AGENT` absent): without a plate there is no item
/// to resolve a scope for, and the query would add a round-trip to every
/// edit for a map nothing reads.
pub fn fetch_work_item_scopes(endpoint: &str) -> Option<WorkItemScopes> {
    let item = crate::plate::current(None)?;
    fetch_work_item_context_for(endpoint, &item).0
}

/// Fetch the only scope context this process can consume: the current item and
/// its direct parent. Both lookups bind an exact identifier before joining the
/// provenance graph, avoiding the former whole-store three-way join.
pub fn fetch_work_item_context(
    endpoint: &str,
) -> (Option<WorkItemScopes>, Option<WorkItemParents>) {
    let Some(item) = crate::plate::current(None) else {
        return (None, None);
    };
    fetch_work_item_context_for(endpoint, &item)
}

fn fetch_work_item_context_for(
    endpoint: &str,
    item: &str,
) -> (Option<WorkItemScopes>, Option<WorkItemParents>) {
    let parents = fetch_work_item_parents_for(endpoint, item);
    let mut ids = vec![item];
    if let Some(parent) = parents.as_ref().and_then(|map| map.parent_of(item)) {
        ids.push(parent);
    }

    let mut rows = Vec::new();
    for id in ids {
        match scope_paths(endpoint, id) {
            Ok(found) => rows.extend(found.into_iter().map(|path| (id.to_string(), path))),
            Err(e) => {
                eprintln!(
                    "yupana: work-item scope map could not be projected ({e}) — \
                     tenants without a declared scope are UNGUARDED by scope, \
                     not silently in-scope"
                );
                return (None, parents);
            }
        }
    }
    (Some(WorkItemScopes::from_rows(rows)), parents)
}

/// Fetch the current item's parent behind the DERIVED rung.
///
/// Same contract as [`fetch_work_item_scopes`] in every respect: gated on a
/// wired tracker, `None` on failure with a loud line, and never an error —
/// a rung that cannot project must disable only itself.
///
/// The difference worth stating: a `None` here is strictly LESS serious than a
/// `None` there. Losing the observed map leaves an undeclared tenant with no
/// scope at all; losing this one only means an item with no ground of its own
/// stops inheriting its parent's, which returns the ladder to exactly the
/// behaviour it had before this rung existed.
pub fn fetch_work_item_parents(endpoint: &str) -> Option<WorkItemParents> {
    let item = crate::plate::current(None)?;
    fetch_work_item_parents_for(endpoint, &item)
}

fn fetch_work_item_parents_for(endpoint: &str, item: &str) -> Option<WorkItemParents> {
    match parent_ids(endpoint, item) {
        Ok(parents) => Some(WorkItemParents::from_rows(
            parents.into_iter().map(|parent| (item.to_string(), parent)),
        )),
        Err(e) => {
            eprintln!(
                "yupana: work-item parent map could not be projected ({e}) — \
                 the derived scope rung is inactive this refresh; items with no \
                 observed ground of their own fall through to unknown scope"
            );
            None
        }
    }
}

impl ProjectionRegistry {
    /// The projected observed scope map, or `None` when the scope projection
    /// is missing/failed (unknown scope — the guard advises).
    #[must_use]
    pub fn work_item_scopes(&self) -> Option<&WorkItemScopes> {
        self.work_item_scopes.as_ref()
    }

    /// Install a scope map directly (test/daemon seam), like `set_grounding`.
    pub fn set_work_item_scopes(&mut self, scopes: Option<WorkItemScopes>) {
        self.work_item_scopes = scopes;
    }
}

impl ProjectionRegistry {
    /// The projected work-item parent map, or `None` when it is missing or
    /// failed (the derived rung simply does not fire).
    #[must_use]
    pub fn work_item_parents(&self) -> Option<&crate::policy::WorkItemParents> {
        self.work_item_parents.as_ref()
    }
}

#[cfg(test)]
// Test names shout the invariant they turn on, the repo's emphasis convention.
#[allow(non_snake_case)]
mod tests {
    use super::{parent_ids, scope_paths};
    use crate::policy::WorkItemScopes;
    use crate::test_stub::stub;
    use serde_json::json;

    const O: &str = crate::export::ONTO;

    /// Every request to the store must be ONE triple pattern per branch (a
    /// UNION of subject-bound branches is fine): a multi-pattern BGP is what
    /// hit quipu's 10 s deadline and left this rung unprojected (aegis-h9c0no).
    fn assert_single_pattern(query: &str) {
        let body = query.split("WHERE {").nth(1).expect("a WHERE clause");
        assert_eq!(body.matches(" . ").count(), 0, "joined patterns: {query}");
        assert!(!body.contains(';'), "joined patterns: {query}");
        assert!(!body.contains("VALUES"), "unbound VALUES scan: {query}");
    }

    #[test]
    fn scope_walks_the_chain_one_bound_pattern_per_hop() {
        let (endpoint, server) = stub(4, |index, _| {
            let body = match index {
                0 => json!({"results":{"bindings":[{"w":{"value":format!("{O}aegis-1.2")}}]}}),
                1 => json!({"results":{"bindings":[
                    {"w":{"value":format!("{O}aegis-1.2")},"c":{"value":format!("{O}c1")}}]}}),
                2 => json!({"results":{"bindings":[
                    {"c":{"value":format!("{O}c1")},"e":{"value":format!("{O}e1")}}]}}),
                _ => json!({"results":{"bindings":[
                    {"e":{"value":format!("{O}e1")},"path":{"value":"src/a.rs"}}]}}),
            };
            (200, body)
        });
        let paths = scope_paths(&endpoint, "aegis-1.2").expect("projected");
        assert_eq!(paths, vec!["src/a.rs"]);
        let requests = server.join().unwrap();
        let queries: Vec<&str> = requests
            .iter()
            .map(|r| r["query"].as_str().unwrap())
            .collect();
        // The dotted child id rides into the literal intact.
        assert!(queries[0].contains("aegis:identifier \"aegis-1.2\""));
        assert!(queries[1].contains(&format!("<{O}aegis-1.2>")));
        for query in &queries {
            assert_single_pattern(query);
        }
    }

    #[test]
    fn parent_walks_contains_then_identifier() {
        let (endpoint, server) = stub(3, |index, _| {
            let body = match index {
                0 => json!({"results":{"bindings":[{"w":{"value":format!("{O}child")}}]}}),
                1 => json!({"results":{"bindings":[
                    {"w":{"value":format!("{O}child")},"p":{"value":format!("{O}epic")}}]}}),
                _ => json!({"results":{"bindings":[
                    {"p":{"value":format!("{O}epic")},"parent":{"value":"aegis-epic"}}]}}),
            };
            (200, body)
        });
        assert_eq!(
            parent_ids(&endpoint, "aegis-epic.1").unwrap(),
            vec!["aegis-epic"]
        );
        for request in server.join().unwrap() {
            assert_single_pattern(request["query"].as_str().unwrap());
        }
    }

    #[test]
    fn a_failed_hop_is_an_error_not_an_empty_scope() {
        let (endpoint, server) = stub(2, |index, _| match index {
            0 => (
                200,
                json!({"results":{"bindings":[{"w":{"value":format!("{O}w")}}]}}),
            ),
            _ => (408, json!({"error":"query deadline"})),
        });
        assert!(scope_paths(&endpoint, "aegis-1").is_err());
        server.join().unwrap();
    }

    #[test]
    fn item_ids_cannot_break_out_of_the_sparql_literal() {
        let (endpoint, server) = stub(1, |_, _| (200, json!({"results":{"bindings":[]}})));
        assert!(scope_paths(&endpoint, "x\" . ?s ?p ?o . #")
            .unwrap()
            .is_empty());
        let query = server.join().unwrap()[0]["query"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(query.contains("x\\\" . ?s ?p ?o . #"));
    }

    #[test]
    fn rows_fold_into_per_item_path_sets() {
        let map = WorkItemScopes::from_rows([
            ("aegis-1".to_string(), "src/a.rs".to_string()),
            ("aegis-1".to_string(), "src/b.rs".to_string()),
            ("aegis-2".to_string(), "docs/x.md".to_string()),
        ]);
        assert_eq!(map.len(), 2);
        let scope = map.scope_for("aegis-1").expect("aegis-1 has a scope");
        assert_eq!(scope.allow_paths, vec!["src/a.rs", "src/b.rs"]);
        assert!(scope.deny_paths.is_empty());
    }

    #[test]
    fn an_item_with_no_observed_paths_is_UNKNOWN_not_empty_scope() {
        let map = WorkItemScopes::from_rows([("aegis-1".to_string(), "src/a.rs".to_string())]);
        // None, never Some(empty-allow) — an empty allow list would mean "any
        // path", and a missing item must not read as unconstrained-by-right.
        assert!(map.scope_for("aegis-9").is_none());
    }

    #[test]
    fn decode_drops_partial_rows_rather_than_erroring() {
        let body = r#"{"results":{"bindings":[
            {"id":{"value":"aegis-1"},"path":{"value":"src/a.rs"}},
            {"id":{"value":"aegis-half"}}
        ]}}"#;
        let rows = crate::project_decode::decode_work_item_scope_rows(body).unwrap();
        assert_eq!(rows, vec![("aegis-1".to_string(), "src/a.rs".to_string())]);
    }
}
