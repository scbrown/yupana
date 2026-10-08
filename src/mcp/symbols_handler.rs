//! File symbol enumeration over the callers/impact graph, not a Rust-only parser.
use super::*;

fn no_symbols(file: &str) -> McpError {
    McpError::invalid_params(
        format!("code graph holds no symbols for {file:?}; the file may be unindexed, unsupported, or have no definitions"),
        None,
    )
}

pub(super) fn symbols(
    server: &YupanaMcpServer,
    req: &SymbolsRequest,
) -> Result<CallToolResult, McpError> {
    let root = server.root.canonicalize().map_err(internal)?;
    let file = server
        .root
        .join(&req.file)
        .canonicalize()
        .map_err(internal)?;
    let rel = file
        .strip_prefix(&root)
        .map_err(|_| McpError::invalid_params("file must be within the analysis root", None))?;
    let rel = rel
        .to_str()
        .ok_or_else(|| McpError::invalid_params("file path must be UTF-8", None))?;
    let symbols = if let Some(reply) =
        super::super::resident::file_symbols(server.config.as_deref(), &server.root, rel)
    {
        if !reply.known || reply.symbols.is_empty() {
            return Err(no_symbols(rel));
        }
        reply
            .symbols
            .into_iter()
            .map(|symbol| SymbolItem {
                name: symbol.name,
                kind: symbol.kind,
                start_line: symbol.start_line,
                // The daemon protocol currently has no end line. Never invent it
                // or re-parse today's file for a different snapshot's extent.
                end_line: None,
                tier: reply.tier.clone(),
            })
            .collect::<Vec<_>>()
    } else {
        let graph = CodeGraph::build(&server.root).map_err(internal)?;
        let found = graph.file_symbols(rel);
        if found.is_empty() {
            return Err(no_symbols(rel));
        }
        found
            .into_iter()
            .map(|symbol| SymbolItem {
                name: symbol.name.clone(),
                kind: symbol.kind.clone(),
                start_line: symbol.start_line,
                end_line: Some(symbol.end_line),
                tier: symbol.tier.as_str().to_owned(),
            })
            .collect()
    };
    json_result(&SymbolsResponse {
        file: req.file.clone(),
        count: symbols.len(),
        symbols,
    })
}
