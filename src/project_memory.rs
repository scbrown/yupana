//! Governed host-memory policies projected from Quipu.
//!
//! The command matcher and threshold are DATA: a Policy aimed at
//! `aegis:MemoryHeavyCommand` composes a live Bash-command selector, an exact
//! regex predicate, and a deterministic `MemAvailable GiB` OperatingPoint.
//! Yupana owns only the evaluator at the pre-action seam.

use serde::{Deserialize, Serialize};

use crate::constraint::{ConstraintClass, VerificationPoint};
use crate::errors::{Error, Result};
use crate::policy::Mode;

/// The graph catalogue query for execution-memory policies.
pub const MEMORY_POLICY_QUERY: &str = "\
PREFIX aegis: <http://aegis.gastown.local/ontology/>
PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>
SELECT ?policy ?label ?pattern ?matchType ?effect ?constraintClass
       ?verificationPoint ?threshold ?basis ?kind ?rationale WHERE {
  ?policy a aegis:Policy ;
          aegis:targets \"aegis:MemoryHeavyCommand\" ;
          aegis:boundary \"action\" ;
          aegis:selector ?selector ;
          aegis:predicate ?predicate ;
          aegis:operatingPoint ?operatingPoint ;
          aegis:effect ?effect ;
          aegis:constraintClass ?constraintClass ;
          aegis:verificationPoint ?verificationPoint .
  ?selector aegis:evidenceSource \"bash-command\" ;
            aegis:tier \"live\" .
  ?predicate aegis:evidenceSource ?pattern ;
             aegis:matchType ?matchType ;
             aegis:tier \"live\" .
  ?operatingPoint aegis:kind ?kind ;
                  aegis:threshold ?threshold ;
                  aegis:calibrationBasis ?basis .
  OPTIONAL { ?policy rdfs:label ?label }
  OPTIONAL { ?policy rdfs:comment ?rationale }
}";

/// One command-memory rule, fully decoded and validated.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MemoryPolicy {
    /// Stable policy IRI.
    pub id: String,
    /// Human-readable policy name.
    pub label: String,
    /// Exact regex selecting memory-heavy command lines.
    pub command_regex: String,
    /// Minimum `MemAvailable`, in GiB, before the command is permitted.
    pub threshold_gib: f64,
    /// Governed response when the threshold is crossed.
    pub effect: String,
    /// Constraint class, independent of response.
    pub class: ConstraintClass,
    /// The point where the graph says this policy runs.
    pub verification_point: VerificationPoint,
    /// Optional governed explanation.
    pub rationale: Option<String>,
}

impl crate::project::ProjectionRegistry {
    /// Governed execution-memory policies and their shared projection freshness.
    #[must_use]
    pub fn memory_policies(&self) -> &[MemoryPolicy] {
        &self.memory_policies
    }
}

impl MemoryPolicy {
    /// Whether this policy selects `command`.
    #[must_use]
    pub fn matches(&self, command: &str) -> bool {
        regex::Regex::new(&self.command_regex).is_ok_and(|r| r.is_match(command))
    }

    /// Whether the policy may block under the deployment's ambient mode.
    #[must_use]
    pub fn blocks(&self, mode: Mode) -> bool {
        self.class.blocks(mode) && !matches!(self.effect.as_str(), "warn" | "record" | "allow")
    }
}

/// Fetch the governed command-memory catalogue.
pub fn fetch_memory_policies(endpoint: &str) -> Result<Vec<MemoryPolicy>> {
    decode_memory_policies(&crate::project::query(endpoint, MEMORY_POLICY_QUERY)?)
}

