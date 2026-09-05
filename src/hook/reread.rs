//! Result-aware Claude Read advice. Transcript evidence is never proof of
//! retention: an unreported eviction remains unobservable. No file contents or
//! paths leave this module, and no persistent copy of returned text is kept.

use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::io::Read;

// Bound hook I/O and memory. Oversize or unreadable evidence is UNKNOWN, not a
// clean pass. Starting in the middle could silently omit a context boundary.
const MAX_TRANSCRIPT: u64 = 16 * 1024 * 1024;

#[derive(Clone, PartialEq)]
struct TextRead {
    digest: Vec<u8>,
    start: u64,
    lines: u64,
}

fn text_read(value: &Value, path: &str) -> Option<TextRead> {
    if value.get("type")?.as_str()? != "text" {
        return None;
    }
    let file = value.get("file")?;
    if file.get("filePath")?.as_str()? != path {
        return None;
    }
    let content = file.get("content")?.as_str()?;
    if content.trim().is_empty() {
        return None;
    }
    Some(TextRead {
        digest: Sha256::digest(content.as_bytes()).to_vec(),
        start: file.get("startLine")?.as_u64()?,
        lines: file.get("numLines")?.as_u64()?,
    })
}

fn boundary(record: &Value) -> bool {
    record.get("isCompactSummary").and_then(Value::as_bool) == Some(true)
        || record.get("compact").and_then(Value::as_bool) == Some(true)
        || record.get("type").and_then(Value::as_str) == Some("compacted")
        || matches!(
            record.get("subtype").and_then(Value::as_str),
            Some("compact_boundary" | "compaction" | "compact")
        )
        || record
            .pointer("/message/isCompactSummary")
            .and_then(Value::as_bool)
            == Some(true)
}

fn polling(path: &str) -> bool {
    path.replace('\\', "/").split('/').any(|p| p == "tasks") && path.ends_with(".output")
}

/// No match is not a statement about context health. UNKNOWN names missing
/// evidence in telemetry; only a candidate is injected into the model.
#[derive(Debug, PartialEq)]
enum Verdict {
    Candidate,
    NoMatch,
    Unknown,
}

/// Follow the current request's parent UUIDs. Chronological order alone can
/// borrow text from an abandoned conversation branch after a rewind.
fn active_records<'a>(transcript: &'a str, request_id: &str) -> Option<Vec<&'a str>> {
    let lines: Vec<_> = transcript.lines().collect();
    let records: Vec<Value> = lines
        .iter()
        .map(|line| serde_json::from_str(line))
        .collect::<Result<_, _>>()
        .ok()?;
    let mut by_uuid = HashMap::new();
    let mut current = None;
    for (i, r) in records.iter().enumerate() {
        if let Some(uuid) = r.get("uuid").and_then(Value::as_str) {
            if by_uuid.insert(uuid, i).is_some() {
                return None;
            }
        }
        if r.pointer("/message/content")
            .and_then(Value::as_array)
            .is_some_and(|items| {
                items.iter().any(|b| {
                    b.get("type").and_then(Value::as_str) == Some("tool_use")
                        && b.get("id").and_then(Value::as_str) == Some(request_id)
                })
            })
            && current.replace(i).is_some()
        {
            return None;
        }
    }
    let current = current?;
    let mut cursor = current;
    let mut active = HashSet::new();
    loop {
        if !active.insert(cursor) {
            return None;
        }
        let record = &records[cursor];
        record.get("uuid")?.as_str()?;
        if boundary(record) {
            break;
        }
        let parent = record.get("parentUuid")?;
        if parent.is_null() {
            break;
        }
        let previous = *by_uuid.get(parent.as_str()?)?;
        if previous >= cursor {
            return None;
        }
        cursor = previous;
    }
    Some(
        lines
            .into_iter()
            .enumerate()
            .filter_map(|(i, line)| (active.contains(&i) || i > current).then_some(line))
            .collect(),
    )
}

