//! Bodies for the two heaviest `yupana_*` tool handlers, lifted out of `server`
//! for size (yupana #83).
//!
//! `#[tool_router]` collects every `#[tool]` method from ONE impl block, so the
//! handlers themselves cannot move — only their bodies. Each is a plain function
//! taking the server, and `server.rs` keeps a thin wrapper carrying the tool
//! attribute and its description. Neither body awaits anything, so neither needs
//! to be async.
//!
//! A child module of `server`, so `internal`, `json_result` and `graph_tier`
//! reach it through `use super::*` and lifting these out changed no visibility.

use super::*;

/// Body of `yupana_promote`.
pub(super) fn promote(
    server: &YupanaMcpServer,
    req: &PromoteRequest,
) -> Result<CallToolResult, McpError> {
    #[cfg(not(feature = "quipu"))]
    {
        // Both arguments go unread on the refusal path; the method took `self`
        // implicitly before the body moved here, so `server` joins `req` in the
        // existing discard rather than becoming an `_server` in the signature.
        let _ = (req, server);
        Err(internal(crate::errors::Error::Config(
            "yupana_promote needs the `quipu` feature; this server was built without it"
                .to_string(),
        )))
    }
    #[cfg(feature = "quipu")]
    {
        let config =
            YupanaConfig::resolve(server.config.as_deref(), &server.root).map_err(internal)?;
        // Promotion is a WRITE — honour the same guard the CLI does. Refused
        // before any work, so read_only means read_only even here.
        config.write_guard("promotion").map_err(internal)?;

        let endpoint = req
            .endpoint
            .clone()
            .filter(|e| !e.is_empty())
            .or_else(|| Some(config.quipu.endpoint.clone()).filter(|e| !e.is_empty()))
            .ok_or_else(|| {
                internal(crate::errors::Error::Promote(
                    "no Quipu endpoint: set [yupana.quipu] endpoint or pass one in the \
                     request. Refusing rather than guessing a graph."
                        .to_string(),
                ))
            })?;

        let base = req
            .path
            .as_ref()
            .map_or_else(|| server.root.clone(), |p| server.root.join(p));
        // Repo identity is a segment of every promoted IRI. Request value wins;
        // otherwise the origin remote names the repository. With neither,
        // REFUSE — the old dir-basename fallback minted `code/<worktree-dir>/…`
        // islands (an agent worktree promoted an entire graph as `code/gennaro`).
        let repo = match req.repo.as_deref() {
            Some(r) => r.to_string(),
            None => crate::git::origin_repo_name(&base).ok_or_else(|| {
                internal(crate::errors::Error::Promote(format!(
                    "cannot determine repository identity: no `origin` remote at {}. \
                     Pass `repo` in the request. Refusing rather than deriving \
                     identity from the directory name, which fragments the graph.",
                    base.display()
                )))
            })?,
        };
        let turtle = crate::export::to_turtle(&base, &repo).map_err(internal)?;

        let source = format!("yupana promote {repo} (mcp)");
        let response =
            match crate::promote::promote(&endpoint, &turtle, &source).map_err(internal)? {
                crate::promote::Promotion::Wrote(k) => PromoteResponse {
                    wrote: true,
                    count: Some(k.count),
                    tx_id: k.tx_ids.last().copied(),
                    chunks: Some(k.chunks),
                    violations: Vec::new(),
                    tier: crate::types::Tier::TreeSitter.as_str().to_string(),
                },
                crate::promote::Promotion::Refused {
                    mut violations,
                    payload,
                } => {
                    // An MCP client sees only this response, so the retained
                    // payload has to ride in it or the path is lost to the
                    // caller who most needs it.
                    if let Some(p) = payload {
                        violations.push(format!("payload retained at: {}", p.display()));
                    }
                    PromoteResponse {
                        wrote: false,
                        count: None,
                        tx_id: None,
                        chunks: None,
                        violations,
                        tier: crate::types::Tier::TreeSitter.as_str().to_string(),
                    }
                }
                // `promote` is the WRITING spelling — the dry run is a separate
                // entry point, and `promote_never_reports_a_dry_run` in
                // promote_test.rs asserts this variant cannot come back from it.
                // So this arm is unreachable, and it is written as a REFUSAL
                // rather than a panic or a `wrote: true`: an MCP client sees
                // only this response, and if the invariant ever breaks, the one
                // unacceptable outcome is reporting a write that did not happen.
                // Fail closed and name the invariant.
                crate::promote::Promotion::Conforms { chunks, bytes, .. } => PromoteResponse {
                    wrote: false,
                    count: None,
                    tx_id: None,
                    chunks: Some(chunks),
                    tier: crate::types::Tier::TreeSitter.as_str().to_string(),
                    violations: vec![format!(
                        "internal invariant broken: `promote` returned a dry-run result \
                         ({chunks} chunks, {bytes} bytes) instead of writing. NOTHING WAS \
                         WRITTEN. This is a bug in yupana, not in the projection — please report it."
                    )],
                },
            };
        json_result(&response)
    }
}

