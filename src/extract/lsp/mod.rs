//! Language-agnostic LSP precision client (FR-2/FR-4).
//!
//! The adapter speaks standard JSON-RPC/LSP to a server selected by language;
//! query code never knows whether that server is rust-analyzer or a TypeScript
//! server. Servers are optional: an absent, unresponsive, or build-unresolvable
//! server returns `None`, letting callers serve an explicitly
//! `treesitter`-tagged fallback. One client owns one warm server process and can
//! answer repeated queries without respawning it.

use std::collections::HashSet;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use serde_json::{json, Value};

/// A one-based source position supplied by CLI/MCP callers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Position {
    /// Root-relative source file.
    pub file: String,
    /// One-based line.
    pub line: usize,
    /// One-based UTF-16 column (the LSP coordinate plus one).
    pub column: usize,
}

/// One precise LSP location, normalized to root-relative, one-based fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Location {
    /// Root-relative source file.
    pub file: String,
    /// One-based start line.
    pub start_line: usize,
    /// One-based start column.
    pub start_column: usize,
    /// One-based end line.
    pub end_line: usize,
    /// One-based end column.
    pub end_column: usize,
}

/// Which precise relation to ask from the language server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Query {
    /// `textDocument/definition`.
    Definition,
    /// `textDocument/references` (excluding declarations).
    References,
}

#[derive(Debug, Clone)]
struct Server {
    program: String,
    args: Vec<String>,
    language_id: String,
}

fn server_for(path: &Path) -> Option<Server> {
    match path.extension()?.to_str()? {
        "rs" => Some(Server {
            program: "rust-analyzer".into(),
            args: Vec::new(),
            language_id: "rust".into(),
        }),
        "ts" | "tsx" | "js" | "jsx" => Some(Server {
            program: "typescript-language-server".into(),
            args: vec!["--stdio".into()],
            language_id: "typescript".into(),
        }),
        _ => None,
    }
}

/// Query the configured server for `position`, returning `None` when this
/// language/build/server cannot provide a precise answer.
#[must_use]
pub fn query(root: &Path, position: &Position, query: Query) -> Option<Vec<Location>> {
    query_result(root, position, query).ok().flatten()
}

/// Diagnostic form of [`query`]. `Ok(None)` means no adapter exists for the
/// file language; process/protocol/build failures remain errors for tests and
/// operators, while normal serving uses [`query`] to degrade gracefully.
pub fn query_result(
    root: &Path,
    position: &Position,
    query: Query,
) -> anyhow::Result<Option<Vec<Location>>> {
    let file = root.join(&position.file);
    let Some(server) = server_for(&file) else {
        return Ok(None);
    };
    Client::start(root, server)
        .and_then(|mut client| client.query(&file, position, query))
        .map(Some)
}

struct Client {
    root: PathBuf,
    child: Child,
    stdin: ChildStdin,
    replies: Receiver<anyhow::Result<Value>>,
    next_id: u64,
    server: Server,
    opened: HashSet<PathBuf>,
}

