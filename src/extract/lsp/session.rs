//! An explicitly owned warm language-server session. Drop it to stop the server.

use super::{file_uri, Client, Location, Path, Position, Query};
use serde_json::{json, Value};

/// A server-produced fact, including the precision tier on the wire.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Precise<T> {
    /// Always LSP; no syntax fallback is passed through this envelope.
    pub tier: crate::types::Tier,
    /// The server's answer, normalized for location queries.
    pub value: T,
}

impl<T> Precise<T> {
    fn new(value: T) -> Self {
        Self {
            tier: crate::types::Tier::Lsp,
            value,
        }
    }
}

/// Reuse one server for on-demand or on-save queries in one workspace/language.
///
/// Unlike the one-shot CLI helper, this owner keeps the initialized process warm.
/// Files are read on demand; changed contents produce a full `didChange` before
/// the next request. No background watcher or per-keystroke work is installed.
/// Missing servers return an error and unsupported languages return `None`;
/// callers can explicitly retain their tree-sitter fallback in either case.
pub struct Session {
    client: Client,
}

impl Session {
    /// Start the language adapter selected by the file extension.
    pub fn start(root: &Path, file: &Path) -> anyhow::Result<Option<Self>> {
        super::server_for(file)
            .map(|server| Client::start(root, server).map(|client| Self { client }))
            .transpose()
    }

    /// Definitions, references, or type definitions at a one-based UTF-16 position.
    pub fn locations(
        &mut self,
        position: &Position,
        query: Query,
    ) -> anyhow::Result<Precise<Vec<Location>>> {
        let file = self.file(&position.file)?;
        self.client.query(&file, position, query).map(Precise::new)
    }

    /// Preserve the protocol's `Hover` contents and optional source range.
    pub fn hover(&mut self, position: &Position) -> anyhow::Result<Precise<Value>> {
        let file = self.file(&position.file)?;
        self.client.open(&file)?;
        self.result("textDocument/hover", &json!({
            "textDocument": {"uri": file_uri(&file)},
            "position": {"line": position.line.saturating_sub(1), "character": position.column.saturating_sub(1)}
        }))
    }

    /// Preserve flat or hierarchical document symbols without guessing their kind.
    pub fn document_symbols(&mut self, file: &str) -> anyhow::Result<Precise<Value>> {
        let file = self.file(file)?;
        self.client.open(&file)?;
        self.result(
            "textDocument/documentSymbol",
            &json!({"textDocument": {"uri": file_uri(&file)}}),
        )
    }

    /// Search symbols in this session's workspace through the same server.
    pub fn workspace_symbols(&mut self, query: &str) -> anyhow::Result<Precise<Value>> {
        self.result("workspace/symbol", &json!({"query": query}))
    }

    fn file(&self, relative: &str) -> anyhow::Result<std::path::PathBuf> {
        let file = self.client.root.join(relative).canonicalize()?;
        anyhow::ensure!(
            file.starts_with(&self.client.root),
            "file is outside this LSP workspace"
        );
        let server =
            super::server_for(&file).ok_or_else(|| anyhow::anyhow!("unsupported LSP language"))?;
        anyhow::ensure!(
            server.program == self.client.server.program,
            "file requires another language-server session"
        );
        Ok(file)
    }

    fn result(&mut self, method: &str, params: &Value) -> anyhow::Result<Precise<Value>> {
        let response = self.client.request(method, params)?;
        let result = response
            .get("result")
            .ok_or_else(|| anyhow::anyhow!("LSP response lacks result"))?;
        Ok(Precise::new(result.clone()))
    }
}
