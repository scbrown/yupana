//! The client seam — how a thin client learns whether the resident daemon is
//! reachable, and is FORCED to notice when it is not (aegis-1qze).
//!
//! This is the load-bearing safety piece of stage 1. When the guard becomes a
//! thin client of the daemon (stage 3), "the daemon did not answer" must never be
//! quietly treated as "allow" — killing the daemon would otherwise be the cheapest
//! way to disable the guard on every edit at once. So a probe returns
//! [`Reachability`], whose `Down` variant carries a reason and cannot be folded
//! into a success by accident: there is no `Default`, no `unwrap_or(allow)` shape,
//! and the type makes a caller pattern-match both arms.
//!
//! Deliberately dependency-free (`std::net` only, no async, no `reqwest`): the
//! pre-edit hook runs synchronously on stdin, so the probe it will call in stage 3
//! must work in that context. A raw HTTP/1.1 `GET /health` over a connect-timed
//! `TcpStream` is enough to answer the only question here — "is a yupana daemon
//! listening and healthy at this address?"

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::Path;
use std::time::Duration;

use super::{Definitions, Impact, MeasureReply, Neighbors};
use crate::config::YupanaConfig;
use crate::graph::Dir;

/// Whether a resident daemon answered a liveness probe.
///
/// `Down` is a first-class outcome, not an error to be `?`-propagated away: the
/// whole point is that a caller must SEE it and decide loudly (fall back to a
/// transient build, and say the guard is running unguarded-by-daemon), never
/// silently allow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reachability {
    /// The daemon answered `/health` with a 2xx.
    Up,
    /// No healthy daemon at this address. The reason is for the operator-facing
    /// notice, not for control flow — every `Down`, whatever the reason, means
    /// "there is no resident guard here right now".
    Down(String),
}

impl Reachability {
    /// True only when a healthy daemon answered. Named so call sites read as a
    /// question, and so the `Down` case is never the implicit `else`.
    #[must_use]
    pub fn is_up(&self) -> bool {
        matches!(self, Reachability::Up)
    }

    /// The reason a probe came back `Down`, for a user-visible notice.
    #[must_use]
    pub fn down_reason(&self) -> Option<&str> {
        match self {
            Reachability::Up => None,
            Reachability::Down(why) => Some(why),
        }
    }
}

/// Probe `http://<host>:<port>/health`. Returns [`Reachability::Up`] only on a
/// 2xx; any connect error, timeout, or non-2xx is [`Reachability::Down`] with a
/// reason. Never panics, never blocks longer than `timeout` on connect.
#[must_use]
pub fn probe(host: &str, port: u16, timeout: Duration) -> Reachability {
    let addr = format!("{host}:{port}");
    let Ok(mut addrs) = addr.to_socket_addrs() else {
        return Reachability::Down(format!("cannot resolve {addr}"));
    };
    let Some(sockaddr) = addrs.next() else {
        return Reachability::Down(format!("no address for {addr}"));
    };

    let mut stream = match TcpStream::connect_timeout(&sockaddr, timeout) {
        Ok(s) => s,
        // Connection refused is the common, important case: no daemon is
        // listening. Named plainly so the notice reads "no daemon", not a raw
        // errno.
        Err(e) => return Reachability::Down(format!("no daemon at {addr} ({e})")),
    };
    let _ = stream.set_read_timeout(Some(timeout));
    let _ = stream.set_write_timeout(Some(timeout));

    let req = format!("GET /health HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n");
    if let Err(e) = stream.write_all(req.as_bytes()) {
        return Reachability::Down(format!("write to {addr} failed ({e})"));
    }

    // The status line is all we need: `HTTP/1.1 200 ...`.
    let mut buf = Vec::with_capacity(64);
    let mut byte = [0u8; 1];
    loop {
        match stream.read(&mut byte) {
            Ok(0) => break,
            Ok(_) => {
                if byte[0] == b'\n' || buf.len() >= 64 {
                    break;
                }
                buf.push(byte[0]);
            }
            Err(e) => return Reachability::Down(format!("read from {addr} failed ({e})")),
        }
    }
    let line = String::from_utf8_lossy(&buf);
    // "HTTP/1.1 200 OK" -> the middle token is the status code.
    match line.split_whitespace().nth(1) {
        Some(code) if code.starts_with('2') => Reachability::Up,
        Some(code) => Reachability::Down(format!("daemon at {addr} returned HTTP {code}")),
        None => Reachability::Down(format!("daemon at {addr} sent no status line")),
    }
}

