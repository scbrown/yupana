//! §9.7's `commit → touched entities` edge (GH #5), produced in yupana.

use super::commit_turtle;

/// A repo whose history has the three commit shapes promotion meets: a root
/// commit, an ordinary one, and a `--no-ff` merge. Returns the dir.
fn fixture() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .arg("-C")
            .arg(dir.path())
            .args(args)
            .output()
            .unwrap();
    };
    let write = |name: &str, body: &str| {
        std::fs::write(dir.path().join(name), body).unwrap();
    };
    git(&["init", "-q", "-b", "main"]);
    git(&["config", "user.email", "dev@example.com"]);
    git(&["config", "user.name", "Dev"]);
    write("a.rs", "pub fn a() -> u32 { 1 }\n");
    git(&["add", "-A"]);
    git(&["commit", "-qm", "root"]);
    git(&["checkout", "-qb", "side"]);
    write("b.rs", "pub fn b() -> u32 { 2 }\n");
    git(&["add", "-A"]);
    git(&["commit", "-qm", "side"]);
    git(&["checkout", "-q", "main"]);
    write("c.rs", "pub fn c() -> u32 { 3 }\n");
    // A doc and a lockfile: changed by the commit, but neither is a CodeModule.
    write("NOTES.md", "# notes\n");
    write("Cargo.lock", "# lock\n");
    git(&["add", "-A"]);
    git(&["commit", "-qm", "main work"]);
    git(&["merge", "-q", "--no-ff", "-m", "merge side", "side"]);
    dir
}

/// The projection the promotion is about to write, for the tree at `reference`.
fn projection(dir: &tempfile::TempDir, reference: &str) -> String {
    crate::export::to_turtle_at(dir.path(), "realname", reference).unwrap()
}

/// The core criterion: a promoted commit yields provenance edges to EXACTLY the
/// entities it touched — no more (the untouched module is absent) and no less.
#[test]
fn a_commit_modifies_exactly_the_entities_it_touched() {
    let dir = fixture();
    let head = crate::git::resolve_commit(dir.path(), "HEAD~1").unwrap();
    let proj = projection(&dir, &head);
    let ttl = commit_turtle(dir.path(), "realname", &head, &proj).expect("edges for a real commit");

    assert!(ttl.contains("a bobbin:GitCommit"), "{ttl}");
    assert!(
        ttl.contains("bobbin:modifies <http://aegis.gastown.local/ontology/code/realname/c.rs>"),
        "the changed module must be modified: {ttl}"
    );
    assert!(
        !ttl.contains("code/realname/a.rs>"),
        "a module this commit did not touch must NOT be modified: {ttl}"
    );
    // Non-code paths the commit really did change are filtered out: they have no
    // CodeModule in the projection, and §9.7's edge ranges over code entities.
    assert!(!ttl.contains("NOTES.md"), "{ttl}");
    assert!(!ttl.contains("Cargo.lock"), "{ttl}");
    assert_eq!(
        ttl.matches("bobbin:modifies").count(),
        1,
        "one edge, for the one touched CodeModule: {ttl}"
    );
}

/// A MERGE is the shape the default `promote_on` policy makes interesting, and
/// the one a bare `git diff-tree` silently reports as touching nothing. It must
/// carry what the merge brought in.
#[test]
fn a_merge_commit_modifies_what_the_merge_brought_in() {
    let dir = fixture();
    let merge = crate::git::resolve_commit(dir.path(), "HEAD").unwrap();
    assert!(
        crate::git::is_merge_commit(dir.path(), &merge),
        "fixture must actually produce a merge"
    );
    let proj = projection(&dir, &merge);
    let ttl = commit_turtle(dir.path(), "realname", &merge, &proj)
        .expect("a merge must not silently touch nothing");
    assert!(
        ttl.contains("code/realname/b.rs>"),
        "the merge brought b.rs in from `side`: {ttl}"
    );
    assert!(
        !ttl.contains("code/realname/c.rs>"),
        "c.rs was already on the first parent — the merge did not bring it: {ttl}"
    );
}

