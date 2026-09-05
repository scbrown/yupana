use super::*;

fn wire(value: &serde_json::Value) -> String {
    let rows: Vec<serde_json::Value> = value["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| {
            let map: serde_json::Map<String, serde_json::Value> = row
                .as_object()
                .unwrap()
                .iter()
                .map(|(key, value)| {
                    (
                        key.clone(),
                        serde_json::json!({"type":"literal", "value":value}),
                    )
                })
                .collect();
            serde_json::Value::Object(map)
        })
        .collect();
    serde_json::json!({"results":{"bindings":rows}}).to_string()
}

fn body() -> serde_json::Value {
    serde_json::json!({"rows":[{
        "policy":"https://example.org/policy/delegate",
        "label":"delegate line",
        "trigger":r#"{"programs":["br","bd"],"verbs":["create"]}"#,
        "ordering":"command-before-edit",
        "tier":"warn",
        "oncePer":"session",
        "effect":"warn", "point":"PAA",
        "rationale":"Check whether the subsequent work belongs to this session's assigned item."
    }]})
}

#[test]
fn projects_trigger_tier_scope_and_text_from_data() {
    let mut value = body();
    value["rows"][0]["trigger"] = serde_json::json!(r#"{"programs":["tracker"],"verbs":["file"]}"#);
    value["rows"][0]["oncePer"] = serde_json::json!("edit");
    let policies = decode_trajectory_policies(&wire(&value)).unwrap();
    assert_eq!(policies[0].trigger.programs, ["tracker"]);
    assert_eq!(policies[0].trigger.verbs, ["file"]);
    assert_eq!(policies[0].once_per, OncePer::Edit);
    assert_eq!(policies[0].tier, "warn");
}

#[test]
fn block_is_refused_with_the_missing_enforcement_point_named() {
    let mut value = body();
    value["rows"][0]["tier"] = serde_json::json!("block");
    let error = decode_trajectory_policies(&wire(&value))
        .unwrap_err()
        .to_string();
    assert!(error.contains("pre-edit enforcement point"), "{error}");
}

#[test]
fn invalid_or_missing_policy_data_is_never_silently_dropped() {
    for key in [
        "trigger",
        "ordering",
        "tier",
        "oncePer",
        "rationale",
        "effect",
        "point",
    ] {
        let mut value = body();
        value["rows"][0].as_object_mut().unwrap().remove(key);
        assert!(decode_trajectory_policies(&wire(&value)).is_err(), "{key}");
    }
    for (key, invalid) in [
        ("trigger", r#"{"programs":[],"verbs":["create"]}"#),
        (
            "trigger",
            r#"{"programs":["br"],"verbs":["create"],"ignored":true}"#,
        ),
        ("ordering", "edit-before-command"),
        ("effect", "deny"),
        ("point", "PAG"),
        ("tier", "unknown"),
        ("oncePer", "forever"),
        ("rationale", ""),
    ] {
        let mut value = body();
        value["rows"][0][key] = serde_json::json!(invalid);
        assert!(
            decode_trajectory_policies(&wire(&value)).is_err(),
            "{key} {invalid}"
        );
    }
}

#[test]
fn conflicts_refuse_and_identical_rows_deduplicate() {
    let mut value = body();
    let row = value["rows"][0].clone();
    value["rows"].as_array_mut().unwrap().push(row);
    assert_eq!(decode_trajectory_policies(&wire(&value)).unwrap().len(), 1);
    value["rows"][1]["oncePer"] = serde_json::json!("edit");
    assert!(decode_trajectory_policies(&wire(&value)).is_err());
}

#[test]
fn empty_is_distinct_from_unknown_response() {
    assert!(decode_trajectory_policies(r#"{"results":{"bindings":[]}}"#)
        .unwrap()
        .is_empty());
    assert!(decode_trajectory_policies("{}").is_err());
}
