//! Offline structural-policy replay for an auditor supplying an immutable blob.
//! No projection, network, cache, metrics, or verdict spool is consulted.

use std::io::Read;

use serde::{Deserialize, Serialize};

use crate::rules::{self, Rule};

/// Maximum JSON request size, including the supplied source text.
const MAX_REQUEST_BYTES: u64 = 16 * 1024 * 1024;

/// Explicit input: callers must obtain source from the commit being audited.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub rule: Rule,
    pub path: String,
    pub source: String,
}

/// A conclusive answer or an inability to evaluate. Unknown never means pass.
#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Satisfied,
    Unsatisfied,
    Unknown,
    /// The path is not in the selector grammar's language.
    NotApplicable,
}

/// Versioned wire response; path and rule bind the response to its request.
#[derive(Debug, Serialize)]
pub struct Response {
    pub schema_version: u32,
    pub rule: String,
    pub path: String,
    pub verdict: Verdict,
    pub violations: Vec<String>,
    pub errors: Vec<String>,
}

impl Response {
    #[must_use]
    pub fn exit_code(&self) -> i32 {
        match self.verdict {
            Verdict::Satisfied | Verdict::NotApplicable => 0,
            Verdict::Unsatisfied => 1,
            Verdict::Unknown => 2,
        }
    }
}

/// Validate completely before invoking the hook's existing structural engine.
#[must_use]
pub fn evaluate(request: &Request) -> Response {
    let mut result = Response {
        schema_version: 1,
        rule: request.rule.name.clone(),
        path: request.path.clone(),
        verdict: Verdict::Unknown,
        violations: Vec::new(),
        errors: Vec::new(),
    };
    let one = std::slice::from_ref(&request.rule);
    result.errors = rules::errors(one).into_iter().map(|(_, why)| why).collect();
    if request.path.is_empty()
        || request.path.starts_with('/')
        || request
            .path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        result
            .errors
            .push("path must be a normalized repository-relative path".into());
    }
    if request.rule.name.is_empty() {
        result.errors.push("rule name is empty".into());
    }
    if !request.rule.applies(&request.path) {
        result
            .errors
            .push("rule does not apply to the supplied path".into());
    }
    // Compile even for another language's path: an unavailable grammar is
    // unknown, never mistaken for a proven language exclusion.
    if let Err(error) =
        crate::extract::query::run_query_strict("", &request.rule.language, &request.rule.query)
    {
        result.errors.push(error.to_string());
    }
    if !result.errors.is_empty() {
        return result;
    }
    if crate::extract::selectable_language(std::path::Path::new(&request.path))
        != Some(request.rule.language.as_str())
    {
        result.verdict = Verdict::NotApplicable;
        return result;
    }
    if let Err(error) = crate::extract::query::run_query_strict(
        &request.source,
        &request.rule.language,
        &request.rule.query,
    ) {
        result.errors.push(error.to_string());
        return result;
    }
    result.violations =
        rules::evaluate(one, &request.source, &request.rule.language, &request.path)
            .into_iter()
            .map(|violation| violation.message)
            .collect();
    result.verdict = if result.violations.is_empty() {
        Verdict::Satisfied
    } else {
        Verdict::Unsatisfied
    };
    result
}

/// Read a bounded request and print one JSON response. Input errors exit 2 too.
///
/// # Errors
/// Returns an error only if serializing the response fails.
pub fn run() -> anyhow::Result<()> {
    let response = match read_request() {
        Ok(request) => evaluate(&request),
        Err(error) => Response {
            schema_version: 1,
            rule: String::new(),
            path: String::new(),
            verdict: Verdict::Unknown,
            violations: Vec::new(),
            errors: vec![error.to_string()],
        },
    };
    println!("{}", serde_json::to_string(&response)?);
    std::process::exit(response.exit_code());
}

fn read_request() -> anyhow::Result<Request> {
    let mut bytes = Vec::new();
    std::io::stdin()
        .take(MAX_REQUEST_BYTES + 1)
        .read_to_end(&mut bytes)?;
    anyhow::ensure!(
        bytes.len() as u64 <= MAX_REQUEST_BYTES,
        "request exceeds 16 MiB"
    );
    Ok(serde_json::from_slice(&bytes)?)
}

#[cfg(test)]
#[path = "audit_rule_tests.rs"]
mod tests;
