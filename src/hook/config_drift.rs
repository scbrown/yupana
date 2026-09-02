//! Guard st configuration artifacts against unrepresented graph state.

use std::path::Path;

use sha2::{Digest, Sha256};

use super::{HookInput, Outcome};
use crate::config::YupanaConfig;
use crate::policy::Mode;

#[derive(Debug, Clone, PartialEq, Eq)]
enum Representation {
    Represented,
    Unrepresented,
    Unknown(String),
}

pub(super) fn check(
    config: &YupanaConfig,
    input: &HookInput,
    file: &Path,
    rel: &str,
) -> Option<Outcome> {
    let mode = config.policy.config_drift;
    if mode == Mode::Off || !is_st_config(file, rel) {
        return None;
    }
    let key = canonical_path(file, rel);
    let baseline = std::fs::read_to_string(file).ok();
    let Some(proposed) = super::pre_edit::proposed_buffer(input, baseline.as_deref()) else {
        return Some(verdict(
            mode,
            Representation::Unknown("proposed file could not be reconstructed".into()),
            &key,
        ));
    };
    if !config.quipu.enabled || config.quipu.endpoint.is_empty() {
        return Some(verdict(
            mode,
            Representation::Unknown("quipu projection is not configured".into()),
            &key,
        ));
    }
    let digest = hex::encode(Sha256::digest(proposed.as_bytes()));
    let path = serde_json::to_string(&key).unwrap_or_else(|_| "\"\"".into());
    let digest = serde_json::to_string(&digest).unwrap_or_else(|_| "\"\"".into());
    let query = format!(
        "PREFIX aegis: <http://aegis.gastown.local/ontology/> SELECT ?config WHERE {{ ?config a aegis:ConfigFile ; aegis:configPath {path} ; aegis:contentSha256 {digest} . }} LIMIT 1"
    );
    let state = match crate::project::query(&config.quipu.endpoint, &query) {
        Ok(body) => decode(&body),
        Err(error) => Representation::Unknown(error.to_string()),
    };
    Some(verdict(mode, state, &key))
}

fn canonical_path(file: &Path, rel: &str) -> String {
    let full = file.to_string_lossy();
    for marker in [".shanty/", ".claude/"] {
        if let Some(index) = full.find(marker) {
            return full[index..].to_string();
        }
    }
    rel.to_string()
}

fn is_st_config(file: &Path, rel: &str) -> bool {
    let path = file.to_string_lossy();
    ((rel.contains(".shanty/crew/") || path.contains("/.shanty/crew/")) && rel.ends_with(".json"))
        || rel.ends_with(".shanty/env.json")
        || rel.ends_with(".claude/settings.json")
        || rel.ends_with(".claude/settings.local.json")
        || path.ends_with("/.claude/settings.json")
        || path.ends_with("/.claude/settings.local.json")
        || path.ends_with("/.shanty/env.json")
}

fn decode(body: &str) -> Representation {
    match serde_json::from_str::<serde_json::Value>(body) {
        Ok(value) => match value
            .pointer("/results/bindings")
            .and_then(|v| v.as_array())
        {
            Some(rows) if !rows.is_empty() => Representation::Represented,
            Some(_) => Representation::Unrepresented,
            None => Representation::Unknown("quipu returned malformed query results".into()),
        },
        Err(error) => Representation::Unknown(format!("quipu results were not JSON: {error}")),
    }
}

fn verdict(mode: Mode, state: Representation, path: &str) -> Outcome {
    let (message, violation) = match state {
        Representation::Represented => return Outcome::Allow,
        Representation::Unrepresented => (
            format!("yupana config drift: `{path}` proposed content is NOT REPRESENTED in quipu"),
            true,
        ),
        Representation::Unknown(reason) => (
            format!("yupana config drift: UNKNOWN for `{path}` ({reason})"),
            true,
        ),
    };
    if mode == Mode::Enforce && violation {
        Outcome::Deny(message)
    } else {
        Outcome::Notify(format!(
            "{message} (advisory: config_drift is not \"enforce\")"
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn represented_passes_unrepresented_warns_and_unknown_is_loud() {
        assert_eq!(
            decode(r#"{"results":{"bindings":[{"config":{"value":"x"}}]}}"#),
            Representation::Represented
        );
        assert!(
            matches!(verdict(Mode::Advise, decode(r#"{"results":{"bindings":[]}}"#), ".shanty/env.json"), Outcome::Notify(s) if s.contains("NOT REPRESENTED"))
        );
        assert!(
            matches!(verdict(Mode::Advise, decode("down"), ".shanty/env.json"), Outcome::Notify(s) if s.contains("UNKNOWN"))
        );
    }

    #[test]
    fn enforce_denies_both_absent_and_unknown_but_not_represented() {
        assert!(matches!(
            verdict(Mode::Enforce, Representation::Unrepresented, "x"),
            Outcome::Deny(_)
        ));
        assert!(matches!(
            verdict(Mode::Enforce, Representation::Unknown("down".into()), "x"),
            Outcome::Deny(_)
        ));
        assert_eq!(
            verdict(Mode::Enforce, Representation::Represented, "x"),
            Outcome::Allow
        );
    }

    #[test]
    fn routing_covers_cards_environment_and_hook_settings_only() {
        assert!(is_st_config(
            Path::new("/r/.shanty/crew/grant.json"),
            ".shanty/crew/grant.json"
        ));
        assert!(is_st_config(
            Path::new("/r/.claude/settings.json"),
            ".claude/settings.json"
        ));
        assert!(!is_st_config(Path::new("/r/src/lib.rs"), "src/lib.rs"));
    }
}
