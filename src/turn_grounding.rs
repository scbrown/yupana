//! Turn-boundary grounding evidence for `NeuralAmplifier` actions.
//!
//! The hot path reads one content-addressed local file and never contacts the
//! graph. The producer owns capture; Yupana only verifies identity, binding and
//! freshness before attaching the result to its existing constraint trace.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Current Unix time without requiring the optional graph-projection feature.
#[must_use]
pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// The reference injected into a hook payload by the trusted harness layer.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct GroundingRef {
    /// Applicability scope. This feature recognises only `na`.
    pub scope: Option<String>,
    /// `sha256:<hex>` of the exact cached evidence bytes.
    pub grounding_id: Option<String>,
    /// Faction whose fog-scoped view produced the evidence.
    pub faction_id: Option<String>,
    /// Content hash of the world view used by the decision.
    pub worldview_sha256: Option<String>,
}

/// The canonical evidence file produced at the turn boundary.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GroundingEvidence {
    /// Named graph consulted.
    pub graph: String,
    /// Exact query text, retained for replay.
    pub query: String,
    /// Entity IRIs returned by the query.
    pub entities: Vec<String>,
    /// Turn at which consultation happened.
    pub turn: u64,
    /// Consultation result.
    pub outcome: EvidenceOutcome,
    /// Faction whose view was consulted.
    pub faction_id: String,
    /// World-view content hash bound to this consultation.
    pub worldview_sha256: String,
    /// Unix timestamp at capture.
    pub captured_at: u64,
}

/// Producer-side consultation outcomes. A boolean would collapse absence and
/// transport failure into the same misleading answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum EvidenceOutcome {
    /// Facts were returned and used.
    Used,
    /// The query completed and returned no facts.
    Empty,
    /// The graph could not be consulted.
    TransportError,
}

/// Yupana's local resolution result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GroundingState {
    /// The action is outside the NA grounding scope.
    NotApplicable,
    /// The harness did not declare a recognised scope.
    UnknownScope,
    /// Known NA scope, but no id was supplied.
    Missing,
    /// The id or local evidence could not be resolved honestly.
    Unresolved(String),
    /// The evidence is older than the accepted turn-context window.
    Stale {
        /// Observed evidence age.
        age_secs: u64,
        /// Configured freshness ceiling.
        max_age_secs: u64,
    },
    /// Consultation completed but returned no entities.
    Empty,
    /// Consultation failed at its transport.
    TransportError,
    /// Fresh evidence was resolved and bound to this faction/world view.
    Used,
}

impl GroundingState {
    /// Stable trace label.
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::NotApplicable => "not-applicable",
            Self::UnknownScope => "unknown-scope",
            Self::Missing => "missing",
            Self::Unresolved(_) => "unresolved",
            Self::Stale { .. } => "stale",
            Self::Empty => "empty",
            Self::TransportError => "transport-error",
            Self::Used => "used",
        }
    }

    /// Model-facing advice. `None` is the clean, fresh case or out-of-scope.
    #[must_use]
    pub fn advice(&self) -> Option<String> {
        let detail = match self {
            Self::NotApplicable | Self::Used => return None,
            Self::UnknownScope => "grounding scope is missing or unrecognised".to_string(),
            Self::Missing => "NA scope is known but grounding_id is missing".to_string(),
            Self::Unresolved(why) => format!("grounding evidence is unresolved ({why})"),
            Self::Stale {
                age_secs,
                max_age_secs,
            } => format!("grounding evidence is stale ({age_secs}s > {max_age_secs}s)"),
            Self::Empty => "grounding query completed but returned no entities".to_string(),
            Self::TransportError => "grounding query ended in transport-error".to_string(),
        };
        Some(format!(
            "yupana (grounding advise, not blocking): {detail}; action remains allowed"
        ))
    }
}

/// Resolve a process cache directory without consulting workspace config.
#[must_use]
pub fn cache_dir() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("YUPANA_GROUNDING_CACHE_DIR") {
        return Some(PathBuf::from(path));
    }
    std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .map(|p| p.join("yupana").join("grounding"))
        .or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .map(|p| p.join(".local/state/yupana/grounding"))
        })
}

