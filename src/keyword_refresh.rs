//! Single background producer for first-class resident labels and relations.
//! Complete snapshots only; no remote fetch on the hook request path.
use std::collections::BTreeSet;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::Duration;

use crate::daemon::keywords::ResidentKeywords;
use crate::keywords::{Edge, Entry, Snapshot};

// Ported DP governed entity classes. Formula/bulk code/credential kinds excluded.
const CLASSES: &str = "Host BareMetalHost ProxmoxNode LXCContainer DockerContainer RemoteHost Service SystemdService SystemdTimer DatabaseService SearchService ExternalService MCPServer GitRepo CrewMember CrewRole Rig Skill CLI AgentTool Script AlertRule CronJob PushgatewayJob Metric MonitoringProbe ReverseProxyRoute NetworkSegment StoragePool NFSExport ConfigFile Directive FailureMode FailurePattern Incident Feature Decision Component Capability";
const LABEL: &str = "http://www.w3.org/2000/01/rdf-schema#label";

/// Decode Quipu's native result; an error/missing rows/truncation is NEVER zero.
fn rows(raw: &str) -> Result<Vec<serde_json::Value>, String> {
    let value: serde_json::Value = serde_json::from_str(raw).map_err(|e| e.to_string())?;
    if value.get("error").is_some() || value["truncated"] == true {
        return Err("keyword refresh failed or truncated".into());
    }
    if let Some(rows) = value["results"]["bindings"].as_array() {
        return rows
            .iter()
            .map(|row| {
                let object = row.as_object().ok_or("invalid keyword binding")?;
                let mut flat = serde_json::Map::new();
                for (key, term) in object {
                    flat.insert(
                        key.clone(),
                        term.get("value")
                            .cloned()
                            .ok_or("keyword binding missing value")?,
                    );
                }
                Ok(serde_json::Value::Object(flat))
            })
            .collect();
    }
    value["rows"]
        .as_array()
        .cloned()
        .ok_or_else(|| "keyword query has no rows".into())
}

/// Fetch a complete snapshot, bounded by class, row count and total deadline.
/// A failed class never removes part of the last good snapshot.
pub(crate) fn fetch(endpoint: &str, namespace: &str) -> Result<Snapshot, String> {
    if !namespace.starts_with("http")
        || namespace
            .chars()
            .any(|c| c.is_whitespace() || "<>\"{}\\".contains(c))
    {
        return Err("invalid keyword namespace".into());
    }
    crate::projection_budget::open_budget(Duration::from_secs(60));
    let mut snapshot = Snapshot {
        generated_at: crate::projection_cache::now_secs(),
        entries: Vec::new(),
        edges: Vec::new(),
    };
    let mut known_edges = BTreeSet::new();
    for kind in CLASSES.split_whitespace() {
        // Only edges FROM explicitly selected entity classes. No whole-graph
        // operation; LIMIT and server truncation are checked before replacement.
        let query = format!("SELECT ?s ?l ?p ?o WHERE {{ ?s a <{namespace}{kind}> . ?s <{LABEL}> ?l . OPTIONAL {{ ?s ?p ?o . FILTER(isIRI(?o)) }} }} LIMIT 10001");
        let raw = crate::project::query(endpoint, &query).map_err(|e| e.to_string())?;
        let rows = rows(&raw)?;
        if rows.len() > 10_000 {
            return Err(format!("keyword class {kind} exceeded row bound"));
        }
        let mut known = BTreeSet::new();
        for row in rows {
            let iri = row["s"].as_str().ok_or("keyword row missing entity")?;
            let label = row["l"].as_str().ok_or("keyword row missing label")?;
            if known.insert((iri.to_string(), label.to_string())) {
                snapshot.entries.push(Entry {
                    iri: iri.into(),
                    label: label.into(),
                    kind: kind.into(),
                });
            }
            if let (Some(predicate), Some(target)) = (row["p"].as_str(), row["o"].as_str()) {
                // rdf:type is classification, not an existing inter-entity edge.
                if predicate.ends_with("#type") {
                    continue;
                }
                let edge = (iri.to_string(), predicate.to_string(), target.to_string());
                if known_edges.insert(edge.clone()) {
                    snapshot.edges.push(Edge {
                        source: edge.0,
                        predicate: edge.1,
                        target: edge.2,
                    });
                }
            }
            if snapshot.entries.len() > 50_000 || snapshot.edges.len() > 100_000 {
                return Err("keyword snapshot exceeded bounds".into());
            }
        }
    }
    // Observation is completed now, not when the first serial request began.
    snapshot.generated_at = crate::projection_cache::now_secs();
    Ok(snapshot)
}

/// Signal shutdown without joining a thread that might be awaiting Quipu.
pub struct RefreshHandle(Arc<AtomicBool>);
impl Drop for RefreshHandle {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

pub fn spawn(resident: ResidentKeywords, endpoint: String, namespace: String) -> RefreshHandle {
    let stop = Arc::new(AtomicBool::new(false));
    let thread_stop = stop.clone();
    let _thread = std::thread::Builder::new()
        .name("yupana-keywords".into())
        .spawn(move || {
            let dictionary_path = std::env::var("YUPANA_KEYWORDS_DICT")
                .unwrap_or_else(|_| "/usr/share/dict/words".into());
            let dictionary: BTreeSet<_> = std::fs::read_to_string(dictionary_path)
                .unwrap_or_default()
                .lines()
                .map(|s| s.trim().to_lowercase())
                .collect();
            while !thread_stop.load(Ordering::Relaxed) {
                match fetch(&endpoint, &namespace)
                    .and_then(|snapshot| resident.replace(snapshot, &dictionary))
                {
                    Ok(()) => {}
                    Err(e) => eprintln!("yupana keyword refresh: {e}; prior snapshot unchanged"),
                }
                for _ in 0..1500 {
                    if thread_stop.load(Ordering::Relaxed) {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(200));
                }
            }
        });
    RefreshHandle(stop)
}

#[cfg(test)]
#[path = "keyword_refresh_test.rs"]
mod tests;
