use super::*;

fn rule() -> TrajectoryPolicy {
    TrajectoryPolicy {
        id: "https://example.org/policy/delegate".into(),
        label: "delegate line".into(),
        trigger: InvocationTrigger {
            programs: vec!["br".into(), "bd".into()],
            verbs: vec!["create".into()],
        },
        ordering: "command-before-edit".into(),
        tier: "warn".into(),
        once_per: OncePer::Session,
        rationale: "A governed explanation from the graph.".into(),
        effect: "warn".into(),
        verification_point: "PAA".into(),
    }
}

#[test]
fn four_arms_include_the_silent_control() {
    let session = super::super::unique_test_session("trajectory");
    let rules = vec![rule()];
    assert_eq!(advice(Some(&session), &rules), None);
    record_command(Some(&session), "br create 'a title'", &rules);
    let text = advice(Some(&session), &rules).expect("filed before edit");
    assert!(text.contains(&rules[0].rationale));
    assert_eq!(advice(Some(&session), &rules), None);
}

#[test]
fn ordinary_existing_item_traffic_does_not_arm_advice() {
    let session = super::super::unique_test_session("trajectory-existing");
    let rules = vec![rule()];
    for command in [
        "br comments add item --file note",
        "br close item",
        "br update item",
        "br list",
        "br show item",
    ] {
        record_command(Some(&session), command, &rules);
    }
    assert_eq!(advice(Some(&session), &rules), None);
}

#[test]
fn parser_preserves_explicit_store_flags_chains_and_missed_flag_limit() {
    let trigger = rule().trigger;
    for command in [
        "br create x",
        "bd create x",
        "br --db /tmp/store create x",
        "cd /repo && br create x",
        "/opt/bin/br create x",
        "br --db=/tmp/store create x",
    ] {
        assert!(command_matches(command, &trigger), "{command}");
    }
    for command in [
        "echo 'br create x'",
        "git commit -m 'br create'",
        "br --db create list",
        "br --json create x",
        "",
    ] {
        assert!(!command_matches(command, &trigger), "{command}");
    }
}

#[test]
fn changing_only_rule_data_changes_trigger_and_frequency() {
    let session = super::super::unique_test_session("trajectory-custom");
    let mut policy = rule();
    policy.trigger = InvocationTrigger {
        programs: vec!["tracker".into()],
        verbs: vec!["file".into()],
    };
    policy.once_per = OncePer::Edit;
    let rules = vec![policy];
    record_command(Some(&session), "br create x", &rules);
    assert_eq!(advice(Some(&session), &rules), None);
    record_command(Some(&session), "tracker file x", &rules);
    assert!(advice(Some(&session), &rules).is_some());
    assert!(advice(Some(&session), &rules).is_some());
}

#[test]
fn changed_trigger_cannot_reuse_old_evidence_and_retirement_is_silent() {
    let session = super::super::unique_test_session("trajectory-change");
    let mut policy = rule();
    record_command(Some(&session), "br create x", &[policy.clone()]);
    policy.trigger.verbs = vec!["file".into()];
    assert_eq!(advice(Some(&session), &[policy]), None);
    assert_eq!(advice(Some(&session), &[]), None);
}

#[test]
fn missing_session_does_not_invent_trajectory_evidence() {
    let rules = vec![rule()];
    record_command(None, "br create x", &rules);
    assert_eq!(advice(None, &rules), None);
}