impl Client {
    fn start(root: &Path, server: Server) -> anyhow::Result<Self> {
        let mut child = Command::new(&server.program)
            .args(&server.args)
            .current_dir(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow::anyhow!("language server stdin unavailable"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow::anyhow!("language server stdout unavailable"))?;
        let (tx, replies) = mpsc::channel();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                match read_message(&mut reader) {
                    Ok(Some(message)) => {
                        if tx.send(Ok(message)).is_err() {
                            break;
                        }
                    }
                    Ok(None) => break,
                    Err(error) => {
                        let _ = tx.send(Err(error));
                        break;
                    }
                }
            }
        });
        let mut client = Self {
            root: root.canonicalize().unwrap_or_else(|_| root.to_path_buf()),
            child,
            stdin,
            replies,
            next_id: 1,
            server,
            opened: HashSet::new(),
        };
        let root_uri = file_uri(&client.root);
        client.request_with_timeout(
            "initialize",
            &json!({
                "processId": std::process::id(),
                "rootUri": root_uri,
                "capabilities": {},
                "workspaceFolders": [{"uri": root_uri, "name": "yupana-query"}]
            }),
            Duration::from_secs(30),
        )?;
        client.notify("initialized", &json!({}))?;
        Ok(client)
    }

    fn query(
        &mut self,
        file: &Path,
        position: &Position,
        query: Query,
    ) -> anyhow::Result<Vec<Location>> {
        let uri = file_uri(file);
        let canonical_file = file.canonicalize().unwrap_or_else(|_| file.to_path_buf());
        if self.opened.insert(canonical_file) {
            let text = std::fs::read_to_string(file)?;
            self.notify(
                "textDocument/didOpen",
                &json!({"textDocument": {
                    "uri": uri,
                    "languageId": &self.server.language_id,
                    "version": 1,
                    "text": text
                }}),
            )?;
        }
        let method = match query {
            Query::Definition => "textDocument/definition",
            Query::References => "textDocument/references",
        };
        let mut params = json!({
            "textDocument": {"uri": uri},
            "position": {
                "line": position.line.saturating_sub(1),
                "character": position.column.saturating_sub(1)
            }
        });
        if query == Query::References {
            params["context"] = json!({"includeDeclaration": false});
        }
        // `initialized` acknowledges the protocol, not that a cold workspace
        // finished indexing. Retry a cold empty answer briefly on this SAME
        // server; a warm client answers immediately and remains under FR-2's
        // one-second query budget.
        for attempt in 0..30 {
            let response = self.request(method, &params)?;
            let found = locations(response.get("result").unwrap_or(&Value::Null), &self.root);
            if !found.is_empty() || attempt == 29 {
                return Ok(found);
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        unreachable!("bounded retry loop always returns")
    }

    fn request(&mut self, method: &str, params: &Value) -> anyhow::Result<Value> {
        self.request_with_timeout(method, params, Duration::from_secs(5))
    }

    fn request_with_timeout(
        &mut self,
        method: &str,
        params: &Value,
        timeout: Duration,
    ) -> anyhow::Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))?;
        loop {
            let message = self.replies.recv_timeout(timeout).map_err(|error| {
                anyhow::anyhow!("language server stopped answering {method}: {error}")
            })??;
            // Servers may ask for configuration/capability data while a request
            // is in flight. A client that ignores those requests can deadlock
            // initialization. We advertise no dynamic capabilities, so `null`
            // is the honest generic answer.
            if message.get("method").is_some() {
                if let Some(server_id) = message.get("id") {
                    self.send(&json!({
                        "jsonrpc": "2.0",
                        "id": server_id,
                        "result": Value::Null,
                    }))?;
                }
                continue;
            }
            if message.get("id").and_then(Value::as_u64) == Some(id) {
                if let Some(error) = message.get("error") {
                    anyhow::bail!("language server {method} error: {error}");
                }
                return Ok(message);
            }
        }
    }

    fn notify(&mut self, method: &str, params: &Value) -> anyhow::Result<()> {
        self.send(&json!({"jsonrpc": "2.0", "method": method, "params": params}))
    }

    fn send(&mut self, value: &Value) -> anyhow::Result<()> {
        let body = serde_json::to_vec(value)?;
        write!(self.stdin, "Content-Length: {}\r\n\r\n", body.len())?;
        self.stdin.write_all(&body)?;
        self.stdin.flush()?;
        Ok(())
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.send(&json!({"jsonrpc": "2.0", "method": "exit"}));
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn read_message(reader: &mut impl BufRead) -> anyhow::Result<Option<Value>> {
    let mut length = None;
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header)? == 0 {
            return Ok(None);
        }
        if header == "\r\n" || header == "\n" {
            break;
        }
        if let Some(value) = header.to_ascii_lowercase().strip_prefix("content-length:") {
            length = Some(value.trim().parse::<usize>()?);
        }
    }
    let length = length.ok_or_else(|| anyhow::anyhow!("LSP frame lacks Content-Length"))?;
    let mut body = vec![0; length];
    reader.read_exact(&mut body)?;
    Ok(Some(serde_json::from_slice(&body)?))
}

fn locations(value: &Value, root: &Path) -> Vec<Location> {
    match value {
        Value::Null => Vec::new(),
        Value::Array(items) => items.iter().filter_map(|v| location(v, root)).collect(),
        Value::Object(_) => location(value, root).into_iter().collect(),
        _ => Vec::new(),
    }
}

fn location(value: &Value, root: &Path) -> Option<Location> {
    // LocationLink uses targetUri/targetSelectionRange; Location uses uri/range.
    let uri = value
        .get("uri")
        .or_else(|| value.get("targetUri"))?
        .as_str()?;
    let range = value
        .get("range")
        .or_else(|| value.get("targetSelectionRange"))?;
    let start = range.get("start")?;
    let end = range.get("end")?;
    let path = uri_path(uri)?;
    let file = path
        .strip_prefix(root)
        .unwrap_or(&path)
        .to_string_lossy()
        .replace('\\', "/");
    Some(Location {
        file,
        start_line: usize::try_from(start.get("line")?.as_u64()?).ok()? + 1,
        start_column: usize::try_from(start.get("character")?.as_u64()?).ok()? + 1,
        end_line: usize::try_from(end.get("line")?.as_u64()?).ok()? + 1,
        end_column: usize::try_from(end.get("character")?.as_u64()?).ok()? + 1,
    })
}

fn file_uri(path: &Path) -> String {
    format!("file://{}", path.to_string_lossy().replace(' ', "%20"))
}

fn uri_path(uri: &str) -> Option<PathBuf> {
    Some(PathBuf::from(
        uri.strip_prefix("file://")?.replace("%20", " "),
    ))
}

#[cfg(test)]
#[path = "lsp_test.rs"]
mod tests;
