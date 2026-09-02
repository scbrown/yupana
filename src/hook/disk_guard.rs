//! Advise-only disk-space guard and one-command-late measurement loop.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::pre_edit::Outcome;

const WARN_HEADROOM_NUMERATOR: u64 = 4;
const WARN_HEADROOM_DENOMINATOR: u64 = 5;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Pending {
    signature: String,
    filesystem: String,
    available_bytes: u64,
    observed_at: String,
}

#[derive(Debug, Clone)]
struct DiskReading {
    filesystem: String,
    available_bytes: u64,
}

pub(super) fn observe_and_check(payload: &str, command: &str) -> Outcome {
    if !might_consume_disk(command) {
        return Outcome::Allow;
    }
    let Some(input) = super::HookInput::parse(payload) else {
        return Outcome::Allow;
    };
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let root = input.root(&cwd);
    let Some(signature) = crate::project_disk::command_signature(command, &root) else {
        return Outcome::Allow;
    };
    let reading = match read_disk(&root) {
        Ok(reading) => reading,
        Err(e) => return Outcome::Notify(format!("yupana: disk impact UNKNOWN: {e}")),
    };
    let config = match crate::config::YupanaConfig::resolve(None, &root) {
        Ok(config) => config,
        Err(e) => {
            return Outcome::Notify(format!(
                "{} disk impact UNKNOWN: unreadable config ({e})",
                super::CONFIG_ERROR_PREFIX
            ))
        }
    };
    if !config.quipu.enabled || config.quipu.endpoint.is_empty() {
        return Outcome::Notify(
            "yupana: disk impact UNKNOWN: Quipu history is not configured".into(),
        );
    }

    let session = input.session_id.as_deref().unwrap_or("anonymous");
    let measurement = finish_previous(session, &reading, &config.quipu.endpoint);
    let _ = arm(session, &signature, &reading);

    let samples = match crate::project_disk::fetch_samples(
        &config.quipu.endpoint,
        &signature,
        &reading.filesystem,
    ) {
        Ok(samples) => samples,
        Err(e) => {
            return Outcome::Notify(format!(
                "yupana: disk impact UNKNOWN: history query failed ({e})"
            ))
        }
    };
    let Some(predicted) = crate::project_disk::p90(&samples) else {
        return Outcome::Notify(format!(
            "yupana: disk impact UNKNOWN for `{signature}` on {}: no Quipu-recorded history; command is allowed, not declared safe{}",
            reading.filesystem,
            measurement.as_deref().map_or(String::new(), |m| format!("; {m}"))
        ));
    };
    let available = headroom_override().unwrap_or(reading.available_bytes);
    let limit = available.saturating_mul(WARN_HEADROOM_NUMERATOR) / WARN_HEADROOM_DENOMINATOR;
    crate::metrics::emit(
        "disk_guard",
        &[
            ("signature", signature.clone().into()),
            ("filesystem", reading.filesystem.clone().into()),
            ("samples", samples.len().into()),
            ("predicted_bytes", predicted.into()),
            ("available_bytes", available.into()),
        ],
    );
    if u64::try_from(predicted).is_ok_and(|predicted| predicted > limit) {
        Outcome::Notify(format!(
            "yupana (governed, not blocking): disk history predicts p90 {} across {} sample(s) for `{signature}` on {}; current headroom is {} and the 80% advisory budget is {}. Free space or choose another filesystem before running.",
            signed_bytes(predicted), samples.len(), reading.filesystem, bytes(available), bytes(limit)
        ))
    } else {
        Outcome::Allow
    }
}

fn might_consume_disk(command: &str) -> bool {
    [
        "cargo",
        "rustc",
        "npm",
        "pnpm",
        "yarn",
        "docker",
        "podman",
        "dd",
        "fallocate",
        "cp",
        "rsync",
        "tar",
        "caboodle",
    ]
    .iter()
    .any(|word| {
        command
            .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
            .any(|part| part == *word)
    })
}

fn read_disk(path: &Path) -> Result<DiskReading, String> {
    let output = Command::new("df")
        .args(["-Pk", "--", path.to_str().unwrap_or(".")])
        .output()
        .map_err(|e| format!("cannot run df: {e}"))?;
    if !output.status.success() {
        return Err("df failed".into());
    }
    let text =
        String::from_utf8(output.stdout).map_err(|e| format!("df output is not UTF-8: {e}"))?;
    let row = text.lines().last().ok_or("df returned no filesystem row")?;
    let columns: Vec<&str> = row.split_whitespace().collect();
    if columns.len() < 6 {
        return Err("df returned an unrecognised row".into());
    }
    let available_kib = columns[3]
        .parse::<u64>()
        .map_err(|e| format!("invalid df availability: {e}"))?;
    Ok(DiskReading {
        filesystem: crate::project_disk::filesystem_identity(columns[0], columns[5]),
        available_bytes: available_kib.saturating_mul(1024),
    })
}