/// Evaluate only the current completed Read, never replay old warnings. A prior
/// response must precede the current request, including when the current result
/// has not yet been flushed to the transcript at `PostToolUse` time.
fn evaluate(payload: &Value, transcript: &str) -> Verdict {
    let Some(session) = payload
        .get("session_id")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
    else {
        return Verdict::Unknown;
    };
    let Some(id) = payload
        .get("tool_use_id")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
    else {
        return Verdict::Unknown;
    };
    let Some(path) = payload
        .pointer("/tool_input/file_path")
        .and_then(Value::as_str)
    else {
        return Verdict::Unknown;
    };
    if polling(path) {
        return Verdict::NoMatch;
    }
    let Some(current) = text_read(&payload["tool_response"], path) else {
        return Verdict::Unknown;
    };
    let mut pending = HashMap::new();
    let mut prior: Option<(Value, TextRead)> = None;
    let mut at_request = None;
    let Some(active) = active_records(transcript, id) else {
        return Verdict::Unknown;
    };
    for line in active {
        let Ok(record) = serde_json::from_str::<Value>(line) else {
            pending.clear();
            prior = None;
            at_request = None;
            continue;
        };
        // Never borrow evidence from a different session or a subagent.
        if record.get("isSidechain").and_then(Value::as_bool) == Some(true) {
            continue;
        }
        let record_session = record.get("sessionId").or_else(|| record.get("session_id"));
        if record_session.and_then(Value::as_str) != Some(session) {
            pending.clear();
            prior = None;
            at_request = None;
            continue;
        }
        if boundary(&record) {
            pending.clear();
            prior = None;
            at_request = None;
            continue;
        }
        let Some(content) = record.pointer("/message/content").and_then(Value::as_array) else {
            continue;
        };
        for block in content {
            match block.get("type").and_then(Value::as_str) {
                Some("tool_use") => {
                    let args = &block["input"];
                    let name = block.get("name").and_then(Value::as_str);
                    let targets_file = args.get("file_path").and_then(Value::as_str) == Some(path)
                        || args.get("notebook_path").and_then(Value::as_str) == Some(path);
                    // Shell commands can write via aliases, subprocesses or a
                    // script whose body is absent here. Invalidate on every
                    // Bash call rather than infer read-only from its spelling.
                    if name == Some("Bash")
                        || (targets_file
                            && matches!(
                                name,
                                Some("Edit" | "Write" | "MultiEdit" | "NotebookEdit")
                            ))
                    {
                        prior = None;
                        pending.clear();
                        at_request = None;
                    }
                    if name != Some("Read") || !targets_file {
                        continue;
                    }
                    let Some(request_id) = block.get("id").and_then(Value::as_str) else {
                        continue;
                    };
                    if request_id == id {
                        // Require exact arguments as well as the actual returned
                        // span: overlapping regions can include previously unseen lines.
                        if args != &payload["tool_input"] {
                            return Verdict::Unknown;
                        }
                        at_request = Some(
                            prior
                                .as_ref()
                                .is_some_and(|(input, text)| input == args && text == &current),
                        );
                    } else {
                        pending.insert(request_id.to_string(), args.clone());
                    }
                }
                Some("tool_result") => {
                    let Some(result_id) = block.get("tool_use_id").and_then(Value::as_str) else {
                        continue;
                    };
                    if result_id == id {
                        if block.get("is_error").and_then(Value::as_bool) == Some(true) {
                            return Verdict::Unknown;
                        }
                        return verdict(at_request);
                    }
                    if let Some(args) = pending.remove(result_id) {
                        // toolUseResult is the harness's structured return, the
                        // same schema used in PostToolUse.tool_response. A text
                        // request or a bare tool_result alone proves nothing.
                        prior = if block.get("is_error").and_then(Value::as_bool) == Some(true) {
                            None
                        } else {
                            text_read(&record["toolUseResult"], path).map(|text| (args, text))
                        };
                    }
                }
                _ => {}
            }
        }
    }
    verdict(at_request)
}

fn verdict(at_request: Option<bool>) -> Verdict {
    match at_request {
        Some(true) => Verdict::Candidate,
        Some(false) => Verdict::NoMatch,
        None => Verdict::Unknown,
    }
}

pub(super) fn advisory(input_json: &str) -> Option<String> {
    let payload: Value = serde_json::from_str(input_json).ok()?;
    if payload.get("tool_name").and_then(Value::as_str) != Some("Read") {
        return None;
    }
    let outcome =
        read_transcript(&payload).map_or(Verdict::Unknown, |text| evaluate(&payload, &text));
    let label = match outcome {
        Verdict::Candidate => "candidate",
        Verdict::NoMatch => "no_match",
        Verdict::Unknown => "unknown",
    };
    let identity = format!("{}:{}", payload["session_id"], payload["tool_use_id"]);
    let observation = hex::encode(Sha256::digest(identity.as_bytes()));
    crate::metrics::emit(
        "reread_evaluated",
        &[
            ("result", label.into()),
            ("observation", observation.into()),
        ],
    );
    (outcome == Verdict::Candidate).then(|| String::from(
        "Yupana reread candidate: this Read returned the same text and region as a completed read before this request, with no recorded edit or compaction between them. Reuse the earlier result if it is still in context. Unreported eviction cannot be ruled out; a necessary context refresh is legitimate. Advisory only.",
    ))
}

fn read_transcript(payload: &Value) -> Option<String> {
    let path = payload.get("transcript_path")?.as_str()?;
    let file = std::fs::File::open(path).ok()?;
    let metadata = file.metadata().ok()?;
    if !metadata.is_file() || metadata.len() > MAX_TRANSCRIPT {
        return None;
    }
    let mut text = String::new();
    file.take(MAX_TRANSCRIPT + 1)
        .read_to_string(&mut text)
        .ok()?;
    (text.len() as u64 <= MAX_TRANSCRIPT).then_some(text)
}

#[cfg(test)]
#[path = "reread_test.rs"]
mod tests;
