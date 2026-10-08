//! `yupana hooks bundle|install|uninstall|status` — yupana owns its hook
//! definitions and installs them into Claude Code AND Codex (aegis-5s32or).
//!
//! The bundle (`hooks/yupana.bundle.json`, schema `st.hook-bundle/1`) is the
//! ONE source of truth for which hooks yupana wants. On a host running
//! shantytown, install REGISTERS it with `st ops hooks register`, and st renders
//! it into every role's Claude and Codex settings on every emit, so a settings
//! re-emit or relaunch cannot drop it. Without st, install writes the harness's
//! own config directly: `settings.json` for Claude, `config.toml` for Codex.
//! Both carry the same shape, `hooks.<Event> = [{matcher, hooks: [{type,
//! command}]}]`, so one merge serves both.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};
use serde_json::{json, Map, Value};

/// The shipped bundle, with the build's version substituted.
#[must_use]
pub fn bundle() -> Value {
    let text = include_str!("../hooks/yupana.bundle.json")
        .replace("{{VERSION}}", env!("CARGO_PKG_VERSION"));
    serde_json::from_str(&text).expect("hooks/yupana.bundle.json is valid JSON")
}

/// A harness yupana can install hooks into.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Harness {
    /// Claude Code (`settings.json`).
    Claude,
    /// Codex (`config.toml`).
    Codex,
}

impl Harness {
    fn name(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
        }
    }
}

