use super::*;

// `leaf` is called by `caller`, which is called by `top` — a 2-hop chain so
// impact can be tested past a single hop.
fn chain_repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("leaf.rs"), "fn leaf() {}\n").unwrap();
    std::fs::write(dir.path().join("mid.rs"), "fn caller() { leaf(); }\n").unwrap();
    std::fs::write(dir.path().join("top.rs"), "fn top() { caller(); }\n").unwrap();
    dir
}

#[test]
fn callers_of_a_known_symbol_come_from_the_resident_graph() {
    let dir = chain_repo();
    let engine = ResidentEngine::build(dir.path(), None).unwrap();
    let n = engine.neighbors("leaf", Dir::Callers);
    assert!(n.found, "leaf is in the graph");
    assert!(
        n.neighbors.iter().any(|r| r.name == "caller"),
        "leaf's direct caller is `caller`, got {:?}",
        n.neighbors
    );
    assert_eq!(n.tier, "treesitter");
}

#[test]
fn an_unknown_symbol_is_NOT_FOUND_distinct_from_no_neighbors() {
    // found=false and an empty list are different answers; a symbol that IS in
    // the graph but has no callers would be found=true + empty.
    let dir = chain_repo();
    let engine = ResidentEngine::build(dir.path(), None).unwrap();
    let missing = engine.neighbors("does_not_exist", Dir::Callers);
    assert!(!missing.found);
    assert!(missing.neighbors.is_empty());

    let top = engine.neighbors("top", Dir::Callers);
    assert!(top.found, "top exists");
    assert!(top.neighbors.is_empty(), "nothing calls top");
}

#[test]
fn measure_edit_sizes_against_the_resident_graph() {
    // The exact question the pre-edit guard asks, answered from the resident
    // graph. Editing `leaf` reaches `caller` (mid.rs) and `top` (top.rs) — two
    // symbols across two files; the edited file itself is excluded. `measure_edit`
    // shares `edit_touch` + `walk_blast` with the transient `measure`, differing
    // only in graph source, so this radius is the transient path's radius too.
    let dir = chain_repo();
    let engine = ResidentEngine::build(dir.path(), None).unwrap();
    let file = dir.path().join("leaf.rs");
    let sizing = engine.measure_edit(&file, "leaf.rs", &["fn leaf".to_string()], 5);
    match sizing {
        Sizing::Measured(radius) => {
            assert_eq!(radius.symbols, 2, "leaf reaches caller and top");
            assert_eq!(radius.files, 2, "in mid.rs and top.rs");
        }
        other => panic!("expected a measured radius, got {other:?}"),
    }
}

#[test]
fn measure_edit_reports_UNMEASURED_for_an_unparseable_file_never_a_silent_zero() {
    // The fail-open/loud contract flows through unchanged: a file the graph
    // cannot parse is NOT a radius of zero (which would read as "within limits").
    let dir = chain_repo();
    std::fs::write(dir.path().join("notes.md"), "# hi\n").unwrap();
    let engine = ResidentEngine::build(dir.path(), None).unwrap();
    let file = dir.path().join("notes.md");
    let sizing = engine.measure_edit(&file, "notes.md", &[], 5);
    assert!(
        !matches!(sizing, Sizing::Measured(_)),
        "an unparseable file must be UNMEASURED, not a measured zero: {sizing:?}"
    );
}

#[test]
fn impact_follows_the_chain_transitively() {
    let dir = chain_repo();
    let engine = ResidentEngine::build(dir.path(), None).unwrap();
    let imp = engine.impact("leaf", 5);
    assert!(imp.found);
    // Changing leaf transitively affects caller AND top.
    let names: Vec<&str> = imp.reachable.iter().map(|r| r.name.as_str()).collect();
    assert!(names.contains(&"caller"), "got {names:?}");
    assert!(names.contains(&"top"), "got {names:?}");
}

#[test]
fn a_resident_impact_query_is_far_under_the_SLO() {
    // The daemon's reason for being: the query runs against the RESIDENT graph,
    // with NO rebuild. yupana #1's SLO is blast-radius 5 hops < 300ms p95. Against
    // a resident graph a single query is microseconds; this pins that the query
    // path itself carries no rebuild cost. (Build time is paid once, at startup,
    // and is excluded here on purpose — that is exactly what the daemon moves off
    // the per-query path.)
    let dir = chain_repo();
    let engine = ResidentEngine::build(dir.path(), None).unwrap();
    let start = std::time::Instant::now();
    for _ in 0..100 {
        let _ = engine.impact("leaf", 5);
    }
    let per_query = start.elapsed() / 100;
    assert!(
        per_query < std::time::Duration::from_millis(50),
        "a resident-graph impact query took {per_query:?} — the SLO win is that \
             this path has no rebuild cost"
    );
}
