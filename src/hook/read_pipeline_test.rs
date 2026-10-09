use super::*;

fn payload() -> Value {
    serde_json::json!({"hook_event_name":"PostToolUse","session_id":"s"})
}
fn envelope(text: &str) -> Value {
    serde_json::json!({"continue":true,"hookSpecificOutput":{"hookEventName":"PostToolUse","additionalContext":text}})
}

#[test]
fn utf8_separator_and_prior_fields_preserved() {
    let prior = serde_json::to_vec(&envelope("é")).unwrap();
    let result = combine(payload(), &prior, true, Some(10), |p| {
        assert_eq!(p["remaining_context_bytes"], 7);
        Some(envelope("legend"))
    });
    let result: Value = serde_json::from_slice(&result).unwrap();
    assert_eq!(context(&result), Some("é\nlegend"));
    assert_eq!(result["continue"], true);
}

#[test]
fn empty_prior_has_one_capped_allocation() {
    let output = combine(payload(), b"", true, Some(9999), |p| {
        assert_eq!(p["remaining_context_bytes"], 600);
        Some(envelope("legend"))
    });
    assert_eq!(
        context(&serde_json::from_slice::<Value>(&output).unwrap()),
        Some("legend")
    );
}

#[test]
fn exhaustion_and_invalid_allocations_do_not_invoke_legend() {
    let prior = serde_json::to_vec(&envelope("é")).unwrap();
    for cap in [None, Some(0), Some(2), Some(3)] {
        assert_eq!(
            combine(payload(), &prior, true, cap, |_| panic!("no allocation")),
            prior
        );
    }
}

#[test]
fn every_legend_failure_preserves_prior_exactly() {
    let prior = serde_json::to_vec(&envelope("signpost")).unwrap();
    for reply in [
        None,
        Some(Value::Null),
        Some(serde_json::json!({})),
        Some(envelope("")),
        Some(envelope(&"x".repeat(601))),
    ] {
        assert_eq!(
            combine(payload(), &prior, true, Some(600), |_| reply),
            prior
        );
    }
}

#[test]
fn flag_off_failed_payload_and_invalid_prior_never_invoke_legend() {
    let prior = b"exact bytes\n";
    assert_eq!(
        combine(payload(), prior, false, Some(600), |_| panic!()),
        prior
    );
    for input in [
        Value::Null,
        serde_json::json!({"hook_event_name":"PostToolUseFailure"}),
    ] {
        assert_eq!(combine(input, prior, true, Some(600), |_| panic!()), prior);
    }
    for prior in [b"bad".as_slice(), b"[]", b"{}"] {
        assert_eq!(
            combine(payload(), prior, true, Some(600), |_| panic!()),
            prior
        );
    }
}

#[cfg(unix)]
#[test]
fn actual_subprocess_input_timeout_failure_and_output_cap() {
    let mut command = Command::new("cat");
    assert_eq!(
        invoke(&mut command, b"input bytes", Duration::from_secs(1)).unwrap(),
        b"input bytes"
    );
    let mut command = Command::new("false");
    assert!(invoke(&mut command, b"", Duration::from_secs(1)).is_none());
    let mut command = Command::new("sleep");
    command.arg("1");
    let start = Instant::now();
    assert!(invoke(&mut command, b"", Duration::from_millis(20)).is_none());
    assert!(start.elapsed() < Duration::from_millis(500));
    let mut command = Command::new("cat");
    assert!(invoke(&mut command, &vec![b'x'; 65537], Duration::from_secs(1)).is_none());
    let mut command = Command::new("/missing/pipeline-test");
    assert!(invoke(&mut command, b"", Duration::from_secs(1)).is_none());
}
