//! First-class resident entity keywords shared by read/edit/briefing clients.
//! Matching ports DP's ASCII boundaries, longest-first overlap and stoplist.
use std::collections::{BTreeMap, BTreeSet};

use aho_corasick::AhoCorasick;
use serde::{Deserialize, Serialize};

/// One governed entity label; several IRIs under one label remain ambiguous.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Entry {
    pub iri: String,
    pub label: String,
    #[serde(rename = "type")]
    pub kind: String,
}

/// An observed relation, never inferred from co-occurrence.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Edge {
    pub source: String,
    pub predicate: String,
    pub target: String,
}

/// A complete replacement, carrying the observation time rather than load time.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Snapshot {
    pub generated_at: u64,
    pub entries: Vec<Entry>,
    #[serde(default)]
    pub edges: Vec<Edge>,
}

/// A bounded query against resident data. Budget must come from the pipeline.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Request {
    pub session_id: String,
    pub text: String,
    #[serde(default)]
    pub reference: String,
    pub remaining_bytes: usize,
}

/// Model context plus scoring-compatible shown keys; no source text echoed.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Reply {
    pub context: String,
    pub shown: Vec<String>,
    pub raw_hits: usize,
    pub generated_at: u64,
}

/// Compiled once on snapshot replacement, never once per hook.
pub struct KeywordIndex {
    matcher: AhoCorasick,
    keys: Vec<String>,
    entities: Vec<Vec<Entry>>,
    snapshot: Snapshot,
}

impl KeywordIndex {
    pub fn compile(snapshot: Snapshot, dictionary: &BTreeSet<String>) -> Result<Self, String> {
        if snapshot.entries.iter().any(|e| {
            e.label.len() > 4096
                || e.iri.len() > 4096
                || e.kind.len() > 64
                || e.kind.chars().any(char::is_control)
        }) || snapshot.edges.iter().any(|e| {
            e.source.len() > 4096
                || e.target.len() > 4096
                || e.predicate.len() > 4096
                || e.predicate.chars().any(char::is_control)
        }) {
            return Err("keyword entry exceeds bounds".into());
        }
        let mut groups: BTreeMap<String, Vec<Entry>> = BTreeMap::new();
        for entry in &snapshot.entries {
            if !admissible(&entry.label, dictionary) {
                continue;
            }
            let group = groups.entry(key(&entry.label)).or_default();
            if !group.iter().any(|e| e.iri == entry.iri) {
                group.push(entry.clone());
            }
        }
        let keys: Vec<_> = groups.keys().cloned().collect();
        let matcher = AhoCorasick::builder()
            .ascii_case_insensitive(true)
            .build(&keys)
            .map_err(|e| e.to_string())?;
        Ok(Self {
            matcher,
            keys,
            entities: groups.into_values().collect(),
            snapshot,
        })
    }

    pub fn render(&self, request: &Request, seen: &BTreeSet<String>) -> Reply {
        const HEADER: &str = "Quipu entities here: ";
        let budget = request.remaining_bytes.min(600);
        let text = &request.text;
        let mut candidates: Vec<_> = self
            .matcher
            .find_overlapping_iter(text)
            .take(32_769)
            .filter(|m| {
                boundary(text.as_bytes(), m.start(), m.end())
                    && text.is_char_boundary(m.start())
                    && text.is_char_boundary(m.end())
            })
            .collect();
        if candidates.len() > 32_768 {
            return Reply {
                generated_at: self.snapshot.generated_at,
                ..Reply::default()
            };
        }
        candidates.sort_by_key(|m| (m.start(), std::cmp::Reverse(m.len())));
        let mut last_end = 0;
        let mut taken = BTreeSet::new();
        let mut reply = Reply {
            generated_at: self.snapshot.generated_at,
            ..Reply::default()
        };
        let mut selected = Vec::new();
        let mut full = false;
        for m in candidates {
            if m.start() < last_end {
                continue;
            }
            last_end = m.end();
            let id = m.pattern().as_usize();
            if !taken.insert(id) {
                continue;
            }
            let entries = &self.entities[id];
            if entries.len() == 1
                && !request.reference.is_empty()
                && entries[0].iri.contains(&request.reference)
            {
                continue;
            }
            reply.raw_hits += 1;
            if full || reply.shown.len() >= 5 || seen.contains(&self.keys[id]) {
                continue;
            }
            let label = &text[m.start()..m.end()];
            let part = if entries.len() > 1 {
                format!("{label} (ambiguous {})", entries.len())
            } else {
                format!("{label} ({})", entries[0].kind)
            };
            let prefix = if reply.context.is_empty() {
                HEADER
            } else {
                ", "
            };
            if reply.context.len() + prefix.len() + part.len() > budget {
                full = true;
                continue;
            }
            reply.context.push_str(prefix);
            reply.context.push_str(&part);
            reply.shown.push(self.keys[id].clone());
            if entries.len() == 1 {
                selected.push((&entries[0].iri, label));
            }
        }
        // Ambiguous mentions cannot name relation endpoints. At most ten pairs
        // and two existing directed edges are displayed inside the SAME budget.
        let mut count = 0;
        for edge in &self.snapshot.edges {
            if count == 2 {
                break;
            }
            let source = selected.iter().find(|(iri, _)| **iri == edge.source);
            let target = selected.iter().find(|(iri, _)| **iri == edge.target);
            if let (Some((_, a)), Some((_, b))) = (source, target) {
                let predicate = edge.predicate.rsplit(['#', '/', ':']).next().unwrap_or("");
                let part = format!("; {a} --{predicate}--> {b}");
                if reply.context.len() + part.len() <= budget {
                    reply.context.push_str(&part);
                    count += 1;
                }
            }
        }
        reply
    }
}

pub(crate) fn key(label: &str) -> String {
    label.trim_matches([' ', '\t', '\n']).to_ascii_lowercase()
}

fn boundary(text: &[u8], start: usize, end: usize) -> bool {
    let word = |b: u8| b.is_ascii_alphanumeric() || b == b'_' || b == b'-';
    (start == 0 || !word(text[start - 1]))
        && (end == text.len() || !word(text[end]))
        && !(end + 1 < text.len() && text[end] == b'.' && text[end + 1].is_ascii_alphanumeric())
}

/// DP's evidence-based precision filter, applied at resident replacement.
pub fn admissible(label: &str, dictionary: &BTreeSet<String>) -> bool {
    let label = label.trim();
    let lower = label.to_lowercase();
    const STOP: &[&str] = &[
        "github.com",
        "gitlab.com",
        "raw.githubusercontent.com",
        "api.github.com",
        "docs.rs",
        "crates.io",
        "pkg.go.dev",
        "pypi.org",
        "npmjs.com",
        "localhost",
        "code-review",
        "deploy.yml",
    ];
    if label.len() < 4 || label.chars().any(char::is_control) || STOP.contains(&lower.as_str()) {
        return false;
    }
    let words: Vec<_> = label.split_whitespace().collect();
    if words.len() >= 2 {
        return words.iter().any(|w| {
            let w = w.to_lowercase();
            ![
                "the", "and", "for", "with", "from", "this", "that", "into", "over",
            ]
            .contains(&w.as_str())
                && !dictionary.contains(&w)
        });
    }
    !dictionary.contains(&lower)
        && (label.bytes().any(|b| b"-_./0123456789".contains(&b))
            || label
                .as_bytes()
                .windows(2)
                .any(|w| w[0].is_ascii_lowercase() && w[1].is_ascii_uppercase()))
}

#[cfg(test)]
#[path = "keywords_test.rs"]
mod tests;
