//! metrics — the usage spool (aegis-0nng: the measurement layer for the
//! aegis-m9ln policy circuit).
//!
//! One JSON line per event, appended to a local spool that a textfile
//! converter turns into Prometheus counters. A spool and NOT a /metrics
//! endpoint on purpose: the pre-edit hook is a short-lived per-edit process
//! with nothing to scrape, and coupling its numbers to the resident daemon
//! would make the metrics vanish exactly when the daemon is down — the moment
//! they matter most. The daemon can grow a /metrics later; the spool is the
//! form that is true for every process shape yupana runs as.
//!
//! ABSOLUTE FAIL-SILENCE, stricter than the guard's own fail-open: a metrics
//! write must never change a guard outcome, never block an edit, never print.
//! The night this was designed, ENOSPC killed a supervisor through an uncaught
//! error in a bookkeeping write — a metrics layer that can take enforcement
//! down with it measures negative value. Every error here is swallowed whole.
//!
//! Event kinds and their fields (the label taxonomy agreed on aegis-0nng —
//! quipu's own vocabulary on the quipu side, the guard's REAL Outcome enum on
//! this side, and "allowed clean" vs "allowed because unguarded" never share
//! a label):
//!   guard     {result: allow|deny|notify, `duration_ms`, ext, mode,
//!              path?, rule?}                                    every pre-edit
//!   `fail_open` {`fail_kind`}              the guard degraded, and why-kind
//!   governed  {rules: [...], structural: n, blocking, exposure,
//!              repo}                                            a rule spoke
//!   command   {cmd}                      DELIBERATE use — the leverage signal
//! Every line also carries ts (unix secs), agent (`$SHANTY_AGENT`) and tenant
//! (`$BOBBIN_ROLE`), the two identity envs every st launch exports.

use std::path::PathBuf;

/// Ceiling before the spool rotates to `<name>.old` (one slot, replace). The
/// converter reads both. Unbounded growth on a 96%-full host is how this
/// feature would recreate the incident that motivated its own fail-silence.
const ROTATE_BYTES: u64 = 64 * 1024 * 1024;

/// Where the spool lives: `$YUPANA_METRICS_PATH`, else
/// `$XDG_STATE_HOME/yupana/metrics.jsonl`, else `~/.local/state/yupana/metrics.jsonl`.
/// Pure so the precedence is testable without touching the process environment
/// (parallel tests race on env vars).
pub fn resolve_path(
    explicit: Option<&str>,
    xdg_state: Option<&str>,
    home: Option<&str>,
) -> Option<PathBuf> {
    if let Some(p) = explicit {
        return Some(PathBuf::from(p));
    }
    if let Some(x) = xdg_state {
        return Some(PathBuf::from(x).join("yupana").join("metrics.jsonl"));
    }
    home.map(|h| {
        PathBuf::from(h)
            .join(".local")
            .join("state")
            .join("yupana")
            .join("metrics.jsonl")
    })
}

fn spool_path() -> Option<PathBuf> {
    resolve_path(
        std::env::var("YUPANA_METRICS_PATH").ok().as_deref(),
        std::env::var("XDG_STATE_HOME").ok().as_deref(),
        std::env::var("HOME").ok().as_deref(),
    )
}

/// Append one event line to the spool. Swallows every error by contract.
pub fn emit(kind: &str, fields: &[(&str, serde_json::Value)]) {
    let Some(path) = spool_path() else { return };
    emit_to(&path, kind, fields);
}