/// Ask the resident daemon to size an edit: `POST /measure`. Returns the
/// [`MeasureReply`] on a 2xx, or an `Err(reason)` on ANY other outcome —
/// connection refused (no daemon), a non-2xx (e.g. the file is outside the
/// daemon's root, so it serves a different repo), a timeout, or an unparseable
/// body. The caller MUST treat `Err` as "the daemon could not size this edit" and
/// fall back — never as "allow". Same `std::net`-only, synchronous shape as
/// [`probe`], because the pre-edit hook that calls this runs sync on stdin.
///
/// The reason string distinguishes the failures (a connection error reads
/// differently from an "HTTP 400"), so a caller can say WHY the daemon was
/// unusable in its loud notice — down vs. serving-another-repo are different
/// operator problems.
pub fn fetch_measure(
    host: &str,
    port: u16,
    file: &str,
    rel: &str,
    anchors: &[String],
    max_hops: u32,
    timeout: Duration,
) -> Result<MeasureReply, String> {
    let addr = format!("{host}:{port}");
    let body = serde_json::json!({
        "file": file,
        "rel": rel,
        "anchors": anchors,
        "max_hops": max_hops,
    })
    .to_string();

    let raw = http_post(host, port, "/measure", &body, timeout)?;
    serde_json::from_str::<MeasureReply>(&raw)
        .map_err(|e| format!("daemon at {addr} sent an unparseable measure reply ({e})"))
}

/// Raw synchronous HTTP POST of a JSON body: the reply body on a 2xx,
/// `Err(reason)` on any other outcome. Same `std::net`-only shape and same
/// contract as [`http_get`].
fn http_post(
    host: &str,
    port: u16,
    path: &str,
    body: &str,
    timeout: Duration,
) -> Result<String, String> {
    let addr = format!("{host}:{port}");
    let Ok(mut addrs) = addr.to_socket_addrs() else {
        return Err(format!("cannot resolve {addr}"));
    };
    let Some(sockaddr) = addrs.next() else {
        return Err(format!("no address for {addr}"));
    };
    let mut stream = TcpStream::connect_timeout(&sockaddr, timeout)
        .map_err(|e| format!("no daemon at {addr} ({e})"))?;
    let _ = stream.set_read_timeout(Some(timeout));
    let _ = stream.set_write_timeout(Some(timeout));

    let req = format!(
        "POST {path} HTTP/1.1\r\nHost: {host}\r\nContent-Type: application/json\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream
        .write_all(req.as_bytes())
        .map_err(|e| format!("write to {addr} failed ({e})"))?;

    let mut raw = String::new();
    stream
        .read_to_string(&mut raw)
        .map_err(|e| format!("read from {addr} failed ({e})"))?;

    let status = raw
        .split_whitespace()
        .nth(1)
        .ok_or_else(|| format!("daemon at {addr} sent no status line"))?;
    if !status.starts_with('2') {
        return Err(format!("daemon at {addr} returned HTTP {status}"));
    }
    raw.split("\r\n\r\n")
        .nth(1)
        .map(str::to_string)
        .ok_or_else(|| format!("daemon at {addr} sent no body"))
}

/// Raw synchronous HTTP GET: the body on a 2xx, `Err(reason)` on any other
/// outcome. The shared shape under the typed fetches below — same
/// `std::net`-only approach as [`probe`], and the same contract: an error is
/// "the daemon could not answer", to be handled by falling back, never ignored.
pub(super) fn http_get(
    host: &str,
    port: u16,
    path: &str,
    timeout: Duration,
) -> Result<String, String> {
    let addr = format!("{host}:{port}");
    let Ok(mut addrs) = addr.to_socket_addrs() else {
        return Err(format!("cannot resolve {addr}"));
    };
    let Some(sockaddr) = addrs.next() else {
        return Err(format!("no address for {addr}"));
    };
    let mut stream = TcpStream::connect_timeout(&sockaddr, timeout)
        .map_err(|e| format!("no daemon at {addr} ({e})"))?;
    let _ = stream.set_read_timeout(Some(timeout));
    let _ = stream.set_write_timeout(Some(timeout));

    let req = format!("GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n");
    stream
        .write_all(req.as_bytes())
        .map_err(|e| format!("write to {addr} failed ({e})"))?;
    let mut raw = String::new();
    stream
        .read_to_string(&mut raw)
        .map_err(|e| format!("read from {addr} failed ({e})"))?;

    let status = raw
        .split_whitespace()
        .nth(1)
        .ok_or_else(|| format!("daemon at {addr} sent no status line"))?;
    if !status.starts_with('2') {
        return Err(format!("daemon at {addr} returned HTTP {status}"));
    }
    raw.split("\r\n\r\n")
        .nth(1)
        .map(str::to_string)
        .ok_or_else(|| format!("daemon at {addr} sent no body"))
}

/// Percent-encode a query-string value. Symbols are code identifiers so this is
/// nearly always a no-op, but a name must never be able to smuggle `&`/`?`/
/// spaces into the request line.
pub(super) fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// The daemon's full `/status` body as JSON. `Err` means "no usable daemon",
/// handled by the caller's fallback.
pub fn fetch_status_json(
    host: &str,
    port: u16,
    timeout: Duration,
) -> Result<serde_json::Value, String> {
    let body = http_get(host, port, "/status", timeout)?;
    serde_json::from_str(&body)
        .map_err(|e| format!("daemon at {host}:{port} sent an unparseable status ({e})"))
}

/// The analysis root the daemon at `host:port` serves, from `GET /status`. A
/// thin client MUST check this against its own root before trusting any graph
/// answer — a daemon for repo B answering repo A's query would not error, it
/// would confidently lie. `Err` means "no usable daemon", handled by fallback.
pub fn fetch_root(host: &str, port: u16, timeout: Duration) -> Result<String, String> {
    let body = http_get(host, port, "/status", timeout)?;
    let status: serde_json::Value = serde_json::from_str(&body)
        .map_err(|e| format!("daemon at {host}:{port} sent an unparseable status ({e})"))?;
    status
        .get("root")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("daemon at {host}:{port} status reports no root"))
}

