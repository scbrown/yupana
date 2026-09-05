//! Indexed catalogue reads. A malformed member is an error, not a lost row.
use std::collections::BTreeMap;

use super::{decode_trajectory_policies, TrajectoryPolicy, TRAJECTORY_QUERY};
use crate::errors::{Error, Result};

fn error(message: impl std::fmt::Display) -> Error {
    Error::Projection(format!("trajectory catalogue: {message}"))
}

type Properties = BTreeMap<String, Vec<String>>;

fn properties(endpoint: &str, iri: &str) -> Result<Properties> {
    // This IRI came from graph data, not source text. Do not interpolate SPARQL
    // delimiters from it: rejecting an unsupported IRI is a visible failure.
    if !iri.contains(':')
        || iri
            .chars()
            .any(|c| c.is_whitespace() || "<>\\\"{}".contains(c))
    {
        return Err(error("subject is not a safe absolute IRI"));
    }
    let body = crate::project::query(
        endpoint,
        &format!("SELECT ?property ?value WHERE {{ <{iri}> ?property ?value }}"),
    )?;
    let value = serde_json::from_str(&body).map_err(error)?;
    let mut properties: Properties = BTreeMap::new();
    for row in crate::project_decode::rows_of(&value)? {
        let property = crate::project_decode::binding_value(row, "property")
            .ok_or_else(|| error("missing property binding"))?;
        let value = crate::project_decode::binding_value(row, "value")
            .ok_or_else(|| error("missing value binding"))?;
        let values = properties.entry(property).or_default();
        if !values.contains(&value) {
            values.push(value);
        }
    }
    Ok(properties)
}

fn one(properties: &Properties, key: &str) -> Result<String> {
    match properties.get(key).map(Vec::as_slice) {
        Some([value]) => Ok(value.clone()),
        Some(_) => Err(error(format!("conflicting values for {key}"))),
        None => Err(error(format!("missing required property {key}"))),
    }
}

fn typed(properties: &Properties, kind: &str) -> Result<()> {
    let target = format!("{}{kind}", super::ONTOLOGY_NS);
    if !properties
        .get("http://www.w3.org/1999/02/22-rdf-syntax-ns#type")
        .is_some_and(|types| types.contains(&target))
    {
        return Err(error(format!("catalogue member must be typed {kind}")));
    }
    Ok(())
}

/// Fetch the catalogue using bounded, indexed subject reads. Empty discovery is
/// a known empty channel; missing atoms and incomplete members are errors.
pub fn fetch_trajectory_policies(endpoint: &str) -> Result<Vec<TrajectoryPolicy>> {
    let body = crate::project::query(endpoint, TRAJECTORY_QUERY)?;
    let value = serde_json::from_str(&body).map_err(error)?;
    let mut bindings = Vec::new();
    for row in crate::project_decode::rows_of(&value)? {
        let id = crate::project_decode::binding_value(row, "policy")
            .ok_or_else(|| error("missing policy identity"))?;
        let policy = properties(endpoint, &id)?;
        typed(&policy, "Policy")?;
        let field = |key| one(&policy, &format!("{}{key}", super::ONTOLOGY_NS));
        let selector = properties(endpoint, &field("selector")?)?;
        let predicate = properties(endpoint, &field("predicate")?)?;
        typed(&selector, "Selector")?;
        typed(&predicate, "Predicate")?;
        let evidence = format!("{}evidenceSource", super::ONTOLOGY_NS);
        let mut row = serde_json::Map::new();
        for (key, value) in [
            ("policy", id),
            ("trigger", one(&selector, &evidence)?),
            ("ordering", one(&predicate, &evidence)?),
            ("tier", field("enforcementTier")?),
            ("oncePer", field("oncePer")?),
            ("effect", field("effect")?),
            ("point", field("verificationPoint")?),
            (
                "rationale",
                one(&policy, "http://www.w3.org/2000/01/rdf-schema#comment")?,
            ),
        ] {
            row.insert(key.into(), serde_json::json!({"value":value}));
        }
        let label_key = "http://www.w3.org/2000/01/rdf-schema#label";
        if policy.contains_key(label_key) {
            row.insert(
                "label".into(),
                serde_json::json!({"value":one(&policy, label_key)?}),
            );
        }
        bindings.push(row);
    }
    decode_trajectory_policies(&serde_json::json!({"results":{"bindings":bindings}}).to_string())
}

#[cfg(test)]
#[path = "project_trajectory_fetch_test.rs"]
mod tests;
