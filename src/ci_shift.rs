//! CI shift-left planning and background execution.
//!
//! Quipu owns the map: CI jobs point to local commands and gated path scopes.
//! Yupana only projects that graph, intersects it with a change, and executes
//! the selected commands asynchronously at the pre-push decision point.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::errors::{Error, Result};

/// Governed CI-map projection.
pub const CI_MAP_QUERY: &str = "\
PREFIX aegis: <http://aegis.gastown.local/ontology/>\n\
PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\n\
SELECT ?job ?label ?command ?scope WHERE {\n\
  ?job a aegis:CiJob ; rdfs:label ?label ;\n\
       aegis:localEquivalent ?local ; aegis:gatesPath ?scope .\n\
  ?local a aegis:LocalCommand ; aegis:commandText ?command .\n\
}";

/// One CI job and the local command that faithfully exercises it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CiCheck {
    /// Stable graph IRI for the CI job.
    pub id: String,
    /// Reader-facing job or step name.
    pub label: String,
    /// Exact local command asserted equivalent by the graph.
    pub command: String,
    /// Repo-relative glob scopes gated by the job.
    pub scopes: Vec<String>,
}

/// Result of intersecting a diff with the governed CI map.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Selection {
    /// No governed map was projected; absence must not look like clean CI.
    Unknown,
    /// A real map exists and no job gates the changed paths.
    Quiet,
    /// These checks gate at least one changed path.
    Checks(Vec<CiCheck>),
}

/// Decode Quipu's SPARQL result into one check per CI job.
pub fn decode_map(body: &str) -> Result<Vec<CiCheck>> {
    let value: serde_json::Value = serde_json::from_str(body)
        .map_err(|e| Error::Projection(format!("CI-map results are not JSON: {e}")))?;
    let rows = value
        .get("results")
        .and_then(|r| r.get("bindings"))
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| {
            Error::Projection("CI-map results have no `results.bindings` array".into())
        })?;
    let mut checks: BTreeMap<String, CiCheck> = BTreeMap::new();
    for (i, row) in rows.iter().enumerate() {
        let required = |key: &str| {
            row.get(key)
                .and_then(|value| value.get("value"))
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
                .ok_or_else(|| {
                    Error::Projection(format!("CI-map row {i}: missing required binding `{key}`"))
                })
        };
        let id = required("job")?;
        let next = CiCheck {
            label: required("label")?,
            command: required("command")?,
            scopes: vec![required("scope")?],
            id: id.clone(),
        };
        match checks.get_mut(&id) {
            None => {
                checks.insert(id, next);
            }
            Some(existing) => {
                if existing.label != next.label || existing.command != next.command {
                    return Err(Error::Projection(format!(
                        "CI job `{id}` has conflicting labels or local commands"
                    )));
                }
                existing.scopes.extend(next.scopes);
                existing.scopes.sort();
                existing.scopes.dedup();
            }
        }
    }
    Ok(checks.into_values().collect())
}

/// Select the checks whose governed scope intersects `changed`.
#[must_use]
pub fn select(checks: &[CiCheck], changed: &[PathBuf]) -> Selection {
    if checks.is_empty() {
        return Selection::Unknown;
    }
    let selected: Vec<CiCheck> = checks
        .iter()
        .filter(|check| {
            check.scopes.iter().any(|scope| {
                glob::Pattern::new(scope).is_ok_and(|pattern| {
                    changed
                        .iter()
                        .any(|path| pattern.matches_path_with(path, glob::MatchOptions::new()))
                })
            })
        })
        .cloned()
        .collect();
    if selected.is_empty() {
        Selection::Quiet
    } else {
        Selection::Checks(selected)
    }
}

/// One completed local equivalent, written by a background worker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckResult {
    /// CI job label.
    pub label: String,
    /// Whether its local equivalent exited successfully.
    pub passed: bool,
}