/// The writable core, path injected — the seam tests use.
pub fn emit_to(path: &std::path::Path, kind: &str, fields: &[(&str, serde_json::Value)]) {
    // Never let a panic escape: this is bookkeeping about enforcement, not
    // enforcement. catch_unwind is cheap at this call rate (~1/edit).
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut obj = serde_json::Map::new();
        obj.insert("kind".into(), kind.into());
        obj.insert(
            "ts".into(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs())
                .into(),
        );
        if let Ok(agent) = std::env::var("SHANTY_AGENT") {
            obj.insert("agent".into(), agent.into());
        }
        if let Ok(tenant) = std::env::var("BOBBIN_ROLE") {
            obj.insert("tenant".into(), tenant.into());
        }
        // The WORK ITEM this action belongs to (aegis-qdjof). Absent when
        // UNKNOWN rather than recorded as null or a guess: a record that omits
        // the field is honestly silent, whereas one asserting the wrong item
        // would be replayed later as justification for a rule.
        if let Some(item) = crate::plate::current() {
            obj.insert("item".into(), item.into());
        }
        for (k, v) in fields {
            obj.insert((*k).into(), v.clone());
        }
        let Ok(line) = serde_json::to_string(&serde_json::Value::Object(obj)) else {
            return;
        };

        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        // Rotate BEFORE appending, one slot: the converter owns compaction of
        // `.old`; we own not filling the disk.
        if let Ok(meta) = std::fs::metadata(path) {
            if meta.len() > ROTATE_BYTES {
                let _ = std::fs::rename(path, path.with_extension("jsonl.old"));
            }
        }
        use std::io::Write;
        // O_APPEND: one small write per line — atomic enough for a line-oriented
        // reader on the same host; a torn tail line is skipped by the converter
        // (the same one-corrupt-record-must-not-dam rule as ev-172).
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            let _ = writeln!(f, "{line}");
        }
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn precedence_explicit_then_xdg_then_home() {
        assert_eq!(
            resolve_path(Some("/x/m.jsonl"), Some("/s"), Some("/h")).unwrap(),
            PathBuf::from("/x/m.jsonl")
        );
        assert_eq!(
            resolve_path(None, Some("/s"), Some("/h")).unwrap(),
            PathBuf::from("/s/yupana/metrics.jsonl")
        );
        assert_eq!(
            resolve_path(None, None, Some("/h")).unwrap(),
            PathBuf::from("/h/.local/state/yupana/metrics.jsonl")
        );
        assert!(resolve_path(None, None, None).is_none());
    }

    #[test]
    fn a_line_is_valid_json_with_kind_and_fields() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("m.jsonl");
        emit_to(
            &p,
            "guard",
            &[("result", "deny".into()), ("duration_ms", 12.into())],
        );
        let text = std::fs::read_to_string(&p).unwrap();
        let v: serde_json::Value = serde_json::from_str(text.trim()).unwrap();
        assert_eq!(v["kind"], "guard");
        assert_eq!(v["result"], "deny");
        assert_eq!(v["duration_ms"], 12);
        assert!(v["ts"].as_u64().unwrap() > 0);
    }

    #[test]
    fn lines_append_never_replace() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("m.jsonl");
        emit_to(&p, "command", &[("cmd", "impact".into())]);
        emit_to(&p, "command", &[("cmd", "verify".into())]);
        assert_eq!(std::fs::read_to_string(&p).unwrap().lines().count(), 2);
    }

    #[test]
    fn an_unwritable_path_is_swallowed_whole() {
        // The contract that outranks all others: bookkeeping must never break
        // enforcement. A directory-as-file path cannot be opened for append —
        // and nothing may escape.
        let dir = tempfile::tempdir().unwrap();
        emit_to(dir.path(), "guard", &[("result", "allow".into())]);
        // reaching here IS the assertion (no panic, no error, no output)
    }

    #[test]
    fn the_spool_rotates_at_the_ceiling_instead_of_growing() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("m.jsonl");
        std::fs::write(&p, vec![b'x'; (ROTATE_BYTES + 1) as usize]).unwrap();
        emit_to(&p, "guard", &[("result", "allow".into())]);
        assert!(
            p.with_extension("jsonl.old").exists(),
            "the big file rolled aside"
        );
        assert_eq!(
            std::fs::read_to_string(&p).unwrap().lines().count(),
            1,
            "the fresh spool holds only the new line"
        );
    }
}