/// Body of `yupana_dataflow`.
pub(super) fn dataflow(
    server: &YupanaMcpServer,
    req: &DataflowRequest,
) -> Result<CallToolResult, McpError> {
    let base = req
        .path
        .as_ref()
        .map_or_else(|| server.root.clone(), |p| server.root.join(p));
    let flow = Dataflow::build(&base).map_err(internal)?;
    let found = flow.has_function(&req.function);

    let (direction, steps, edges) = match &req.var {
        Some(var) => {
            let dir = if req.forward.unwrap_or(false) {
                FlowDir::FlowsInto
            } else {
                FlowDir::DependsOn
            };
            let steps = flow
                .flow(&req.function, var, dir, req.hops.unwrap_or(5))
                .into_iter()
                .map(|s| FlowStepItem {
                    name: s.name,
                    distance: s.distance,
                })
                .collect();
            (Some(dir.as_str().to_string()), steps, Vec::new())
        }
        None => {
            let edges = flow
                .edges(&req.function)
                .iter()
                .map(|e| DepEdgeItem {
                    dependent: e.dependent.clone(),
                    depends_on: e.depends_on.clone(),
                    line: e.line,
                })
                .collect();
            (None, Vec::new(), edges)
        }
    };

    let response = DataflowResponse {
        function: req.function.clone(),
        found,
        direction,
        var: req.var.clone(),
        flow: steps,
        edges,
        tier: graph_tier(),
    };
    json_result(&response)
}

/// Body of `yupana_references`.
pub(super) fn references(
    server: &YupanaMcpServer,
    req: &ReferencesRequest,
) -> Result<CallToolResult, McpError> {
    // A position-based request (FR-4, yupana #8) needs the node SPANS, which
    // the daemon's `/references` reply does not carry — so it takes the
    // transient path, like a `path`-scoped one. Slower, and correct;
    // answering it from a name lookup would hand back every same-named
    // symbol, which is the ambiguity the position was given to resolve.
    let by_position = req.at_file.is_some() || req.at_line.is_some();
    if req.path.is_none() && !by_position {
        if let Some(symbol) = req.symbol.as_deref() {
            if let Some(response) =
                crate::mcp::resident::references(server.config.as_deref(), &server.root, symbol)
            {
                return json_result(&response);
            }
        }
    }
    let base = req
        .path
        .as_ref()
        .map_or_else(|| server.root.clone(), |p| server.root.join(p));
    // Build the multi-language graph, exactly as the resident path above
    // resolves against one. This walked `rust_files` and parsed every hit as
    // `"rust"`, so a `path`-scoped request (which always lands here, daemon
    // or not) over a Python tree searched ZERO files and answered "no
    // definitions" — the CLI-side yupana #76 bug, same cause, second surface.
    let graph = match CodeGraph::build(&base) {
        Ok(graph) => graph,
        Err(e) => return Err(McpError::internal_error(e.to_string(), None)),
    };
    let to_item = |symbol: &crate::graph::SymbolNode| RefItem {
        file: symbol.file.clone(),
        name: symbol.name.clone(),
        kind: symbol.kind.clone(),
        start_line: symbol.start_line,
        tier: symbol.tier.as_str().to_string(),
    };
    let (queried, definitions): (String, Vec<RefItem>) =
        match (&req.symbol, &req.at_file, req.at_line) {
            // Position wins when given: it is the more specific request.
            (_, Some(file), Some(line)) => {
                let hit = graph.symbol_at(file, line);
                (
                    hit.map_or_else(|| format!("{file}:{line}"), |n| n.name.clone()),
                    hit.into_iter().map(to_item).collect(),
                )
            }
            // Half a position is not a position. Refuse rather than quietly
            // dropping to a name lookup the caller did not ask for — a silent
            // downgrade would answer a "which one is here" question with "all
            // of them", the exact over-connection this parameter exists to cut.
            (_, Some(_), None) | (_, None, Some(_)) => {
                return Err(McpError::invalid_params(
                    "at_file and at_line go together: give both for a position-based lookup, \
                     or neither and use `symbol`."
                        .to_string(),
                    None,
                ));
            }
            (Some(name), None, None) => (
                name.clone(),
                graph.definitions(name).into_iter().map(to_item).collect(),
            ),
            (None, None, None) => {
                return Err(McpError::invalid_params(
                    "give `symbol`, or `at_file` + `at_line` to resolve by position.".to_string(),
                    None,
                ));
            }
        };
    let response = ReferencesResponse {
        symbol: queried,
        count: definitions.len(),
        definitions,
        searched_symbols: Some(graph.stats().0),
        tier: crate::types::Tier::TreeSitter.as_str().to_string(),
    };
    json_result(&response)
}
