use super::*;
use serde_json::json;

fn response(text: &str) -> Value {
    json!({"type":"text", "file":{"filePath":"/fixture.txt", "content":text,
        "startLine":1,"numLines":2,"totalLines":2}})
}

fn request(id: &str) -> Value {
    json!({"sessionId":"session", "message":{"content":[{
        "type":"tool_use","name":"Read","id":id,
        "input":{"file_path":"/fixture.txt"}}]}})
}

fn result(id: &str, text: &str) -> Value {
    json!({"sessionId":"session", "toolUseResult":response(text),
        "message":{"content":[{"type":"tool_result","tool_use_id":id,"content":text}]}})
}

fn payload() -> Value {
    json!({"session_id":"session", "tool_name":"Read","tool_use_id":"second",
        "tool_input":{"file_path":"/fixture.txt"},"tool_response":response("alpha\nbeta")})
}

fn evaluate_records(p: &Value, records: &[Value]) -> Verdict {
    let records = linked(records);
    evaluate(
        p,
        &records
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

fn linked(records: &[Value]) -> Vec<Value> {
    records
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let mut r = r.clone();
            r["uuid"] = format!("record-{i}").into();
            r["parentUuid"] = if i == 0 {
                Value::Null
            } else {
                format!("record-{}", i - 1).into()
            };
            r
        })
        .collect()
}

fn baseline() -> Vec<Value> {
    vec![
        request("first"),
        result("first", "alpha\nbeta"),
        request("second"),
    ]
}

#[test]
fn advises_current_repeat_before_or_after_result_flush() {
    let mut records = baseline();
    assert_eq!(evaluate_records(&payload(), &records), Verdict::Candidate);
    records.push(result("second", "alpha\nbeta"));
    assert_eq!(evaluate_records(&payload(), &records), Verdict::Candidate);
    // Old candidates must never be mistaken for the current request.
    let mut p = payload();
    p["tool_use_id"] = "absent".into();
    assert_eq!(evaluate_records(&p, &records), Verdict::Unknown);
}

#[test]
fn new_content_or_region_is_legitimate() {
    for (name, value) in [
        ("content", json!("changed")),
        ("startLine", json!(2)),
        ("numLines", json!(3)),
    ] {
        let mut p = payload();
        p["tool_response"]["file"][name] = value;
        assert_eq!(evaluate_records(&p, &baseline()), Verdict::NoMatch);
    }
    let mut records = baseline();
    records[0]["message"]["content"][0]["input"]["limit"] = 2.into();
    assert_eq!(evaluate_records(&payload(), &records), Verdict::NoMatch);
}

#[test]
fn edit_compaction_and_possible_shell_writes_invalidate() {
    let events = [
        json!({"isCompactSummary":true}),
        json!({"subtype":"compact_boundary"}),
        json!({"type":"compacted"}),
        json!({"message":{"content":[{"type":"tool_use","name":"Edit","input":{"file_path":"/fixture.txt"}}]}}),
        json!({"message":{"content":[{"type":"tool_use","name":"Bash","input":{"command":"run-script"}}]}}),
    ];
    for mut event in events {
        event["sessionId"] = "session".into();
        let mut records = baseline();
        records.insert(2, event.clone());
        assert_eq!(evaluate_records(&payload(), &records), Verdict::NoMatch);
        let mut records = baseline();
        records.push(event);
        assert_eq!(evaluate_records(&payload(), &records), Verdict::Unknown);
    }
}

#[test]
fn first_missing_failed_image_and_parallel_reads_are_not_candidates() {
    assert_eq!(
        evaluate_records(&payload(), &[request("second")]),
        Verdict::NoMatch
    );
    assert_eq!(
        evaluate_records(&payload(), &[request("first"), request("second")]),
        Verdict::NoMatch
    );
    let parallel = [
        request("first"),
        request("second"),
        result("first", "alpha\nbeta"),
    ];
    assert_eq!(evaluate_records(&payload(), &parallel), Verdict::NoMatch);
    let mut records = baseline();
    records[1]["message"]["content"][0]["is_error"] = true.into();
    assert_eq!(evaluate_records(&payload(), &records), Verdict::NoMatch);
    records[1] = result("first", "alpha\nbeta");
    records[1]["toolUseResult"]["type"] = "image".into();
    assert_eq!(evaluate_records(&payload(), &records), Verdict::NoMatch);
    let mut p = payload();
    p["tool_response"]["type"] = "image".into();
    assert_eq!(evaluate_records(&p, &baseline()), Verdict::Unknown);
    let mut records = baseline();
    let mut failed = result("second", "alpha\nbeta");
    failed["message"]["content"][0]["is_error"] = true.into();
    records.push(failed);
    assert_eq!(evaluate_records(&payload(), &records), Verdict::Unknown);
}

#[test]
fn missing_identity_wrong_session_and_subagent_evidence_are_unknown() {
    for field in ["session_id", "tool_use_id"] {
        let mut p = payload();
        p.as_object_mut().unwrap().remove(field);
        assert_eq!(evaluate_records(&p, &baseline()), Verdict::Unknown);
    }
    let mut records = baseline();
    for r in &mut records {
        r["sessionId"] = "another-session".into();
    }
    assert_eq!(evaluate_records(&payload(), &records), Verdict::Unknown);
    let mut records = baseline();
    records[0]["isSidechain"] = true.into();
    records[1]["isSidechain"] = true.into();
    assert_eq!(evaluate_records(&payload(), &records), Verdict::NoMatch);
}

#[test]
fn malformed_records_and_missing_structured_returns_invalidate() {
    let records = baseline();
    let transcript = format!("{}\n{}\nnot-json\n{}", records[0], records[1], records[2]);
    assert_eq!(evaluate(&payload(), &transcript), Verdict::Unknown);
    let mut records = baseline();
    records[1].as_object_mut().unwrap().remove("toolUseResult");
    assert_eq!(evaluate_records(&payload(), &records), Verdict::NoMatch);
    let mut records = baseline();
    records[1]["toolUseResult"]["file"]["filePath"] = "/different.txt".into();
    assert_eq!(evaluate_records(&payload(), &records), Verdict::NoMatch);
}

#[test]
fn task_polling_stays_silent() {
    let mut p = payload();
    p["tool_input"]["file_path"] = "/tmp/tasks/job.output".into();
    assert_eq!(evaluate_records(&p, &baseline()), Verdict::NoMatch);
}

#[test]
fn bounded_file_reader_rejects_missing_and_oversize_evidence() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("transcript.jsonl");
    let p = json!({"transcript_path":file});
    assert!(read_transcript(&p).is_none());
    std::fs::write(&file, "{}").unwrap();
    assert_eq!(read_transcript(&p).as_deref(), Some("{}"));
    std::fs::OpenOptions::new()
        .write(true)
        .open(&file)
        .unwrap()
        .set_len(MAX_TRANSCRIPT + 1)
        .unwrap();
    assert!(read_transcript(&p).is_none());
}

#[test]
fn rewound_branches_and_missing_lineage_cannot_supply_context() {
    let mut records = linked(&baseline());
    records[2]["parentUuid"] = "record-0".into(); // completed result is on the abandoned branch
    let text = records
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    assert_eq!(evaluate(&payload(), &text), Verdict::NoMatch);
    records[2]["parentUuid"] = "missing".into();
    let text = records
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    assert_eq!(evaluate(&payload(), &text), Verdict::Unknown);
}
