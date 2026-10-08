//! Shared Quipu client credentials and refusal state across hook processes.
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};

const HELP: &str = "checked QUIPU_AUTH_TOKEN > QUIPU_AUTH_TOKEN_FILE > ~/.config/quipu/token; install the issued token at ~/.config/quipu/token; run caboodle doctor";
static DISABLED: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

#[derive(Clone, Copy)]
pub(crate) enum Failure {
    Missing,
    Unreadable,
    Invalid,
    Rejected,
}
impl Failure {
    fn name(self) -> &'static str {
        match self {
            Self::Missing => "missing",
            Self::Unreadable => "unreadable",
            Self::Invalid => "invalid",
            Self::Rejected => "rejected (HTTP 401)",
        }
    }
}

pub(crate) fn resolve(
    inline: Option<&str>,
    explicit: Option<&Path>,
    home: Option<&Path>,
) -> Result<Option<String>, Failure> {
    if let Some(value) = inline.map(str::trim).filter(|s| !s.is_empty()) {
        return validate(value).map(Some);
    }
    let default = home.map(|home| home.join(".config/quipu/token"));
    let Some(path) = explicit
        .filter(|p| !p.as_os_str().is_empty())
        .or(default.as_deref())
    else {
        return Ok(None);
    };
    match std::fs::read_to_string(path) {
        Ok(value) if value.trim().is_empty() => Ok(None),
        Ok(value) => validate(value.trim()).map(Some),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err(Failure::Unreadable),
    }
}

fn validate(value: &str) -> Result<String, Failure> {
    if value.contains(['\n', '\r']) {
        return Err(Failure::Invalid);
    }
    Ok(value.to_owned())
}

fn token_result() -> Result<Option<String>, Failure> {
    resolve(
        std::env::var("QUIPU_AUTH_TOKEN").ok().as_deref(),
        std::env::var_os("QUIPU_AUTH_TOKEN_FILE")
            .as_deref()
            .map(Path::new),
        std::env::var_os("HOME").as_deref().map(Path::new),
    )
}

fn ambient_session() -> Option<String> {
    [
        "QUIPU_SESSION",
        "CODEX_SESSION_ID",
        "CODEX_THREAD_ID",
        "CLAUDE_CODE_SESSION_ID",
    ]
    .iter()
    .find_map(|key| std::env::var(key).ok().filter(|s| !s.is_empty()))
}

fn key(endpoint: &str, session: Option<&str>) -> String {
    hex::encode(Sha256::digest(
        format!(
            "{}\0{}",
            endpoint.trim_end_matches('/'),
            session.unwrap_or("process")
        )
        .as_bytes(),
    ))
}

fn marker(endpoint: &str, session: Option<&str>) -> Option<PathBuf> {
    session?;
    let root = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state"))
        })?;
    Some(
        root.join("yupana/quipu-auth")
            .join(format!("{}.disabled", key(endpoint, session))),
    )
}

fn disabled(endpoint: &str, session: Option<&str>) -> bool {
    DISABLED
        .get_or_init(Default::default)
        .lock()
        .map_or(true, |cache| cache.contains(&key(endpoint, session)))
        || marker(endpoint, session).is_some_and(|path| path.exists())
}

fn record_marker(path: &Path, reason: &str) -> std::io::Result<bool> {
    use std::io::Write;
    let parent = path.parent().expect("marker has parent");
    std::fs::create_dir_all(parent)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    match options.open(path) {
        Ok(mut file) => {
            file.write_all(reason.as_bytes())?;
            Ok(true)
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
        Err(error) => Err(error),
    }
}

fn refuse(endpoint: &str, session: Option<&str>, reason: Failure) -> String {
    if let Ok(mut cache) = DISABLED.get_or_init(Default::default).lock() {
        if cache.insert(key(endpoint, session)) {
            let (first, scope) = match marker(endpoint, session) {
                Some(path) => match record_marker(&path, reason.name()) {
                    Ok(first) => (first, "harness session"),
                    Err(_) => (true, "process only (session latch unavailable)"),
                },
                None => (true, "process only (no harness session ID)"),
            };
            if first {
                eprintln!(
                    "yupana: Quipu credential {}; {HELP}; writes disabled for this {scope}",
                    reason.name()
                );
            }
        }
    }
    format!(
        "Quipu writes disabled: credential {}; run caboodle doctor",
        reason.name()
    )
}

/// A write needs a credential; an explicit hook session overrides ambient metadata.
pub(crate) fn require(endpoint: &str, session: Option<&str>) -> Result<String, String> {
    let ambient = ambient_session();
    let session = session.or(ambient.as_deref());
    if disabled(endpoint, session) {
        return Err(
            "Quipu writes disabled for this session; start a new session after credential repair"
                .into(),
        );
    }
    match token_result() {
        Ok(Some(token)) => Ok(token),
        Ok(None) => Err(refuse(endpoint, session, Failure::Missing)),
        Err(reason) => Err(refuse(endpoint, session, reason)),
    }
}

/// Record a definite authentication refusal without echoing the response body.
pub(crate) fn rejected(endpoint: &str, session: Option<&str>) -> String {
    let ambient = ambient_session();
    refuse(endpoint, session.or(ambient.as_deref()), Failure::Rejected)
}

/// Non-mutating status: configured is not accepted authentication.
pub(crate) fn status(endpoint: &str) -> serde_json::Value {
    let session = ambient_session();
    let state = if disabled(endpoint, session.as_deref()) {
        "writes_disabled"
    } else {
        match token_result() {
            Ok(Some(_)) => "configured_acceptance_unproven",
            Ok(None) => "missing",
            Err(Failure::Invalid) => "invalid",
            Err(_) => "unreadable",
        }
    };
    serde_json::json!({"state": state, "help": HELP, "session_scope": if session.is_some() {"harness_session"} else {"process_only"}})
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn canonical_precedence_and_explicit_absence_never_use_legacy() {
        let home = tempfile::tempdir().unwrap();
        let canonical = home.path().join(".config/quipu/token");
        let legacy = home.path().join(".config/aegis/quipu_token");
        std::fs::create_dir_all(legacy.parent().unwrap()).unwrap();
        std::fs::write(&legacy, "legacy").unwrap();
        assert!(resolve(None, None, Some(home.path()))
            .unwrap_or(None)
            .is_none());
        std::fs::create_dir_all(canonical.parent().unwrap()).unwrap();
        std::fs::write(&canonical, "canonical\n").unwrap();
        assert_eq!(
            resolve(Some(" \n"), None, Some(home.path()))
                .ok()
                .flatten()
                .as_deref(),
            Some("canonical")
        );
        assert_eq!(
            resolve(Some("inline"), Some(&legacy), Some(home.path()))
                .ok()
                .flatten()
                .as_deref(),
            Some("inline")
        );
        assert_eq!(
            resolve(None, Some(&legacy), Some(home.path()))
                .ok()
                .flatten()
                .as_deref(),
            Some("legacy")
        );
        assert!(
            resolve(None, Some(&home.path().join("absent")), Some(home.path()))
                .ok()
                .flatten()
                .is_none()
        );
        assert!(resolve(Some("invalid\ncredential"), None, Some(home.path())).is_err());
    }
}
