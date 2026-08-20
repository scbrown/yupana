use super::*;

fn sig(kind: &str, class: &str) -> StepSig {
    StepSig {
        action_kind: kind.into(),
        target_class: class.into(),
    }
}

fn step(kind: &str) -> SubmittedStep {
    SubmittedStep {
        action_kind: Some(kind.into()),
        target_class: Some("literal".into()),
        label: None,
    }
}

fn blind() -> SubmittedStep {
    SubmittedStep {
        action_kind: None,
        target_class: None,
        label: Some("no action kind recorded".into()),
    }
}

fn pat(kinds: &[&str]) -> Vec<StepSig> {
    kinds.iter().map(|k| sig(k, "literal")).collect()
}

#[test]
fn a_plan_with_gaps_still_conforms() {
    let m = match_plan(
        &pat(&["edit", "verify"]),
        &[step("read"), step("edit"), step("run"), step("verify")],
    );
    assert_eq!(m.matched, 2);
    assert_eq!(m.first_deviation, None);
}

#[test]
fn order_is_not_negotiable_and_the_anchor_is_named() {
    let m = match_plan(&pat(&["run", "edit"]), &[step("edit"), step("run")]);
    assert_eq!(m.matched, 1);
    let d = m.first_deviation.expect("must deviate");
    assert_eq!(d.pattern_index, 1);
    // The 'run' pattern element matched submitted step 1; 'edit' then has
    // nothing left to match.
    assert_eq!(d.after_step, Some(1));
}

#[test]
fn a_plan_matching_nothing_anchors_nowhere() {
    let m = match_plan(&pat(&["verify"]), &[step("read")]);
    let d = m.first_deviation.expect("must deviate");
    assert_eq!(d.pattern_index, 0);
    assert_eq!(d.after_step, None);
}

#[test]
fn unevaluable_steps_neither_match_nor_deviate() {
    let m = match_plan(&pat(&["edit"]), &[blind(), step("edit")]);
    assert_eq!(m.matched, 1);
    assert_eq!(m.first_deviation, None);
}

#[test]
fn a_missing_target_class_reads_as_none_not_as_a_wildcard() {
    // The pattern expects (edit, literal); a submitted step with no target
    // class has signature (edit, none) and must NOT match.
    let bare = SubmittedStep {
        action_kind: Some("edit".into()),
        target_class: None,
        label: None,
    };
    let m = match_plan(&pat(&["edit"]), &[bare]);
    assert_eq!(m.matched, 0);
}

#[test]
fn hazards_name_the_step_and_carry_the_note() {
    let dead = vec![DeadEnd {
        sig: sig("edit", "literal"),
        note: Some("exemplars tried a cache here; it did not help".into()),
    }];
    let hits = hazards(&dead, &[step("read"), step("edit")]);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].0, 1);
    assert!(hits[0].1.as_deref().unwrap().contains("cache"));
}

#[test]
fn the_projected_path_serialization_round_trips() {
    // The wire shape pinned in quipu's conformance-grammar.md.
    let json = r#"{
        "grammar": "gp-grammar/1",
        "path": "http://ex/gp-deploy",
        "level": "advisory",
        "pattern": [{"action_kind": "edit", "target_class": "literal"}],
        "dead_ends": [{"action_kind": "edit", "target_class": "none", "note": "n"}],
        "exemplars": ["http://ex/traj"],
        "projected_at": "2026-08-20T00:00:00Z"
    }"#;
    let p: ProjectedPath = serde_json::from_str(json).unwrap();
    assert_eq!(p.grammar, GRAMMAR_VERSION);
    assert_eq!(p.level, PathLevel::Advisory);
    assert_eq!(p.dead_ends[0].sig.target_class, "none");
    let back = serde_json::to_string(&p).unwrap();
    assert!(back.contains("gp-grammar/1"));
}

#[test]
fn constraint_backing_does_not_parse_as_a_level() {
    // L5 is gated on verdict signing; an unsigned constraint-backing path
    // must fail to parse rather than enforce as if it were signed.
    let json = r#"{"grammar": "gp-grammar/1", "path": "p", "level": "constraint-backing",
                   "pattern": []}"#;
    assert!(serde_json::from_str::<ProjectedPath>(json).is_err());
}