fn finish_previous(session: &str, now: &DiskReading, endpoint: &str) -> Option<String> {
    let path = state_path(session)?;
    let pending: Pending = serde_json::from_slice(&std::fs::read(&path).ok()?).ok()?;
    if pending.filesystem != now.filesystem {
        return Some("previous command crossed filesystems; no delta recorded".into());
    }
    let delta = (i128::from(pending.available_bytes) - i128::from(now.available_bytes))
        .clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64;
    match post_sample(endpoint, &pending, delta) {
        Ok(()) => Some(format!(
            "recorded previous-command delta {}",
            signed_bytes(delta)
        )),
        Err(e) => Some(format!("previous-command delta NOT RECORDED ({e})")),
    }
}

fn arm(session: &str, signature: &str, reading: &DiskReading) -> Result<(), String> {
    let path = state_path(session).ok_or("no state directory")?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let pending = Pending {
        signature: signature.into(),
        filesystem: reading.filesystem.clone(),
        available_bytes: reading.available_bytes,
        observed_at: chrono::Utc::now().to_rfc3339(),
    };
    let mut file = std::fs::File::create(path).map_err(|e| e.to_string())?;
    serde_json::to_writer(&mut file, &pending).map_err(|e| e.to_string())?;
    file.flush().map_err(|e| e.to_string())
}

fn state_path(session: &str) -> Option<PathBuf> {
    let root = std::env::var_os("YUPANA_DISK_STATE_DIR")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("XDG_STATE_HOME")
                .map(PathBuf::from)
                .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))
        })?;
    let digest = Sha256::digest(session.as_bytes());
    Some(
        root.join("yupana/disk")
            .join(format!("{}.json", hex::encode(&digest[..12]))),
    )
}

fn post_sample(endpoint: &str, pending: &Pending, delta: i64) -> Result<(), String> {
    let id = hex::encode(Sha256::digest(
        format!(
            "{}\0{}\0{}",
            pending.signature, pending.filesystem, pending.observed_at
        )
        .as_bytes(),
    ));
    let turtle = format!(
        "@prefix aegis: <http://aegis.gastown.local/ontology/> .\n@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .\n\
         aegis:disk-impact-{id} a aegis:CommandDiskImpactObservation ; aegis:commandSignature {} ; \
         aegis:filesystemIdentity {} ; aegis:diskDeltaBytes \"{delta}\"^^xsd:integer ; aegis:observedAt {}^^xsd:dateTime ; \
         rdfs:label {} .",
        serde_json::to_string(&pending.signature).map_err(|e| e.to_string())?,
        serde_json::to_string(&pending.filesystem).map_err(|e| e.to_string())?,
        serde_json::to_string(&pending.observed_at).map_err(|e| e.to_string())?,
        serde_json::to_string(&format!("disk impact {}", pending.signature))
            .map_err(|e| e.to_string())?
    );
    let url = format!("{}/knot", endpoint.trim_end_matches('/'));
    let mut request = ureq::post(&url)
        .set("Content-Type", "application/json")
        .set("X-Quipu-Client", "agent-adhoc");
    if let Some(token) = auth_token() {
        request = request.set("Authorization", &format!("Bearer {token}"));
    }
    let body =
        serde_json::json!({"turtle": turtle, "actor": "yupana", "source": "command-disk-impact"})
            .to_string();
    let response = request
        .timeout(std::time::Duration::from_secs(3))
        .send_string(&body)
        .map_err(|e| e.to_string())?;
    let response_body = response
        .into_string()
        .map_err(|e| format!("cannot read Quipu knot response: {e}"))?;
    let result: serde_json::Value = serde_json::from_str(&response_body)
        .map_err(|e| format!("invalid Quipu knot response: {e}"))?;
    if result.get("conforms").and_then(serde_json::Value::as_bool) == Some(false) {
        return Err(format!("Quipu rejected observation: {result}"));
    }
    let count = result
        .get("count")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let tx_id = result.get("tx_id").and_then(serde_json::Value::as_u64);
    if count == 0 && tx_id != Some(0) {
        return Err(format!("Quipu wrote zero observation triples: {result}"));
    }
    Ok(())
}

fn auth_token() -> Option<String> {
    std::env::var("QUIPU_AUTH_TOKEN")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| {
            let home = std::env::var_os("HOME")?;
            std::fs::read_to_string(PathBuf::from(home).join(".config/aegis/quipu_token"))
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        })
}

fn headroom_override() -> Option<u64> {
    std::env::var("YUPANA_DISK_HEADROOM_BYTES")
        .ok()?
        .parse()
        .ok()
}

fn bytes(value: u64) -> String {
    const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
    format!("{:.2} GiB", value as f64 / GIB)
}

fn signed_bytes(value: i64) -> String {
    let sign = if value < 0 { "-" } else { "" };
    format!("{sign}{}", bytes(value.unsigned_abs()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routing_is_narrow_and_noop_is_quiet() {
        assert!(might_consume_disk("cargo build --release"));
        assert!(might_consume_disk("fallocate -l 1G image"));
        assert!(!might_consume_disk("true"));
        assert!(!might_consume_disk("git status"));
    }
}
