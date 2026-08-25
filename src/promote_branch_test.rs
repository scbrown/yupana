//! §9.4 branch modeling (GH #4): the qualifier fallback, and the refusal that
//! keeps the unimplemented design from looking implemented.

use super::{parse, qualify, BranchModel};

/// A projection in exactly the shape `export::render` emits.
const PROJECTION: &str = "\
@prefix bobbin: <http://aegis.gastown.local/ontology/> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .

<http://x/code/r/a.rs> a bobbin:CodeModule ;
    rdfs:label \"a.rs\" ;
    bobbin:filePath \"a.rs\" ;
    bobbin:repo \"r\" ;
    bobbin:language \"rust\" .

<http://x/code/r/a.rs::f> a bobbin:CodeSymbol ;
    rdfs:label \"f\" ;
    bobbin:name \"f\" ;
    bobbin:symbolKind \"function\" ;
    bobbin:definedIn <http://x/code/r/a.rs> .

<http://x/code/r/a.rs::f> bobbin:calls <http://x/code/r/a.rs::g> .
";

#[test]
fn the_two_documented_values_parse_and_nothing_else_does() {
    assert_eq!(parse("qualifier").unwrap(), BranchModel::Qualifier);
    assert_eq!(parse(" named_graph ").unwrap(), BranchModel::NamedGraph);
    let err = parse("branch").unwrap_err().to_string();
    assert!(err.contains("branch"), "must quote what was set: {err}");
    assert!(err.contains("qualifier"), "must list the values: {err}");
    assert!(err.contains("named_graph"), "must list the values: {err}");
}

/// The config default must be the model that is actually implemented — a
/// default the promotion path refuses would refuse every promotion out of the
/// box, which is how the preferred-but-unbuilt design would have shipped.
#[test]
fn the_config_default_is_the_implemented_model() {
    let default = crate::config::YupanaConfig::default().quipu.branch_model;
    assert_eq!(
        parse(&default).unwrap(),
        BranchModel::Qualifier,
        "default branch_model = {default:?} is not the implemented model"
    );
    assert!(qualify(parse(&default).unwrap(), PROJECTION, Some("main")).is_ok());
}

/// Every typed entity gains the qualifier — modules and symbols alike — and the
/// original projection is left byte-for-byte intact ahead of it.
#[test]
fn qualifier_tags_every_typed_entity_and_preserves_the_projection() {
    let out = qualify(BranchModel::Qualifier, PROJECTION, Some("main")).unwrap();
    assert!(
        out.starts_with(PROJECTION),
        "the qualifier must be appended, never rewrite the projection"
    );
    assert!(out.contains("<http://x/code/r/a.rs> bobbin:onBranch \"main\" ."));
    assert!(out.contains("<http://x/code/r/a.rs::f> bobbin:onBranch \"main\" ."));
    assert_eq!(
        out.matches("bobbin:onBranch").count(),
        2,
        "one qualifier per typed subject, and the `calls` edge line is not a \
         typed subject — it must not mint a third"
    );
}

/// The whole point of the fallback: it needs no Quipu change, so a qualified
/// projection must still satisfy the shapes yupana gates writes with.
#[cfg(feature = "quipu")]
#[test]
fn a_qualified_projection_still_passes_the_promotion_gate() {
    let out = qualify(BranchModel::Qualifier, PROJECTION, Some("main")).unwrap();
    let v = crate::promote::validate(&out, crate::promote::CODE_EDGE_SHAPES).expect("ran");
    assert!(
        v.conforms,
        "the branch qualifier broke SHACL conformance: {:?}",
        v.violations
    );
}

/// ABSENT, NEVER FAKED. An undeterminable branch emits no qualifier at all —
/// not `"unknown"`, not a guess at `"main"`. Same rule as FR-3 freshness.
#[test]
fn an_undeterminable_branch_emits_no_qualifier() {
    for branch in [None, Some(""), Some("   ")] {
        let out = qualify(BranchModel::Qualifier, PROJECTION, branch).unwrap();
        assert_eq!(
            out, PROJECTION,
            "branch {branch:?} must leave the projection untouched"
        );
        assert!(!out.contains("onBranch"));
    }
}

/// `named_graph` REFUSES and names the blocker. It must not silently degrade to
/// the qualifier: an operator who set the preferred model would otherwise
/// believe their branches were partitioned when nothing partitions them.
#[test]
fn named_graph_refuses_loudly_and_names_quipu_36() {
    let err = qualify(BranchModel::NamedGraph, PROJECTION, Some("main"))
        .unwrap_err()
        .to_string();
    assert!(err.contains("named_graph"), "must name the value: {err}");
    assert!(
        err.contains("quipu#36"),
        "must name the blocker so the reader need not go find it: {err}"
    );
    assert!(
        err.contains("qualifier"),
        "must name the working alternative: {err}"
    );
    // And it refuses even when there is no branch to qualify with, so the
    // refusal is about the MODEL, not about this particular promotion.
    assert!(qualify(BranchModel::NamedGraph, PROJECTION, None).is_err());
}

/// A branch name git allows but Turtle does not is escaped, not emitted raw —
/// an unescaped quote breaks the whole document, not just its own triple.
#[test]
fn a_hostile_branch_name_is_escaped() {
    let out = qualify(BranchModel::Qualifier, PROJECTION, Some("fix/\"quote\"")).unwrap();
    assert!(
        out.contains("bobbin:onBranch \"fix/\\\"quote\\\"\" ."),
        "{out}"
    );
}

/// `branch_for` answers for the two shapes promotion actually runs in, and
/// abstains otherwise.
#[test]
fn branch_for_answers_a_named_branch_and_abstains_on_a_bare_sha() {
    let dir = tempfile::tempdir().unwrap();
    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .arg("-C")
            .arg(dir.path())
            .args(args)
            .output()
            .unwrap();
    };
    git(&["init", "-q", "-b", "main"]);
    git(&["config", "user.email", "t@t"]);
    git(&["config", "user.name", "t"]);
    std::fs::write(dir.path().join("a.txt"), "one\n").unwrap();
    git(&["add", "-A"]);
    git(&["commit", "-qm", "one"]);
    let first = crate::git::head_commit(dir.path()).unwrap();
    std::fs::write(dir.path().join("a.txt"), "two\n").unwrap();
    git(&["add", "-A"]);
    git(&["commit", "-qm", "two"]);

    // The developer/hook shape: HEAD on an attached branch.
    assert_eq!(
        crate::git::branch_for(dir.path(), "HEAD").as_deref(),
        Some("main")
    );
    // The CI shape: the ref ARGUMENT names the branch.
    assert_eq!(
        crate::git::branch_for(dir.path(), "main").as_deref(),
        Some("main")
    );
    // A bare older SHA sits on `main` too, but git will not say which branch a
    // commit "belongs" to and neither will we — abstain.
    assert_eq!(crate::git::branch_for(dir.path(), &first), None);
    // Not a repo at all.
    let empty = tempfile::tempdir().unwrap();
    assert_eq!(crate::git::branch_for(empty.path(), "HEAD"), None);
}
