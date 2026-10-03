//! Translate native Codex patches into per-file pre-edit inputs without writing.
//!
//! Matching is deliberately exact. Unsupported/fuzzy/ambiguous patches return
//! UNKNOWN to the caller, never a fabricated clean evaluation. Added text is
//! kept separately from the reconstructed buffer so unchanged context cannot
//! trigger an introduced-text policy.
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

pub(super) fn inputs(payload: &Value, root: &Path) -> Result<Vec<String>, &'static str> {
    let patch = payload["tool_input"]["command"]
        .as_str()
        .ok_or("missing patch")?;
    let lines: Vec<&str> = patch.lines().collect();
    if lines.first() != Some(&"*** Begin Patch") || lines.last() != Some(&"*** End Patch") {
        return Err("unsupported patch envelope");
    }
    let mut out = Vec::new();
    let mut paths = std::collections::HashSet::new();
    let mut pos = 1;
    while pos < lines.len() - 1 {
        let header = lines[pos];
        let (operation, name) = ["Add", "Update", "Delete"]
            .into_iter()
            .find_map(|op| {
                header
                    .strip_prefix(&format!("*** {op} File: "))
                    .map(|name| (op, name))
            })
            .ok_or("unsupported file operation")?;
        if name.is_empty() {
            return Err("empty path");
        }
        let path = absolute(root, name);
        pos += 1;
        let mut destination = path.clone();
        if operation == "Update" {
            if let Some(name) = lines.get(pos).and_then(|s| s.strip_prefix("*** Move to: ")) {
                if name.is_empty() {
                    return Err("empty move destination");
                }
                destination = absolute(root, name);
                pos += 1;
            }
        }
        if !paths.insert(path.clone())
            || (destination != path && !paths.insert(destination.clone()))
        {
            return Err("repeated target path");
        }
        let start = pos;
        while pos < lines.len() - 1 && !is_file_header(lines[pos]) {
            pos += 1;
        }
        let body = &lines[start..pos];
        let (content, added, edits) = match operation {
            "Add" => {
                let mut text = String::new();
                for line in body {
                    text.push_str(line.strip_prefix('+').ok_or("invalid add line")?);
                    text.push('\n');
                }
                (text.clone(), text, Vec::new())
            }
            "Delete" => {
                if !body.is_empty() {
                    return Err("unexpected delete body");
                }
                let old = std::fs::read_to_string(&path).map_err(|_| "unreadable deleted file")?;
                (
                    String::new(),
                    String::new(),
                    vec![json!({"old_string": old, "new_string":""})],
                )
            }
            _ => {
                let old = std::fs::read_to_string(&path).map_err(|_| "unreadable updated file")?;
                update(&old, body)?
            }
        };
        if destination != path {
            // A move changes BOTH scopes. Check removal at the source, then
            // the complete arriving content at the destination.
            out.push(input(payload, &path, "", "", &edits));
            out.push(input(payload, &destination, &content, &content, &[]));
        } else {
            out.push(input(payload, &path, &content, &added, &edits));
        }
    }
    if out.is_empty() {
        return Err("empty patch");
    }
    Ok(out)
}

fn absolute(root: &Path, name: &str) -> PathBuf {
    let path = Path::new(name);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    }
}

fn is_file_header(line: &str) -> bool {
    ["*** Add File: ", "*** Update File: ", "*** Delete File: "]
        .iter()
        .any(|s| line.starts_with(s))
}

fn input(payload: &Value, path: &Path, content: &str, added: &str, edits: &[Value]) -> String {
    let mut normalized = payload.clone();
    normalized["tool_input"] = json!({"file_path":path, "content":content,
        "new_string":added, "edits":edits});
    normalized.to_string()
}

fn update(old: &str, body: &[&str]) -> Result<(String, String, Vec<Value>), &'static str> {
    let mut source: Vec<String> = old.lines().map(str::to_owned).collect();
    let mut cursor = 0;
    let mut pos = 0;
    let mut added = String::new();
    let mut edits = Vec::new();
    while pos < body.len() {
        if body[pos] == "@@" || body[pos].starts_with("@@ ") {
            pos += 1;
        }
        let mut before = Vec::new();
        let mut after = Vec::new();
        let mut eof = false;
        let start = pos;
        while pos < body.len() && body[pos] != "@@" && !body[pos].starts_with("@@ ") {
            let line = body[pos];
            pos += 1;
            if line == "*** End of File" {
                eof = true;
                if pos != body.len() {
                    return Err("end marker before final hunk");
                }
                break;
            }
            match line.as_bytes().first() {
                Some(b' ') => {
                    before.push(line[1..].to_owned());
                    after.push(line[1..].to_owned());
                }
                Some(b'-') => before.push(line[1..].to_owned()),
                Some(b'+') => {
                    after.push(line[1..].to_owned());
                    added.push_str(&line[1..]);
                    added.push('\n');
                }
                _ => return Err("unsupported hunk line"),
            }
        }
        if pos == start {
            return Err("empty hunk");
        }
        let at = if before.is_empty() {
            // Codex's context-free insertion appends to the file.
            source.len()
        } else {
            let hits: Vec<usize> = (cursor..=source.len().saturating_sub(before.len()))
                .filter(|&i| source.get(i..i + before.len()) == Some(before.as_slice()))
                .filter(|&i| !eof || i + before.len() == source.len())
                .collect();
            if hits.len() != 1 {
                return Err("hunk anchor missing or ambiguous");
            }
            hits[0]
        };
        edits.push(json!({"old_string":before.join("\n"), "new_string":after.join("\n")}));
        cursor = at + after.len();
        source.splice(at..at + before.len(), after);
    }
    if edits.is_empty() {
        return Err("update without hunks");
    }
    let mut content = source.join("\n");
    if !content.is_empty() {
        content.push('\n');
    }
    Ok((content, added, edits))
}

#[cfg(test)]
#[path = "codex_patch_test.rs"]
mod tests;
