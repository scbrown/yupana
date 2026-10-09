//! Resident keyword snapshot, atomic refresh, and bounded session dedup.
//! No hook request triggers a remote query or compiles a matcher.
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::keywords::{KeywordIndex, Reply, Request, Snapshot};

const MAX_AGE: u64 = 600;
const MAX_SESSIONS: usize = 256;
const MAX_SEEN: usize = 8192;

#[derive(Default)]
struct State {
    index: Option<KeywordIndex>,
    sessions: BTreeMap<String, (Instant, BTreeSet<String>)>,
}

/// Shared resident data. Other feedback clients use the same index API.
#[derive(Clone, Default)]
pub struct ResidentKeywords(Arc<Mutex<State>>);

impl ResidentKeywords {
    /// Compile off-lock, then replace atomically. Failed updates leave truth
    /// and its ORIGINAL observation time intact, so expiry still means expiry.
    pub fn replace(&self, snapshot: Snapshot, dict: &BTreeSet<String>) -> Result<(), String> {
        if snapshot.entries.len() > 50_000 || snapshot.edges.len() > 100_000 {
            return Err("keyword snapshot exceeds resident bounds".into());
        }
        let index = KeywordIndex::compile(snapshot, dict)?;
        self.0.lock().map_err(|_| "keyword lock poisoned")?.index = Some(index);
        Ok(())
    }

    /// Read resident memory only. Missing/expired data is unavailable, not an
    /// empty successful catalogue; the advisory hook chooses silence on error.
    pub fn query(&self, request: &Request, now: u64) -> Result<Reply, String> {
        if request.text.len() > 128 * 1024
            || request.session_id.len() > 256
            || request.reference.len() > 4096
        {
            return Err("keyword request exceeds bounds".into());
        }
        if request.session_id.is_empty() {
            return Err("keyword query requires session id".into());
        }
        let mut state = self.0.lock().map_err(|_| "keyword lock poisoned")?;
        state
            .sessions
            .retain(|_, (t, _)| t.elapsed() < Duration::from_secs(86_400));
        let empty = BTreeSet::new();
        let seen = state
            .sessions
            .get(&request.session_id)
            .map_or(&empty, |(_, s)| s);
        let index = state.index.as_ref().ok_or("no keyword snapshot")?;
        let reply = index.render(request, seen);
        if reply.generated_at > now || now - reply.generated_at > MAX_AGE {
            return Err("keyword snapshot expired or future-dated".into());
        }
        if !reply.shown.is_empty() {
            // Never evict active session state and redisplay its keywords. A
            // saturated dedup store refuses new output until sessions expire.
            if !state.sessions.contains_key(&request.session_id)
                && state.sessions.len() >= MAX_SESSIONS
            {
                return Err("keyword session capacity exhausted".into());
            }
            let session = state
                .sessions
                .entry(request.session_id.clone())
                .or_insert_with(|| (Instant::now(), BTreeSet::new()));
            if session.1.len() + reply.shown.len() > MAX_SEEN {
                return Err("keyword dedup capacity exhausted".into());
            }
            session.1.extend(reply.shown.iter().cloned());
        }
        Ok(reply)
    }
}

/// The background loader is optional and never enabled by a read payload.
#[cfg(feature = "quipu")]
pub fn spawn_for(engine: &super::ResidentEngine) -> Option<crate::keyword_refresh::RefreshHandle> {
    let namespace = std::env::var("YUPANA_KEYWORDS_NAMESPACE").ok()?;
    let config = engine.config();
    if !config.quipu.enabled || config.quipu.endpoint.is_empty() {
        return None;
    }
    Some(crate::keyword_refresh::spawn(
        engine.keywords().clone(),
        config.quipu.endpoint.clone(),
        namespace,
    ))
}