/// The ROOT commit has no parent. It must report its whole tree, not fail for
/// want of something to diff against.
#[test]
fn the_root_commit_modifies_its_whole_tree() {
    let dir = fixture();
    let root = String::from_utf8(
        std::process::Command::new("git")
            .arg("-C")
            .arg(dir.path())
            .args(["rev-list", "--max-parents=0", "HEAD"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_string();
    let proj = projection(&dir, &root);
    let ttl = commit_turtle(dir.path(), "realname", &root, &proj).expect("root commit has facts");
    assert!(ttl.contains("code/realname/a.rs>"), "{ttl}");
}

/// The commit node carries the provenance fields the pinned contract names —
/// including the authored time as a TYPED dateTime, which is the valid-time this
/// edge exists to carry.
#[test]
fn the_commit_node_carries_hash_repo_author_and_a_typed_date() {
    let dir = fixture();
    let head = crate::git::resolve_commit(dir.path(), "HEAD~1").unwrap();
    let proj = projection(&dir, &head);
    let ttl = commit_turtle(dir.path(), "realname", &head, &proj).unwrap();
    assert!(ttl.contains(&format!("bobbin:hash \"{head}\"")), "{ttl}");
    assert!(ttl.contains("bobbin:repo \"realname\""), "{ttl}");
    assert!(
        ttl.contains("bobbin:author \"Dev <dev@example.com>\""),
        "{ttl}"
    );
    assert!(
        ttl.contains("^^xsd:dateTime"),
        "the date must be typed: {ttl}"
    );
    // The label spelling matches the tracker-aware ingest lane's, so a future
    // convergence on one commit IRI does not accumulate two labels on one node.
    assert!(
        ttl.contains(&format!("rdfs:label \"realname@{}\"", &head[..12])),
        "{ttl}"
    );
    // The commit IRI shares the base every other promoted entity uses — the
    // whole point of producing this edge in yupana rather than out of tree.
    assert!(
        ttl.contains(&format!(
            "<http://aegis.gastown.local/ontology/code/realname/commit/{head}>"
        )),
        "{ttl}"
    );
}

/// Yupana emits `modifies`, never `implements` — the commit→work-item link needs
/// a project-prefix vocabulary yupana does not hold, and re-deriving it here is
/// how two lanes drift. Pinned so it is a decision, not an oversight someone
/// later "fixes".
#[test]
fn no_implements_edge_is_invented() {
    let dir = fixture();
    let head = crate::git::resolve_commit(dir.path(), "HEAD~1").unwrap();
    let proj = projection(&dir, &head);
    let ttl = commit_turtle(dir.path(), "realname", &head, &proj).unwrap();
    assert!(!ttl.contains("implements"), "{ttl}");
    assert!(!ttl.contains("Bead"), "{ttl}");
}

/// ABSTAIN rather than emit a bare commit node: a commit that touched no
/// promoted module is not the fact §9.7 asks for.
#[test]
fn a_commit_touching_no_promoted_module_emits_nothing() {
    let dir = fixture();
    let head = crate::git::resolve_commit(dir.path(), "HEAD~1").unwrap();
    // An EMPTY projection declares no CodeModule, so nothing survives the filter.
    assert!(commit_turtle(dir.path(), "realname", &head, "").is_none());
    // And an unresolvable ref is an abstention, not a panic.
    assert!(commit_turtle(dir.path(), "realname", "no-such-ref", "").is_none());
}

/// The whole projection — structure plus provenance plus the branch qualifier —
/// must pass the gate that stands in front of every write. A provenance edge
/// that cannot be promoted is not a provenance edge.
#[cfg(feature = "quipu")]
#[test]
fn the_provenance_survives_the_promotion_gate() {
    let dir = fixture();
    let head = crate::git::resolve_commit(dir.path(), "HEAD").unwrap();
    let mut ttl = projection(&dir, &head);
    ttl.push_str(&commit_turtle(dir.path(), "realname", &head, &ttl.clone()).unwrap());
    let ttl = crate::promote_branch::qualify(
        crate::promote_branch::BranchModel::Qualifier,
        &ttl,
        Some("main"),
    )
    .unwrap();
    let v = crate::promote::validate(&ttl, crate::promote::CODE_EDGE_SHAPES).expect("ran");
    assert!(
        v.conforms,
        "the provenance edge broke SHACL conformance: {:?}",
        v.violations
    );
    // The commit node is on the branch too — it was appended before the
    // qualifier ran, which is what makes that true rather than accidental.
    assert!(
        ttl.contains(&format!(
            "<http://aegis.gastown.local/ontology/code/realname/commit/{head}> bobbin:onBranch \"main\""
        )),
        "{ttl}"
    );
}

/// A commit block must not become an unsplittable statement: an import-shaped
/// commit touching every file would then blow the chunk limit and refuse the
/// whole promotion. Each edge is its own statement.
#[test]
fn each_modifies_edge_is_its_own_statement() {
    let dir = fixture();
    let root = crate::git::resolve_commit(dir.path(), "HEAD~1").unwrap();
    let proj = projection(&dir, &root);
    let ttl = commit_turtle(dir.path(), "realname", &root, &proj).unwrap();
    for line in ttl.lines().filter(|l| l.contains("bobbin:modifies")) {
        assert!(
            line.trim_end().ends_with(" ."),
            "a modifies edge must terminate its own statement: {line:?}"
        );
    }
}