/// Direct callers/callees of `symbol` from the RESIDENT graph (`GET /callers`
/// or `/callees`). `Err` on any failure — the caller falls back to a transient
/// build, never treats it as an empty answer (that would collapse "daemon down"
/// into "no callers", the fact-vs-absence bug).
pub fn fetch_neighbors(
    host: &str,
    port: u16,
    symbol: &str,
    dir: Dir,
    timeout: Duration,
) -> Result<Neighbors, String> {
    let endpoint = match dir {
        Dir::Callers => "callers",
        Dir::Callees => "callees",
    };
    let body = http_get(
        host,
        port,
        &format!("/{endpoint}?symbol={}", urlencode(symbol)),
        timeout,
    )?;
    serde_json::from_str(&body)
        .map_err(|e| format!("daemon at {host}:{port} sent an unparseable {endpoint} reply ({e})"))
}

/// Blast radius of `symbol` from the RESIDENT graph (`GET /impact`). Same
/// contract as [`fetch_neighbors`]: `Err` means fall back, never "no impact".
pub fn fetch_impact(
    host: &str,
    port: u16,
    symbol: &str,
    hops: u32,
    timeout: Duration,
) -> Result<Impact, String> {
    let body = http_get(
        host,
        port,
        &format!("/impact?symbol={}&hops={hops}", urlencode(symbol)),
        timeout,
    )?;
    serde_json::from_str(&body)
        .map_err(|e| format!("daemon at {host}:{port} sent an unparseable impact reply ({e})"))
}

/// The daemon address from `config`, IF one is expected AND it serves `root`.
///
/// Three-state on purpose — the caller's loudness depends on which:
/// - `None`: no daemon expected (`use_daemon = false`, the default). Absence
///   is normal; stay silent.
/// - `Some(Err(reason))`: a daemon IS expected but unusable — down,
///   unparseable, or serving a DIFFERENT root (a daemon for repo B answering
///   repo A would not error, it would confidently lie, so the root must match
///   canonicalized). The caller falls back and reports the reason.
/// - `Some(Ok((host, port)))`: usable — query it.
pub fn expected_same_root_daemon(
    config: &YupanaConfig,
    root: &Path,
    timeout: Duration,
) -> Option<Result<(String, u16), String>> {
    if !config.serve.use_daemon {
        return None;
    }
    let host = config.serve.bind_address.clone();
    let port = config.serve.mcp_http_port;
    let served = match fetch_root(&host, port, timeout) {
        Ok(served) => served,
        Err(reason) => return Some(Err(reason)),
    };
    let same = match (Path::new(&served).canonicalize(), root.canonicalize()) {
        (Ok(theirs), Ok(ours)) => theirs == ours,
        _ => false,
    };
    if !same {
        return Some(Err(format!(
            "daemon at {host}:{port} serves {served}, not {}",
            root.display()
        )));
    }
    Some(Ok((host, port)))
}

