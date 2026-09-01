use super::super::grammar::{DeadEnd, PathLevel, ProjectedPath, StepSig, SubmittedStep};
use super::*;

fn sig(kind: &str) -> StepSig {
    StepSig {
        action_kind: kind.into(),
        target_class: "literal".into(),
    }
}

fn step(kind: &str) -> SubmittedStep {
    SubmittedStep {
        action_kind: Some(kind.into()),
        target_class: Some("literal".into()),
        label: None,
    }
}

fn path(level: PathLevel) -> ProjectedPath {
    ProjectedPath {
        grammar: "gp-grammar/1".into(),
        path: "http://ex/gp-deploy".into(),
        level,
        pattern: vec![sig("edit"), sig("run"), sig("verify")],
        dead_ends: vec![DeadEnd {
            sig: sig("cache"),
            note: Some("did not help the exemplars".into()),
        }],
        exemplars: vec!["http://ex/traj".into()],
        projected_at: Some("2026-08-20T00:00:00Z".into()),
    }
}

fn evaluated(outcome: CheckOutcome) -> PathCheckReport {
    match outcome {
        CheckOutcome::Evaluated(r) => *r,
        CheckOutcome::Refused { reason } => panic!("refused: {reason}"),
    }
}

fn refused(outcome: CheckOutcome) -> String {
    match outcome {
        CheckOutcome::Refused { reason } => reason,
        CheckOutcome::Evaluated(r) => panic!("evaluated: {r:?}"),
    }
}

// ---- refusals: never a clean report over nothing ---------------------------

#[test]
fn an_empty_path_registry_is_refused_not_reported_clean() {
    let reason = refused(check(
        &[],
        "http://ex/gp-deploy",
        &[],
        CheckMode::Plan,
        false,
    ));
    assert!(
        reason.contains("green light over a dead backend"),
        "{reason}"
    );
}

#[test]
fn an_undeclared_path_is_refused_naming_what_was_held() {
    let reason = refused(check(
        &[path(PathLevel::Advisory)],
        "http://ex/gp-other",
        &[],
        CheckMode::Plan,
        false,
    ));
    assert!(reason.contains("gp-other"), "{reason}");
    assert!(reason.contains("gp-deploy"), "{reason}");
}

#[test]
fn a_grammar_version_mismatch_is_unevaluated_not_approved() {
    let mut p = path(PathLevel::Blessed);
    p.grammar = "gp-grammar/2".into();
    let reason = refused(check(
        &[p],
        "http://ex/gp-deploy",
        &[step("edit")],
        CheckMode::Plan,
        true,
    ));
    assert!(reason.contains("gp-grammar/2"), "{reason}");
    assert!(reason.contains("UNEVALUATED"), "{reason}");
}

// ---- plan mode (FR-42) ------------------------------------------------------

#[test]
fn a_conforming_plan_reports_no_effect_and_its_freshness() {
    let r = evaluated(check(
        &[path(PathLevel::Advisory)],
        "http://ex/gp-deploy",
        &[step("edit"), step("test-prep"), step("run"), step("verify")],
        CheckMode::Plan,
        false,
    ));
    assert_eq!(r.matched, 3);
    assert_eq!(r.tier, "engine-state");
    assert_eq!(r.first_deviation, None);
    assert_eq!(r.effect, "none");
    assert_eq!(r.projected_at.as_deref(), Some("2026-08-20T00:00:00Z"));
    assert_eq!(r.exemplars, vec!["http://ex/traj".to_string()]);
}

#[test]
fn a_deviating_plan_names_the_first_deviation_point() {
    let r = evaluated(check(
        &[path(PathLevel::Advisory)],
        "http://ex/gp-deploy",
        &[step("edit"), step("verify")],
        CheckMode::Plan,
        false,
    ));
    // 'edit' matches, 'run' is never matched ('verify' cannot satisfy it),
    // so the plan leaves the path at pattern[1].
    assert_eq!(r.matched, 1);
    let d = r.first_deviation.expect("must deviate");
    assert_eq!(d.pattern_index, 1);
    assert_eq!(d.after_step, Some(0));
    assert_eq!(r.effect, "warn");
}

#[test]
fn advisory_paths_never_deny_even_when_asked() {
    let r = evaluated(check(
        &[path(PathLevel::Advisory)],
        "http://ex/gp-deploy",
        &[step("verify")],
        CheckMode::Plan,
        true,
    ));
    assert_eq!(r.effect, "warn", "advisory is the effect ceiling");
}

#[test]
fn blessed_paths_deny_only_on_opt_in() {
    let by_default = evaluated(check(
        &[path(PathLevel::Blessed)],
        "http://ex/gp-deploy",
        &[step("verify")],
        CheckMode::Plan,
        false,
    ));
    assert_eq!(by_default.effect, "warn");
    let opted = evaluated(check(
        &[path(PathLevel::Blessed)],
        "http://ex/gp-deploy",
        &[step("verify")],
        CheckMode::Plan,
        true,
    ));
    assert_eq!(opted.effect, "deny");
}

// ---- progress mode (FR-41) --------------------------------------------------

#[test]
fn progress_mode_never_deviates_and_never_denies() {
    // The same steps that deviate in plan mode are merely "1 of 3 so far" for
    // an open trajectory: a future step could still match 'run'.
    let r = evaluated(check(
        &[path(PathLevel::Blessed)],
        "http://ex/gp-deploy",
        &[step("edit"), step("verify")],
        CheckMode::Progress,
        true,
    ));
    assert_eq!(r.first_deviation, None);
    assert_eq!(r.matched, 1);
    assert_eq!(r.effect, "none");
}

#[test]
fn hazards_warn_even_in_progress_mode() {
    let r = evaluated(check(
        &[path(PathLevel::Advisory)],
        "http://ex/gp-deploy",
        &[step("edit"), step("cache")],
        CheckMode::Progress,
        false,
    ));
    assert_eq!(r.hazards.len(), 1);
    assert_eq!(r.hazards[0].step, 1);
    assert!(r.hazards[0]
        .note
        .as_deref()
        .unwrap()
        .contains("did not help"));
    assert_eq!(r.effect, "warn");
}

// ---- honesty: unevaluated is reported, never dropped ------------------------

#[test]
fn unevaluable_steps_are_listed_on_the_verdict() {
    let blind = SubmittedStep {
        action_kind: None,
        target_class: None,
        label: None,
    };
    let r = evaluated(check(
        &[path(PathLevel::Advisory)],
        "http://ex/gp-deploy",
        &[blind, step("edit"), step("run"), step("verify")],
        CheckMode::Plan,
        false,
    ));
    assert_eq!(r.unevaluated_steps, vec![0]);
    assert_eq!(r.first_deviation, None, "missing data is not misconduct");
}

#[test]
fn freshness_is_omitted_rather_than_faked() {
    let mut p = path(PathLevel::Advisory);
    p.projected_at = None;
    let r = evaluated(check(
        &[p],
        "http://ex/gp-deploy",
        &[step("edit"), step("run"), step("verify")],
        CheckMode::Plan,
        false,
    ));
    assert_eq!(r.projected_at, None);
    let json = serde_json::to_string(&r).unwrap();
    assert!(!json.contains("projected_at"), "{json}");
}
