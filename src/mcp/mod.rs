//! MCP (Model Context Protocol) server for Yupana.
//!
//! Exposes Yupana's live structural analysis to agents over both stdio and
//! streamable-HTTP, using `rmcp` — the same SDK and registration pattern Bobbin
//! uses. Tools follow the `yupana_*` naming convention (see `docs/yupana-spec.md`
//! §10). This module is gated behind the `mcp` feature.

/// Request DTO for the golden-path check (FR-41/FR-42), defined for both arms
/// of the `golden-path` feature — the tool method is registered on every build.
mod goldenpath_tools;
mod resident;
mod server;
/// Request DTOs for the board tools (FR-35/37/38), defined for both arms of the
/// `game-state` feature — the tool methods are registered on every build.
mod state_tools;
mod tools;
mod transport;

pub use transport::{run_http, run_stdio};
