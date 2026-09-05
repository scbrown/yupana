//! Governed session trajectories: command evidence followed by an edit.
//!
//! This channel uses the existing Policy/Selector/Predicate vocabulary. The
//! selector's evidenceSource is a validated JSON invocation specification;
//! the predicate names the event ordering. No CLI names or advice live here.

use serde::{Deserialize, Serialize};

use crate::errors::{Error, Result};

/// Discover the channel by one indexed predicate lookup. Fetch each returned
/// subject directly: optional multi-joins exceeded the live query deadline.
/// Missing types or fields are validated after discovery, never filtered out.
const ONTOLOGY_NS: &str = "http://aegis.gastown.local/ontology/";

pub const TRAJECTORY_QUERY: &str = "\
PREFIX aegis: <http://aegis.gastown.local/ontology/>
SELECT ?policy WHERE { ?policy aegis:targets \"session-trajectory\" }";

#[path = "project_trajectory_fetch.rs"]
mod fetch;
pub use fetch::fetch_trajectory_policies;

/// Invocation vocabulary, owned by the graph rather than the shell parser.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InvocationTrigger {
    pub programs: Vec<String>,
    pub verbs: Vec<String>,
}

/// Advice frequency. An edit scope deliberately permits repeated advice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OncePer {
    Session,
    Edit,
}

/// A fully decoded graph rule. Only the warn tier is executable at post-edit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrajectoryPolicy {
    pub id: String,
    pub label: String,
    pub trigger: InvocationTrigger,
    pub ordering: String,
    pub tier: String,
    pub once_per: OncePer,
    pub rationale: String,
    pub effect: String,
    pub verification_point: String,
}

impl TrajectoryPolicy {
    /// Validate both network results and restored cache data.
    pub fn validate(&self) -> Result<()> {
        let invalid = |why| Error::Projection(format!("trajectory policy `{}`: {why}", self.id));
        if self.tier == "block" {
            return Err(invalid("block tier requires a pre-edit enforcement point; this build only supports post-edit advice"));
        }
        if self.tier != "warn" {
            return Err(invalid("unsupported enforcement tier; expected warn"));
        }
        if self.effect != "warn" || self.verification_point != "PAA" {
            return Err(invalid("this channel requires effect warn at PAA; other responses or placements are not implemented"));
        }
        if self.ordering != "command-before-edit" {
            return Err(invalid(
                "unsupported event ordering; expected command-before-edit",
            ));
        }
        for values in [&self.trigger.programs, &self.trigger.verbs] {
            if values.is_empty()
                || values.iter().any(|value| {
                    value.is_empty()
                        || !value
                            .chars()
                            .all(|c| c.is_ascii_alphanumeric() || "-_".contains(c))
                })
            {
                return Err(invalid(
                    "trigger programs and verbs must be nonempty lists of bare command tokens",
                ));
            }
        }
        if self.id.is_empty() || self.rationale.trim().is_empty() {
            return Err(invalid("policy identity and rationale are required"));
        }
        Ok(())
    }
}

impl crate::project::ProjectionRegistry {
    /// None is an unknown channel in an older cache, not an empty catalogue.
    #[must_use]
    pub fn trajectory_policies(&self) -> Option<&[TrajectoryPolicy]> {
        self.trajectory_policies.as_deref()
    }
}

pub fn decode_trajectory_policies(body: &str) -> Result<Vec<TrajectoryPolicy>> {
    let value = serde_json::from_str(body)
        .map_err(|e| Error::Projection(format!("trajectory results are not JSON: {e}")))?;
    let mut policies: Vec<TrajectoryPolicy> = Vec::new();
    for row in crate::project_decode::rows_of(&value)? {
        let required = |key| {
            crate::project_decode::binding_value(row, key).ok_or_else(|| {
                Error::Projection(format!(
                    "trajectory policy missing required binding `{key}`"
                ))
            })
        };
        let id = required("policy")?;
        let trigger = serde_json::from_str(&required("trigger")?).map_err(|e| {
            Error::Projection(format!("trajectory policy `{id}` has invalid trigger: {e}"))
        })?;
        let once_per = match required("oncePer")?.as_str() {
            "session" => OncePer::Session,
            "edit" => OncePer::Edit,
            other => {
                return Err(Error::Projection(format!(
                    "trajectory policy `{id}` has unsupported oncePer `{other}`"
                )))
            }
        };
        let policy = TrajectoryPolicy {
            label: crate::project_decode::binding_value(row, "label").unwrap_or_else(|| id.clone()),
            id,
            trigger,
            ordering: required("ordering")?,
            tier: required("tier")?,
            once_per,
            rationale: required("rationale")?,
            effect: required("effect")?,
            verification_point: required("point")?,
        };
        policy.validate()?;
        if let Some(existing) = policies.iter().find(|p| p.id == policy.id) {
            if existing != &policy {
                return Err(Error::Projection(format!(
                    "trajectory policy `{}` has conflicting definitions",
                    policy.id
                )));
            }
        } else {
            policies.push(policy);
        }
    }
    Ok(policies)
}

#[cfg(test)]
#[path = "project_trajectory_test.rs"]
mod tests;
