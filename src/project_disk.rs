//! Quipu-backed command disk-impact observations.
//!
//! Samples are free-space deltas from `df`, keyed by a normalized command
//! signature and an opaque filesystem identity. Raw argv and paths never enter
//! the graph.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::errors::{Error, Result};

/// One historical consumption sample, in bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiskSample {
    /// Bytes consumed while the command ran. Free-space gains are recorded as zero.
    pub consumed_bytes: u64,
}

/// Return a stable, privacy-preserving command signature.
#[must_use]
pub fn command_signature(command: &str, cwd: &std::path::Path) -> Option<String> {
    let segment = command.split([';', '|', '&']).next()?.trim();
    let mut words = segment
        .split_whitespace()
        .filter(|word| !word.contains('='));
    let binary = words.next()?.rsplit('/').next()?.trim_matches(['\'', '"']);
    let binary = match binary {
        "sudo" | "env" | "nice" => words.next()?.rsplit('/').next()?,
        other => other,
    };
    let subcommand = words
        .find(|word| !word.starts_with('-'))
        .map_or("_", |word| word.trim_matches(['\'', '"']));
    let repo = repo_class(cwd).unwrap_or_else(|| "outside-repo".to_string());
    Some(format!(
        "{}:{}:{}",
        safe_token(binary),
        safe_token(subcommand),
        repo
    ))
}

fn repo_class(cwd: &std::path::Path) -> Option<String> {
    let remote_name = std::process::Command::new("git")
        .args(["-C", cwd.to_str()?, "config", "--get", "remote.origin.url"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .and_then(|remote| {
            remote
                .trim()
                .trim_end_matches('/')
                .rsplit(['/', ':'])
                .next()
                .map(str::to_owned)
        });
    if let Some(name) = remote_name {
        let name = name.trim_end_matches(".git");
        if !name.is_empty() {
            return Some(safe_token(name));
        }
    }
    cwd.ancestors()
        .find(|path| path.join(".git").exists())
        .and_then(std::path::Path::file_name)
        .and_then(|name| name.to_str())
        .map(safe_token)
}

fn safe_token(text: &str) -> String {
    let token: String = text
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        .take(80)
        .collect();
    if token.is_empty() {
        "_".into()
    } else {
        token
    }
}

/// Opaque stable identifier; never discloses a device or mount path.
#[must_use]
pub fn filesystem_identity(device: &str, mount: &str) -> String {
    let digest = Sha256::digest(format!("{device}\0{mount}").as_bytes());
    format!("fs-{}", hex::encode(&digest[..12]))
}

/// Nearest-rank p90 prediction. Empty history is honestly `None`.
#[must_use]
pub fn p90(samples: &[DiskSample]) -> Option<u64> {
    if samples.is_empty() {
        return None;
    }
    let mut values: Vec<u64> = samples.iter().map(|s| s.consumed_bytes).collect();
    values.sort_unstable();
    let rank = (9 * values.len()).div_ceil(10).saturating_sub(1);
    values.get(rank).copied()
}

/// Fetch recent samples for one normalized command/filesystem pair.
pub fn fetch_samples(endpoint: &str, signature: &str, filesystem: &str) -> Result<Vec<DiskSample>> {
    let sparql = format!(
        "PREFIX aegis: <http://aegis.gastown.local/ontology/>\n\
         SELECT ?delta WHERE {{ ?s a aegis:CommandDiskImpactObservation ; \
         aegis:commandSignature {} ; aegis:filesystemIdentity {} ; \
         aegis:diskDeltaBytes ?delta . }} ORDER BY DESC(?s) LIMIT 100",
        sparql_string(signature),
        sparql_string(filesystem)
    );
    decode_samples(&crate::project::query(endpoint, &sparql)?)
}

fn sparql_string(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "\"\"".into())
}

/// Decode Quipu's W3C SPARQL result envelope.
pub fn decode_samples(body: &str) -> Result<Vec<DiskSample>> {
    let value: serde_json::Value = serde_json::from_str(body)
        .map_err(|e| Error::Projection(format!("disk-history results are not JSON: {e}")))?;
    let rows = crate::project_decode::rows_of(&value)?;
    rows.iter()
        .enumerate()
        .map(|(i, row)| {
            let value = crate::project_decode::binding_value(row, "delta").ok_or_else(|| {
                Error::Projection(format!("disk-history row {i}: missing `delta`"))
            })?;
            let consumed_bytes = value.parse::<u64>().map_err(|e| {
                Error::Projection(format!("disk-history row {i}: invalid `delta`: {e}"))
            })?;
            Ok(DiskSample { consumed_bytes })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signature_omits_flags_paths_and_raw_argv() {
        let got = command_signature(
            "/usr/bin/cargo build --target-dir /secret/alice",
            std::path::Path::new("/tmp"),
        );
        assert_eq!(got.as_deref(), Some("cargo:build:outside-repo"));
        assert!(!got.unwrap().contains("secret"));
    }

    #[test]
    fn filesystem_identity_is_opaque() {
        let got = filesystem_identity("/dev/mapper/private", "/home/jsmith");
        assert!(got.starts_with("fs-"));
        assert!(!got.contains("alice"));
        assert!(!got.contains("mapper"));
    }

    #[test]
    fn p90_is_unknown_without_history_and_nearest_rank_with_it() {
        assert_eq!(p90(&[]), None);
        let samples: Vec<_> = (1..=10).map(|n| DiskSample { consumed_bytes: n }).collect();
        assert_eq!(p90(&samples), Some(9));
    }

    #[test]
    fn decodes_samples() {
        let body = r#"{"results":{"bindings":[{"delta":{"value":"42"}}]}}"#;
        assert_eq!(
            decode_samples(body).unwrap(),
            vec![DiskSample { consumed_bytes: 42 }]
        );
    }
}