/// Decode Quipu's W3C SPARQL-results response.
pub fn decode_memory_policies(body: &str) -> Result<Vec<MemoryPolicy>> {
    let value: serde_json::Value = serde_json::from_str(body)
        .map_err(|e| Error::Projection(format!("memory-policy results are not JSON: {e}")))?;
    let rows = crate::project_decode::rows_of(&value)?;
    let mut out: Vec<MemoryPolicy> = Vec::new();

    for (i, row) in rows.iter().enumerate() {
        let get = |key: &str| crate::project_decode::binding_value(row, key);
        let required = |key: &str| {
            get(key).ok_or_else(|| {
                Error::Projection(format!(
                    "memory-policy row {i}: missing required binding `{key}`"
                ))
            })
        };
        let id = required("policy")?;
        let pattern = required("pattern")?;
        regex::Regex::new(&pattern).map_err(|e| {
            Error::Projection(format!(
                "memory policy `{id}` has invalid command regex: {e}"
            ))
        })?;
        if required("matchType")? != "must-match" {
            return Err(Error::Projection(format!(
                "memory policy `{id}` must use matchType `must-match`"
            )));
        }
        if required("kind")? != "deterministic_threshold"
            || required("basis")? != "MemAvailable GiB"
        {
            return Err(Error::Projection(format!(
                "memory policy `{id}` must declare deterministic_threshold over MemAvailable GiB"
            )));
        }
        let threshold_gib = required("threshold")?.parse::<f64>().map_err(|e| {
            Error::Projection(format!("memory policy `{id}` has invalid threshold: {e}"))
        })?;
        if !threshold_gib.is_finite() || threshold_gib <= 0.0 {
            return Err(Error::Projection(format!(
                "memory policy `{id}` threshold must be finite and positive"
            )));
        }
        let class_text = required("constraintClass")?;
        let class = ConstraintClass::parse(&class_text).ok_or_else(|| {
            Error::Projection(format!(
                "memory policy `{id}` has unknown constraintClass `{class_text}`"
            ))
        })?;
        let point_text = required("verificationPoint")?;
        let verification_point = VerificationPoint::parse(&point_text).ok_or_else(|| {
            Error::Projection(format!(
                "memory policy `{id}` has unknown verificationPoint `{point_text}`"
            ))
        })?;
        if verification_point != VerificationPoint::Pag {
            return Err(Error::Projection(format!(
                "memory policy `{id}` is evaluated by pre-bash and must declare PAG"
            )));
        }
        let policy = MemoryPolicy {
            label: get("label")
                .unwrap_or_else(|| id.rsplit('/').next().unwrap_or(id.as_str()).to_string()),
            command_regex: pattern,
            threshold_gib,
            effect: required("effect")?,
            class,
            verification_point,
            rationale: get("rationale"),
            id: id.clone(),
        };

        if let Some(existing) = out.iter().find(|p| p.id == id) {
            if existing != &policy {
                return Err(Error::Projection(format!(
                    "memory policy `{id}` has conflicting values across SPARQL rows"
                )));
            }
        } else {
            out.push(policy);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(threshold: &str) -> String {
        serde_json::json!({"results":{"bindings":[{
            "policy":{"value":"http://example/policy_memory"},
            "label":{"value":"memory headroom"},
            "pattern":{"value":"(^|[;&|[:space:]])cargo[[:space:]]+(build|test)"},
            "matchType":{"value":"must-match"},
            "effect":{"value":"deny"},
            "constraintClass":{"value":"hard"},
            "verificationPoint":{"value":"PAG"},
            "threshold":{"value":threshold},
            "basis":{"value":"MemAvailable GiB"},
            "kind":{"value":"deterministic_threshold"}
        }]}})
        .to_string()
    }

    #[test]
    fn decodes_and_matches_a_governed_threshold() {
        let p = decode_memory_policies(&body("24")).unwrap().remove(0);
        assert_eq!(p.threshold_gib, 24.0);
        assert!(p.matches("just test && cargo build --release"));
        assert!(!p.matches("cargo metadata --no-deps"));
        assert!(!p.blocks(Mode::Advise));
        assert!(p.blocks(Mode::Enforce));
    }

    #[test]
    fn invalid_threshold_is_a_projection_error() {
        assert!(decode_memory_policies(&body("0")).is_err());
        assert!(decode_memory_policies(&body("not-a-number")).is_err());
    }
}
