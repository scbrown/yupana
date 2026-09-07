//! Read the text catalogue without joining optional fields across the store.
//!
//! Membership still uses the full TextRule subclass path. Exact subject reads
//! then reconstruct the same required/optional row product as TEXT_POLICY_QUERY;
//! the existing decoder keeps every distinct optional value and rejects conflicts.

use std::collections::HashSet;

use serde_json::{json, Value};

use super::{decode_text_rules, query};
use crate::errors::{Error, Result};
use crate::project_decode::{binding_value, rows_of};
use crate::textrules::TextRule;

const MEMBERS: &str = "\
PREFIX aegis: <http://aegis.gastown.local/ontology/>
PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>
SELECT ?s WHERE { ?s a/rdfs:subClassOf* aegis:TextRule }";

const FIELDS: &[(&str, &str, bool)] = &[
    ("regex", "aegis:regex", true),
    ("tier", "aegis:enforcementTier", true),
    ("label", "rdfs:label", false),
    ("class", "aegis:identifierClass", false),
    ("exempt", "aegis:exemptPathRegex", false),
    ("rationale", "rdfs:comment", false),
];

fn error(message: impl std::fmt::Display) -> Error {
    Error::Projection(format!("text catalogue: {message}"))
}

fn read(endpoint: &str, sparql: &str) -> Result<Value> {
    let value: Value =
        serde_json::from_str(&query(endpoint, sparql).map_err(error)?).map_err(error)?;
    if value.get("truncated").and_then(Value::as_bool) == Some(true) {
        return Err(error("truncated result; refusing a partial catalogue"));
    }
    rows_of(&value)?;
    Ok(value)
}

fn property_name(iri: &str) -> String {
    iri.replace("http://aegis.gastown.local/ontology/", "aegis:")
        .replace("http://www.w3.org/2000/01/rdf-schema#", "rdfs:")
}

/// Preserve the OPTIONAL cross product, including multi-valued exemptions and
/// rationales. No first-value selection or LIMIT can silently shrink a rule.
fn expand(subject: &Value, properties: &[Value]) -> Result<Vec<Value>> {
    let mut fields = std::collections::HashMap::<String, Vec<Value>>::new();
    for row in properties {
        let property =
            binding_value(row, "property").ok_or_else(|| error("missing property binding"))?;
        binding_value(row, "value").ok_or_else(|| error("missing value binding"))?;
        fields
            .entry(property_name(&property))
            .or_default()
            .push(row["value"].clone());
    }
    let mut rows = vec![json!({"s":subject})];
    for &(field, property, required) in FIELDS {
        match fields.get(property) {
            Some(values) => {
                rows = rows
                    .into_iter()
                    .flat_map(|row| {
                        values.iter().map(move |value| {
                            let mut row = row.clone();
                            row[field] = value.clone();
                            row
                        })
                    })
                    .collect();
            }
            // The original query's mandatory triples exclude this subject.
            None if required => return Ok(Vec::new()),
            None => {}
        }
    }
    Ok(rows)
}

pub(super) fn fetch(endpoint: &str) -> Result<Vec<TextRule>> {
    let members = read(endpoint, MEMBERS)?;
    let mut seen = HashSet::new();
    let mut bindings = Vec::new();
    for member in rows_of(&members)? {
        let subject = member.get("s").ok_or_else(|| error("missing subject"))?;
        let iri = binding_value(member, "s").ok_or_else(|| error("missing subject IRI"))?;
        // IRIREF cannot contain these delimiters. Never interpolate graph data
        // as executable SPARQL, and never silently omit an unsupported identity.
        if subject.get("type").and_then(Value::as_str) != Some("uri")
            || !iri.contains(':')
            || iri
                .chars()
                .any(|c| c.is_whitespace() || c.is_control() || "<>\\\"{}|^`".contains(c))
        {
            return Err(error("subject is not a safe absolute IRI"));
        }
        if !seen.insert(iri.clone()) {
            continue;
        }
        let properties = read(
            endpoint,
            &format!("SELECT ?property ?value WHERE {{ <{iri}> ?property ?value }}"),
        )?;
        bindings.extend(expand(subject, rows_of(&properties)?)?);
    }
    decode_text_rules(&json!({"results":{"bindings":bindings}}).to_string())
}

#[cfg(test)]
#[path = "text_fetch_test.rs"]
mod tests;