/// Run selected checks in a background thread and atomically publish results.
///
/// The command text is governed Quipu data. Callers must pass only a validated
/// projection; this executor deliberately does not discover commands from a
/// mutable workspace file.
pub fn run_background(
    root: PathBuf,
    checks: Vec<CiCheck>,
    result_path: PathBuf,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let results: Vec<CheckResult> = checks
            .into_iter()
            .map(|check| CheckResult {
                passed: Command::new("sh")
                    .arg("-c")
                    .arg(&check.command)
                    .current_dir(&root)
                    .status()
                    .is_ok_and(|status| status.success()),
                label: check.label,
            })
            .collect();
        let Some(parent) = result_path.parent() else {
            return;
        };
        if std::fs::create_dir_all(parent).is_err() {
            return;
        }
        let tmp = result_path.with_extension("tmp");
        if serde_json::to_vec(&results)
            .ok()
            .is_some_and(|bytes| std::fs::write(&tmp, bytes).is_ok())
        {
            let _ = std::fs::rename(tmp, result_path);
        }
    })
}

/// Read the last asynchronously published results.
#[must_use]
pub fn read_results(path: &Path) -> Option<Vec<CheckResult>> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

/// Paths that a push would carry, including uncommitted edits visible to the
/// operator. An unresolved upstream is UNKNOWN to the caller, never quiet.
#[must_use]
pub fn changed_for_push(root: &Path) -> Option<Vec<PathBuf>> {
    let output = Command::new("git")
        .args(["diff", "--name-only", "origin/main...HEAD"])
        .current_dir(root)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let mut paths: Vec<PathBuf> = String::from_utf8(output.stdout)
        .ok()?
        .lines()
        .map(PathBuf::from)
        .collect();
    let working = Command::new("git")
        .args(["diff", "--name-only", "HEAD"])
        .current_dir(root)
        .output()
        .ok()?;
    if !working.status.success() {
        return None;
    }
    paths.extend(
        String::from_utf8(working.stdout)
            .ok()?
            .lines()
            .map(PathBuf::from),
    );
    paths.sort();
    paths.dedup();
    Some(paths)
}

/// Launch each selected local equivalent as a child process that survives the
/// short-lived hook process. Commands are arguments, not interpolated shell
/// text; only governed graph projections reach this boundary.
pub fn spawn_detached(root: &Path, checks: &[CiCheck], state_dir: &Path) -> Result<()> {
    std::fs::create_dir_all(state_dir)?;
    for (index, check) in checks.iter().enumerate() {
        let result = state_dir.join(format!("{index}.status"));
        let tmp = state_dir.join(format!("{index}.tmp"));
        Command::new("sh")
            .args([
                "-c",
                "sh -c \"$1\" >/dev/null 2>&1; code=$?; printf '%s' \"$code\" >\"$2\"; mv \"$2\" \"$3\"",
                "yupana-ci",
                &check.command,
                &tmp.to_string_lossy(),
                &result.to_string_lossy(),
            ])
            .current_dir(root)
            .spawn()
            .map_err(|e| Error::Projection(format!("could not start `{}`: {e}", check.label)))?;
    }
    Ok(())
}

