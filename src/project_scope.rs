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
use crate::project_queries::{WORK_ITEM_PARENT_QUERY, WORK_ITEM_SCOPE_QUERY};

fn binding_values(body: &str, name: &str) -> crate::errors::Result<Vec<String>> {
    let value: serde_json::Value = serde_json::from_str(body)
        .map_err(|e| crate::errors::Error::Projection(format!("results are not JSON: {e}")))?;
    let rows = value
        .get("results")
        .and_then(|v| v.get("bindings"))
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| crate::errors::Error::Projection("missing results.bindings".into()))?;
    Ok(rows
        .iter()
        .filter_map(|row| row.get(name)?.get("value")?.as_str().map(str::to_owned))
        .collect())
}

fn sparql_string(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

fn scope_query(item: &str) -> String {
    WORK_ITEM_SCOPE_QUERY.replace("$ITEM", &sparql_string(item))
}

fn parent_query(item: &str) -> String {
    WORK_ITEM_PARENT_QUERY.replace("$ITEM", &sparql_string(item))
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
    let item = crate::plate::current()?;
    fetch_work_item_context_for(endpoint, &item).0
}

/// Fetch the only scope context this process can consume: the current item and
/// its direct parent. Both lookups bind an exact identifier before joining the
/// provenance graph, avoiding the former whole-store three-way join.
pub fn fetch_work_item_context(
    endpoint: &str,
) -> (Option<WorkItemScopes>, Option<WorkItemParents>) {
    let Some(item) = crate::plate::current() else {
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
        let projected = crate::project::query(endpoint, &scope_query(&id))
            .and_then(|body| binding_values(&body, "path"));
        match projected {
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
    let item = crate::plate::current()?;
    fetch_work_item_parents_for(endpoint, &item)
}

fn fetch_work_item_parents_for(endpoint: &str, item: &str) -> Option<WorkItemParents> {
    match crate::project::query(endpoint, &parent_query(item))
        .and_then(|body| binding_values(&body, "parent"))
    {
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
    use super::{parent_query, scope_query};
    use crate::policy::WorkItemScopes;

    #[test]
    fn scope_projection_is_bound_to_one_exact_item() {
        let query = scope_query("aegis-1");
        assert!(query.contains("aegis:identifier \"aegis-1\""));
        assert!(!query.contains("aegis:identifier ?id"));
        assert!(query.contains("SELECT ?path"));
    }

    #[test]
    fn parent_projection_is_bound_to_one_exact_item() {
        let query = parent_query("aegis-1");
        assert!(query.contains("aegis:identifier \"aegis-1\""));
        assert!(!query.contains("?w aegis:identifier ?id"));
        assert!(query.contains("SELECT ?parent"));
    }

    #[test]
    fn item_ids_cannot_break_out_of_the_sparql_literal() {
        let query = scope_query("x\" . ?s ?p ?o . #");
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