/// The `(event, matcher, command)` hooks the bundle declares for `harness`.
#[must_use]
pub fn hooks_for(bundle: &Value, harness: Harness) -> Vec<(String, Option<String>, String)> {
    bundle["hooks"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|h| {
            h["harnesses"]
                .as_array()
                .is_some_and(|hs| hs.iter().any(|x| x == harness.name()))
        })
        .map(|h| {
            (
                h["event"].as_str().unwrap_or_default().to_string(),
                h["matcher"].as_str().map(str::to_string),
                h["command"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// Add every bundle hook for `harness` to a `hooks` table (Claude/Codex
/// shape). Idempotent: a hook whose command is already under the same event
/// and matcher is left alone. Returns how many were added.
pub fn merge(config: &mut Value, bundle: &Value, harness: Harness) -> usize {
    let hooks = ensure_object(config, "hooks");
    let mut added = 0;
    for (event, matcher, command) in hooks_for(bundle, harness) {
        let groups = hooks
            .entry(event)
            .or_insert_with(|| json!([]))
            .as_array_mut()
            .expect("hooks.<event> is a list");
        let group = match groups
            .iter()
            .position(|g| g["matcher"].as_str() == matcher.as_deref())
        {
            Some(i) => &mut groups[i],
            None => {
                let mut g = Map::new();
                if let Some(m) = &matcher {
                    g.insert("matcher".into(), json!(m));
                }
                g.insert("hooks".into(), json!([]));
                groups.push(Value::Object(g));
                groups.last_mut().expect("just pushed")
            }
        };
        let list = group["hooks"].as_array_mut().expect("hooks list");
        if !list.iter().any(|h| h["command"] == command.as_str()) {
            list.push(json!({"type": "command", "command": command}));
            added += 1;
        }
    }
    added
}

/// Remove every bundle hook for `harness` (matched by exact command), dropping
/// groups and events left empty. Hooks yupana did not declare are untouched.
pub fn remove(config: &mut Value, bundle: &Value, harness: Harness) -> usize {
    let ours: Vec<String> = hooks_for(bundle, harness)
        .into_iter()
        .map(|h| h.2)
        .collect();
    let Some(hooks) = config.get_mut("hooks").and_then(Value::as_object_mut) else {
        return 0;
    };
    let mut removed = 0;
    for groups in hooks.values_mut() {
        let Some(groups) = groups.as_array_mut() else {
            continue;
        };
        for group in groups.iter_mut() {
            if let Some(list) = group.get_mut("hooks").and_then(Value::as_array_mut) {
                let before = list.len();
                list.retain(|h| !ours.iter().any(|c| h["command"] == c.as_str()));
                removed += before - list.len();
            }
        }
        groups.retain(|g| g["hooks"].as_array().is_none_or(|l| !l.is_empty()));
    }
    hooks.retain(|_, v| v.as_array().is_none_or(|g| !g.is_empty()));
    removed
}

/// How many of the bundle's hooks for `harness` are present in `config`.
#[must_use]
pub fn present(config: &Value, bundle: &Value, harness: Harness) -> (usize, usize) {
    let want = hooks_for(bundle, harness);
    let found = want
        .iter()
        .filter(|(event, matcher, command)| {
            config["hooks"][event].as_array().is_some_and(|groups| {
                groups.iter().any(|g| {
                    g["matcher"].as_str() == matcher.as_deref()
                        && g["hooks"]
                            .as_array()
                            .is_some_and(|l| l.iter().any(|h| h["command"] == command.as_str()))
                })
            })
        })
        .count();
    (found, want.len())
}

fn ensure_object<'a>(value: &'a mut Value, key: &str) -> &'a mut Map<String, Value> {
    if !value.is_object() {
        *value = json!({});
    }
    let map = value.as_object_mut().expect("object");
    if !map.get(key).is_some_and(Value::is_object) {
        map.insert(key.into(), json!({}));
    }
    map.get_mut(key)
        .and_then(Value::as_object_mut)
        .expect("object")
}

/// The harness config file for a scope: `user` (the harness home) or
/// `project` (the current directory's `.claude/` or `.codex/`).
#[must_use]
pub fn config_path(harness: Harness, project: bool) -> PathBuf {
    let home = || PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    match (harness, project) {
        (Harness::Claude, false) => home().join(".claude/settings.json"),
        (Harness::Claude, true) => PathBuf::from(".claude/settings.json"),
        (Harness::Codex, false) => std::env::var_os("CODEX_HOME")
            .map_or_else(|| home().join(".codex"), PathBuf::from)
            .join("config.toml"),
        (Harness::Codex, true) => PathBuf::from(".codex/config.toml"),
    }
}

/// Read a harness config as JSON (TOML for Codex). Absent means empty.
pub fn read_config(path: &Path, harness: Harness) -> Result<Value> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Ok(json!({}));
    };
    Ok(match harness {
        Harness::Claude => serde_json::from_str(&text)
            .with_context(|| format!("{} is not valid JSON", path.display()))?,
        Harness::Codex => serde_json::to_value(
            toml::from_str::<toml::Value>(&text)
                .with_context(|| format!("{} is not valid TOML", path.display()))?,
        )?,
    })
}

/// Write a harness config, keeping the previous file as `<name>.bak-yupana`
/// (a TOML rewrite does not keep comments).
pub fn write_config(path: &Path, harness: Harness, config: &Value) -> Result<()> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)?;
    }
    if path.exists() {
        let mut backup = path.as_os_str().to_owned();
        backup.push(".bak-yupana");
        std::fs::copy(path, PathBuf::from(backup))?;
    }
    let text = match harness {
        Harness::Claude => serde_json::to_string_pretty(config)? + "\n",
        Harness::Codex => toml::to_string(&serde_json::from_value::<toml::Value>(config.clone())?)?,
    };
    std::fs::write(path, text)?;
    Ok(())
}

/// Is a shantytown registry usable here? `st ops hooks list` answering is the test.
#[must_use]
pub fn st_available() -> bool {
    Command::new("st")
        .args(["ops", "hooks", "list"])
        .output()
        .is_ok_and(|o| o.status.success())
}

/// Register or unregister the bundle with st (both harnesses, every role).
pub fn st_register(register: bool) -> Result<String> {
    let out = if register {
        let file = std::env::temp_dir().join(format!("yupana-bundle-{}.json", std::process::id()));
        std::fs::write(&file, serde_json::to_string_pretty(&bundle())?)?;
        let out = Command::new("st")
            .args(["ops", "hooks", "register"])
            .arg(&file)
            .output();
        let _ = std::fs::remove_file(&file);
        out?
    } else {
        Command::new("st")
            .args(["ops", "hooks", "unregister", "yupana"])
            .output()?
    };
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    if !out.status.success() {
        bail!("st refused: {}", text.trim());
    }
    Ok(text.trim().to_string())
}

#[cfg(test)]
#[path = "hooks_install_test.rs"]
mod tests;