/// Configured freshness ceiling; malformed values conservatively use 300s.
#[must_use]
pub fn max_age_secs() -> u64 {
    std::env::var("YUPANA_GROUNDING_MAX_AGE_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(300)
}

/// Resolve and verify grounding evidence using local bytes only.
#[must_use]
pub fn assess(
    reference: Option<&GroundingRef>,
    cache: Option<&Path>,
    now: u64,
    max_age_secs: u64,
) -> GroundingState {
    let Some(reference) = reference else {
        return GroundingState::NotApplicable;
    };
    match reference.scope.as_deref() {
        Some("na") => {}
        _ => return GroundingState::UnknownScope,
    }
    let Some(id) = reference.grounding_id.as_deref() else {
        return GroundingState::Missing;
    };
    let Some(hex_id) = id.strip_prefix("sha256:") else {
        return GroundingState::Unresolved("grounding_id is not sha256:<hex>".to_string());
    };
    if hex_id.len() != 64 || !hex_id.bytes().all(|b| b.is_ascii_hexdigit()) {
        return GroundingState::Unresolved("grounding_id has an invalid digest".to_string());
    }
    let Some(cache) = cache else {
        return GroundingState::Unresolved("no local cache directory is configured".to_string());
    };
    let path = cache.join(format!("{hex_id}.json"));
    let body = match std::fs::read(&path) {
        Ok(body) => body,
        Err(e) => return GroundingState::Unresolved(format!("{}: {e}", path.display())),
    };
    if hex::encode(Sha256::digest(&body)) != hex_id.to_ascii_lowercase() {
        return GroundingState::Unresolved("cached bytes do not match grounding_id".to_string());
    }
    let evidence: GroundingEvidence = match serde_json::from_slice(&body) {
        Ok(evidence) => evidence,
        Err(e) => return GroundingState::Unresolved(format!("evidence is not valid JSON ({e})")),
    };
    if reference.faction_id.as_deref() != Some(evidence.faction_id.as_str()) {
        return GroundingState::Unresolved("faction_id binding does not match".to_string());
    }
    if reference.worldview_sha256.as_deref() != Some(evidence.worldview_sha256.as_str()) {
        return GroundingState::Unresolved("worldview_sha256 binding does not match".to_string());
    }
    let Some(age_secs) = now.checked_sub(evidence.captured_at) else {
        return GroundingState::Unresolved("evidence is future-dated".to_string());
    };
    if age_secs > max_age_secs {
        return GroundingState::Stale {
            age_secs,
            max_age_secs,
        };
    }
    match evidence.outcome {
        EvidenceOutcome::Used => GroundingState::Used,
        EvidenceOutcome::Empty => GroundingState::Empty,
        EvidenceOutcome::TransportError => GroundingState::TransportError,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn evidence(outcome: EvidenceOutcome) -> GroundingEvidence {
        GroundingEvidence {
            graph: "urn:neuralamplifier:graph:knowledge".into(),
            query: "SELECT ?s WHERE { ?s ?p ?o }".into(),
            entities: vec!["urn:entity:one".into()],
            turn: 42,
            outcome,
            faction_id: "raptors".into(),
            worldview_sha256: "sha256:worldview".into(),
            captured_at: 1_000,
        }
    }

    fn write(cache: &Path, evidence: &GroundingEvidence) -> GroundingRef {
        let body = serde_json::to_vec(evidence).unwrap();
        let digest = hex::encode(Sha256::digest(&body));
        std::fs::write(cache.join(format!("{digest}.json")), body).unwrap();
        GroundingRef {
            scope: Some("na".into()),
            grounding_id: Some(format!("sha256:{digest}")),
            faction_id: Some(evidence.faction_id.clone()),
            worldview_sha256: Some(evidence.worldview_sha256.clone()),
        }
    }

    #[test]
    fn known_answer_binds_id_faction_and_worldview() {
        let dir = tempfile::tempdir().unwrap();
        let reference = write(dir.path(), &evidence(EvidenceOutcome::Used));
        assert_eq!(
            assess(Some(&reference), Some(dir.path()), 1_010, 300),
            GroundingState::Used
        );
        let mut wrong = reference;
        wrong.faction_id = Some("other".into());
        assert!(matches!(
            assess(Some(&wrong), Some(dir.path()), 1_010, 300),
            GroundingState::Unresolved(_)
        ));
    }

    #[test]
    fn every_absent_error_and_stale_state_is_distinct() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            assess(None, Some(dir.path()), 1_010, 300),
            GroundingState::NotApplicable
        );
        let unknown = GroundingRef::default();
        assert_eq!(
            assess(Some(&unknown), Some(dir.path()), 1_010, 300),
            GroundingState::UnknownScope
        );
        let missing = GroundingRef {
            scope: Some("na".into()),
            ..GroundingRef::default()
        };
        assert_eq!(
            assess(Some(&missing), Some(dir.path()), 1_010, 300),
            GroundingState::Missing
        );
        let empty = write(dir.path(), &evidence(EvidenceOutcome::Empty));
        assert_eq!(
            assess(Some(&empty), Some(dir.path()), 1_010, 300),
            GroundingState::Empty
        );
        let failed = write(dir.path(), &evidence(EvidenceOutcome::TransportError));
        assert_eq!(
            assess(Some(&failed), Some(dir.path()), 1_010, 300),
            GroundingState::TransportError
        );
        let stale = write(dir.path(), &evidence(EvidenceOutcome::Used));
        assert!(matches!(
            assess(Some(&stale), Some(dir.path()), 1_301, 300),
            GroundingState::Stale { .. }
        ));
    }

    #[test]
    fn one_hundred_local_resolutions_fit_inside_one_hot_path_budget() {
        let dir = tempfile::tempdir().unwrap();
        let reference = write(dir.path(), &evidence(EvidenceOutcome::Used));
        let started = std::time::Instant::now();
        for _ in 0..100 {
            assert_eq!(
                assess(Some(&reference), Some(dir.path()), 1_010, 300),
                GroundingState::Used
            );
        }
        assert!(
            started.elapsed() < std::time::Duration::from_millis(100),
            "100 local resolutions took {:?}",
            started.elapsed()
        );
    }
}