/// Evaluate the already-installed `pre-bash` hook at the push boundary.
#[cfg(feature = "quipu")]
pub(crate) fn hook_advisory(payload: &str, command: &str) -> crate::hook::Outcome {
    use crate::hook::Outcome;
    if !command
        .split_whitespace()
        .collect::<Vec<_>>()
        .windows(2)
        .any(|pair| pair == ["git", "push"])
    {
        return Outcome::Allow;
    }
    let Some(input) = crate::hook::HookInput::parse(payload) else {
        return Outcome::Notify(
            "yupana CI shift-left: UNKNOWN (hook payload did not parse)".into(),
        );
    };
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let root = input.root(&cwd);
    let config = match crate::config::YupanaConfig::resolve(None, &root) {
        Ok(config) => config,
        Err(error) => return Outcome::Notify(format!("yupana CI shift-left: UNKNOWN ({error})")),
    };
    let body = match crate::project::query(&config.quipu.endpoint, CI_MAP_QUERY) {
        Ok(body) => body,
        Err(error) => return Outcome::Notify(format!("yupana CI shift-left: UNKNOWN ({error})")),
    };
    let checks = match decode_map(&body) {
        Ok(checks) => checks,
        Err(error) => return Outcome::Notify(format!("yupana CI shift-left: UNKNOWN ({error})")),
    };
    let Some(changed) = changed_for_push(&root) else {
        return Outcome::Notify("yupana CI shift-left: UNKNOWN (push diff did not resolve)".into());
    };
    match select(&checks, &changed) {
        Selection::Unknown => {
            Outcome::Notify("yupana CI shift-left: UNKNOWN (no governed CI map)".into())
        }
        Selection::Quiet => Outcome::Allow,
        Selection::Checks(selected) => {
            let Some(state) = crate::projection_cache::cache_path()
                .and_then(|path| path.parent().map(|parent| parent.join("ci-shift-left")))
            else {
                return Outcome::Notify(
                    "yupana CI shift-left: UNKNOWN (no state directory)".into(),
                );
            };
            if let Err(error) = spawn_detached(&root, &selected, &state) {
                return Outcome::Notify(format!("yupana CI shift-left: UNKNOWN ({error})"));
            }
            Outcome::Notify(format!(
                "yupana CI shift-left: started local CI equivalents in background: {}",
                selected
                    .iter()
                    .map(|check| check.label.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn historical_map() -> Vec<CiCheck> {
        decode_map(
            &serde_json::json!({"results":{"bindings":[
                {"job":{"value":"ci:file-size"},"label":{"value":"file size check"},
                 "command":{"value":"pre-commit run file-size-check --all-files"},"scope":{"value":"src/**/*.rs"}},
                {"job":{"value":"ci:internal-identifiers"},"label":{"value":"internal identifier ratchet"},
                 "command":{"value":"cargo test --test no_internal_identifiers"},"scope":{"value":"**"}},
                {"job":{"value":"ci:wasm"},"label":{"value":"Check wasm32 target"},
                 "command":{"value":"cargo check --target wasm32-unknown-unknown --no-default-features --lib"},
                 "scope":{"value":"src/**/*.rs"}}
            ]}})
            .to_string(),
        )
        .unwrap()
    }

    #[test]
    fn historical_misses_select_every_guard_that_can_observe_the_change() {
        let map = historical_map();
        let Selection::Checks(yupana) = select(&map, &["src/hook/config_drift.rs".into()]) else {
            panic!("a085c8a must select the repository guards");
        };
        assert_eq!(
            yupana
                .iter()
                .map(|check| check.label.as_str())
                .collect::<Vec<_>>(),
            [
                "file size check",
                "internal identifier ratchet",
                "Check wasm32 target"
            ]
        );

        let wasm = map.iter().find(|check| check.id == "ci:wasm").unwrap();
        let Selection::Checks(quipu) = select(
            std::slice::from_ref(wasm),
            &["src/store/snapshot_upload.rs".into()],
        ) else {
            panic!("e510e47 must select the wasm32 gate");
        };
        assert_eq!(quipu[0].label, "Check wasm32 target");
        let Selection::Checks(docs) = select(&map, &["docs/readme.md".into()]) else {
            panic!("the tree-wide identifier ratchet must cover documentation");
        };
        assert_eq!(docs[0].label, "internal identifier ratchet");
    }

    #[test]
    fn an_absent_map_is_unknown_not_quiet() {
        assert_eq!(select(&[], &["src/lib.rs".into()]), Selection::Unknown);
    }

    #[test]
    fn background_results_are_published_atomically() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state/results.json");
        let checks = vec![
            CiCheck {
                id: "ok".into(),
                label: "passes".into(),
                command: "true".into(),
                scopes: vec!["**".into()],
            },
            CiCheck {
                id: "bad".into(),
                label: "fails".into(),
                command: "false".into(),
                scopes: vec!["**".into()],
            },
        ];
        run_background(dir.path().into(), checks, path.clone())
            .join()
            .unwrap();
        assert_eq!(
            read_results(&path).unwrap(),
            vec![
                CheckResult {
                    label: "passes".into(),
                    passed: true
                },
                CheckResult {
                    label: "fails".into(),
                    passed: false
                },
            ]
        );
    }
}
