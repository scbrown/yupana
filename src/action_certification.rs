//! Signed certification records for agent actions.
//!
//! The host guard supplies observed action facts and ordered checks. Yupana
//! computes the decision, signs every decision-bearing field, and appends the
//! complete record to a local JSONL spool. Promotion is deliberately separate
//! from the push path, as it is for structural verdicts.
#![allow(missing_docs)]

use std::path::Path;

use ring::signature::{Ed25519KeyPair, KeyPair};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::errors::{Error, Result};

pub const SCHEMA_VERSION: &str = "aegis-action-certification/v1";
pub const CANONICALIZATION: &str = "serde-json-struct-order/v1";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CheckInput {
    pub id: String,
    pub expected: serde_json::Value,
    pub observed: serde_json::Value,
    pub evidence_ref: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActionInput {
    pub record_id: String,
    pub correlation_id: String,
    pub session: String,
    pub ts: u64,
    pub agent: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub item: Option<String>,
    pub verb: String,
    pub target: String,
    pub target_class: String,
    pub tenant: String,
    pub result: String,
    pub repo: String,
    pub sha: String,
    #[serde(rename = "ref")]
    pub git_ref: String,
    pub remote_authority: String,
    pub scope_provenance: serde_json::Value,
    pub checks: Vec<CheckInput>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CheckResult {
    pub id: String,
    pub outcome: String,
    pub expected: serde_json::Value,
    pub observed: serde_json::Value,
    pub evidence_ref: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SignedActionRecord {
    pub schema_version: String,
    pub kind: String,
    pub record_id: String,
    pub correlation_id: String,
    pub session: String,
    pub ts: u64,
    pub agent: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub item: Option<String>,
    pub verb: String,
    pub target: String,
    pub target_class: String,
    pub tenant: String,
    pub result: String,
    pub repo: String,
    pub sha: String,
    #[serde(rename = "ref")]
    pub git_ref: String,
    pub remote_authority: String,
    pub scope_provenance: serde_json::Value,
    pub checks: Vec<CheckResult>,
    pub certification_status: String,
    pub reason_codes: Vec<String>,
    pub verifier_id: String,
    pub key_id: String,
    pub signature_alg: String,
    pub canonicalization: String,
    pub signed_payload_hash: String,
    pub verdict_id: String,
    pub signature: String,
}

#[derive(Serialize)]
struct Payload<'a> {
    schema_version: &'static str,
    kind: &'static str,
    input: &'a ActionInput,
    checks: &'a [CheckResult],
    certification_status: &'a str,
    reason_codes: &'a [String],
    verifier_id: &'static str,
    key_id: &'a str,
    canonicalization: &'static str,
}

pub fn sign(input: ActionInput, key: &Ed25519KeyPair) -> Result<SignedActionRecord> {
    if input.record_id.is_empty() || input.correlation_id.is_empty() || input.session.is_empty() {
        return Err(Error::Promote(
            "record_id, correlation_id, and session are required".into(),
        ));
    }
    let checks: Vec<_> = input
        .checks
        .iter()
        .map(|check| CheckResult {
            id: check.id.clone(),
            outcome: if check.expected == check.observed {
                "satisfied"
            } else {
                "unsatisfied"
            }
            .into(),
            expected: check.expected.clone(),
            observed: check.observed.clone(),
            evidence_ref: check.evidence_ref.clone(),
        })
        .collect();
    let reason_codes: Vec<_> = checks
        .iter()
        .filter(|c| c.outcome == "unsatisfied")
        .map(|c| format!("{}_mismatch", c.id.replace('-', "_")))
        .collect();
    let status = if reason_codes.is_empty() {
        "certified"
    } else {
        "uncertified"
    };
    let key_id = format!(
        "sha256:{}",
        hex::encode(Sha256::digest(key.public_key().as_ref()))
    );
    let payload = Payload {
        schema_version: SCHEMA_VERSION,
        kind: "action",
        input: &input,
        checks: &checks,
        certification_status: status,
        reason_codes: &reason_codes,
        verifier_id: crate::verdict::VERIFIER,
        key_id: &key_id,
        canonicalization: CANONICALIZATION,
    };
    let bytes = serde_json::to_vec(&payload)
        .map_err(|e| Error::Promote(format!("canonicalize action: {e}")))?;
    let signed_payload_hash = format!("sha256:{}", hex::encode(Sha256::digest(&bytes)));
    let signature = hex::encode(key.sign(&bytes).as_ref());
    let verdict_id = format!("action-verdict-{}", &signature[..32]);
    Ok(SignedActionRecord {
        schema_version: SCHEMA_VERSION.into(),
        kind: "action".into(),
        record_id: input.record_id,
        correlation_id: input.correlation_id,
        session: input.session,
        ts: input.ts,
        agent: input.agent,
        item: input.item,
        verb: input.verb,
        target: input.target,
        target_class: input.target_class,
        tenant: input.tenant,
        result: input.result,
        repo: input.repo,
        sha: input.sha,
        git_ref: input.git_ref,
        remote_authority: input.remote_authority,
        scope_provenance: input.scope_provenance,
        checks,
        certification_status: status.into(),
        reason_codes,
        verifier_id: crate::verdict::VERIFIER.into(),
        key_id,
        signature_alg: "ed25519".into(),
        canonicalization: CANONICALIZATION.into(),
        signed_payload_hash,
        verdict_id,
        signature,
    })
}

pub fn append(path: &Path, record: &SignedActionRecord) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    serde_json::to_writer(&mut file, record)
        .map_err(|e| Error::Promote(format!("serialize action record: {e}")))?;
    writeln!(file)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ring::rand::SystemRandom;

    fn key() -> Ed25519KeyPair {
        Ed25519KeyPair::from_pkcs8(
            Ed25519KeyPair::generate_pkcs8(&SystemRandom::new())
                .unwrap()
                .as_ref(),
        )
        .unwrap()
    }
    fn input(observed: bool) -> ActionInput {
        ActionInput {
            record_id: "r1".into(),
            correlation_id: "c1".into(),
            session: "s1".into(),
            ts: 1,
            agent: "grant".into(),
            item: Some("aegis-x".into()),
            verb: "push".into(),
            target: "repo_yupana".into(),
            target_class: "repo".into(),
            tenant: "worker".into(),
            result: "attempted".into(),
            repo: "scbrown/yupana".into(),
            sha: "a".repeat(40),
            git_ref: "refs/heads/main".into(),
            remote_authority: "github.com/scbrown".into(),
            scope_provenance: serde_json::json!({"as_of":1,"query_id":"q1"}),
            checks: vec![CheckInput {
                id: "assignee-match".into(),
                expected: true.into(),
                observed: observed.into(),
                evidence_ref: "br:aegis-x".into(),
            }],
        }
    }
    #[test]
    fn positive_and_negative_are_same_shape() {
        let kp = key();
        let yes = sign(input(true), &kp).unwrap();
        let no = sign(input(false), &kp).unwrap();
        assert_eq!(yes.certification_status, "certified");
        assert!(yes.reason_codes.is_empty());
        assert_eq!(no.certification_status, "uncertified");
        assert_eq!(no.reason_codes, ["assignee_match_mismatch"]);
        assert_eq!(yes.kind, no.kind);
        assert_ne!(yes.signature, no.signature);
    }
    #[test]
    fn missing_item_remains_a_signed_negative() {
        let kp = key();
        let mut i = input(false);
        i.item = None;
        i.checks[0].id = "bead-present".into();
        let record = sign(i, &kp).unwrap();
        assert_eq!(record.certification_status, "uncertified");
        assert!(serde_json::to_value(record).unwrap().get("item").is_none());
    }
}