/// Feed the daemon's overlay for `tenant` with the just-saved `rel` and get
/// the FR-30 advisory back (`POST /edit`; the daemon reads the file from disk,
/// root-confined). `Err` means fall back to the transient advisory — the edit
/// is then NOT recorded in any overlay, which is fine: the overlay is a cache
/// of the tenant's edits, not the system of record (the file on disk is).
pub fn fetch_edit(
    host: &str,
    port: u16,
    tenant: &str,
    rel: &str,
    timeout: Duration,
) -> Result<super::EditReply, String> {
    let addr = format!("{host}:{port}");
    let body = serde_json::json!({ "tenant": tenant, "rel": rel }).to_string();
    let raw = http_post(host, port, "/edit", &body, timeout)?;
    serde_json::from_str(&raw)
        .map_err(|e| format!("daemon at {addr} sent an unparseable edit reply ({e})"))
}

/// Definition sites of `symbol` from the RESIDENT node index
/// (`GET /references`). Same contract as [`fetch_neighbors`]: `Err` means fall
/// back to the transient walk, never "no definitions".
pub fn fetch_references(
    host: &str,
    port: u16,
    symbol: &str,
    timeout: Duration,
) -> Result<Definitions, String> {
    let body = http_get(
        host,
        port,
        &format!("/references?symbol={}", urlencode(symbol)),
        timeout,
    )?;
    serde_json::from_str(&body)
        .map_err(|e| format!("daemon at {host}:{port} sent an unparseable references reply ({e})"))
}

#[cfg(test)]
// Test names here shout the invariant they pin — `is_NEVER_observable`,
// `daemon_EXPECTED_but_DOWN`, `is_DOWN_not_UP`. That capitalisation is the same
// emphasis the prose and comments use throughout this repo, and it is load-
// bearing in a test name: it says which word the assertion turns on. Allowed
// explicitly, and scoped to tests, so the lint stays live everywhere else
// rather than being switched off crate-wide (yupana #83).
#[allow(non_snake_case)]
mod tests {
    use super::*;

    #[test]
    fn a_closed_port_is_DOWN_with_a_reason_not_a_panic() {
        // The exact case that must never be silently "allow": nothing is
        // listening. Port 1 is reserved and never open.
        let r = probe("127.0.0.1", 1, Duration::from_millis(200));
        assert!(!r.is_up());
        assert!(
            r.down_reason().is_some(),
            "Down must carry a reason for the notice"
        );
    }

    #[test]
    fn an_unroutable_host_is_DOWN_not_UP() {
        // A resolve/connect failure is Down, never mistaken for Up.
        let r = probe(
            "no.such.host.yupana.invalid",
            3040,
            Duration::from_millis(200),
        );
        assert!(!r.is_up());
    }

    #[test]
    fn urlencode_passes_identifiers_and_encodes_request_line_metacharacters() {
        // The common case is a no-op; the point is that a symbol name cannot
        // smuggle `&`/`?`/spaces into the HTTP request line.
        assert_eq!(urlencode("authenticate_v2"), "authenticate_v2");
        assert_eq!(urlencode("a&b ?c"), "a%26b%20%3Fc");
    }

    #[test]
    fn fetch_neighbors_and_impact_on_a_closed_port_are_Err_never_empty() {
        // "Daemon down" must be an Err the caller handles by falling back —
        // never a parse into an empty answer (fact-vs-absence).
        let n = fetch_neighbors(
            "127.0.0.1",
            1,
            "leaf",
            Dir::Callers,
            Duration::from_millis(200),
        );
        assert!(n.is_err());
        let i = fetch_impact("127.0.0.1", 1, "leaf", 5, Duration::from_millis(200));
        assert!(i.is_err());
        let r = fetch_root("127.0.0.1", 1, Duration::from_millis(200));
        assert!(r.is_err());
    }

    #[test]
    fn down_and_up_are_distinct_and_reason_only_on_down() {
        assert!(Reachability::Up.is_up());
        assert_eq!(Reachability::Up.down_reason(), None);
        let d = Reachability::Down("no daemon".into());
        assert!(!d.is_up());
        assert_eq!(d.down_reason(), Some("no daemon"));
    }
}
