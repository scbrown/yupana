//! What each MCP tool does to its environment, declared as MCP annotations, and
//! the read-only toolset `yupana serve --read-only` exposes (aegis-q2rn4u).
//!
//! Without annotations an MCP client has to treat every tool as destructive:
//! codex refuses all of them under a never-approve policy, and a crew manifest
//! cannot offer `yupana_impact` without also offering `yupana_promote`, which
//! writes into Quipu. Every registered tool is classified here; an unclassified
//! tool fails the test below, so a new tool cannot ship without a declaration.

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::model::ToolAnnotations;

/// The tools `serve --read-only` exposes: code-structure QUERIES only. Agents
/// get the blast-radius question ("what does this change affect?") without any
/// write path, and the tool list stays small for the context budget.
pub const READ_ONLY_TOOLS: &[&str] = &[
    "yupana_impact",
    "yupana_callers",
    "yupana_references",
    "yupana_dataflow",
    "yupana_symbols",
    "yupana_analyze",
    "yupana_status",
];

/// How a tool touches its environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    /// Reads the tree, config or an in-process board; changes nothing.
    Read,
    /// Writes only this process's in-memory board (additive, not persisted).
    ProcessWrite,
    /// Writes an external store (Quipu).
    ExternalWrite,
}

/// The declared effect of every tool `yupana serve` registers, or `None` for a
/// tool nobody has classified.
#[must_use]
pub fn effect(tool: &str) -> Option<Effect> {
    Some(match tool {
        "yupana_status" | "yupana_symbols" | "yupana_references" | "yupana_analyze"
        | "yupana_callers" | "yupana_callees" | "yupana_impact" | "yupana_dataflow"
        | "yupana_communities" | "yupana_verify" | "yupana_path_check" => Effect::Read,
        // Copy-on-write overlays over the board: they take its READ lock only.
        "yupana_guard" | "yupana_whatif" => Effect::Read,
        "yupana_ingest" => Effect::ProcessWrite,
        "yupana_promote" => Effect::ExternalWrite,
        _ => return None,
    })
}

fn annotations(effect: Effect) -> ToolAnnotations {
    match effect {
        Effect::Read => ToolAnnotations::new().read_only(true).open_world(false),
        Effect::ProcessWrite => ToolAnnotations::new()
            .read_only(false)
            .destructive(false)
            .open_world(false),
        Effect::ExternalWrite => ToolAnnotations::new()
            .read_only(false)
            .destructive(true)
            .open_world(true),
    }
}

/// Attach each tool's declared annotations. An unclassified tool is left
/// without annotations, which clients read as "may be destructive".
pub fn annotate<S: Send + Sync + 'static>(router: &mut ToolRouter<S>) {
    for (name, route) in &mut router.map {
        if let Some(effect) = effect(name) {
            route.attr.annotations = Some(annotations(effect));
        }
    }
}

/// `router` with each tool's declared annotations attached.
#[must_use]
pub fn annotated<S: Send + Sync + 'static>(mut router: ToolRouter<S>) -> ToolRouter<S> {
    annotate(&mut router);
    router
}

impl super::server::YupanaMcpServer {
    /// Serve only the code-structure query tools (`serve --read-only`).
    #[must_use]
    pub fn read_only(mut self) -> Self {
        restrict_to_read_only(&mut self.tool_router);
        self
    }

    /// The tools this server lists, with their annotations.
    #[cfg(test)]
    #[must_use]
    pub fn tools(&self) -> Vec<rmcp::model::Tool> {
        self.tool_router.list_all()
    }
}

/// Keep only [`READ_ONLY_TOOLS`]: anything else is not even listed.
pub fn restrict_to_read_only<S: Send + Sync + 'static>(router: &mut ToolRouter<S>) {
    router
        .map
        .retain(|name, _| READ_ONLY_TOOLS.contains(&name.as_ref()));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::server::YupanaMcpServer;

    fn names(server: &YupanaMcpServer) -> Vec<String> {
        let mut out: Vec<String> = server
            .tools()
            .into_iter()
            .map(|t| t.name.to_string())
            .collect();
        out.sort();
        out
    }

    #[test]
    fn every_registered_tool_is_classified_and_annotated() {
        let server = YupanaMcpServer::new(".".into(), None, None);
        for tool in server.tools() {
            let effect = effect(&tool.name)
                .unwrap_or_else(|| panic!("{} has no declared effect in tool_policy", tool.name));
            let a = tool.annotations.expect("annotated");
            assert_eq!(
                a.read_only_hint,
                Some(effect == Effect::Read),
                "{}",
                tool.name
            );
        }
    }

    #[test]
    fn the_quipu_write_is_declared_destructive() {
        let server = YupanaMcpServer::new(".".into(), None, None);
        let promote = server
            .tools()
            .into_iter()
            .find(|t| t.name == "yupana_promote")
            .unwrap();
        let a = promote.annotations.unwrap();
        assert_eq!(
            (a.read_only_hint, a.destructive_hint),
            (Some(false), Some(true))
        );
    }

    #[test]
    fn read_only_mode_lists_exactly_the_query_tools_and_no_write() {
        let server = YupanaMcpServer::new(".".into(), None, None).read_only();
        let mut want: Vec<String> = READ_ONLY_TOOLS.iter().map(|s| (*s).to_string()).collect();
        want.sort();
        assert_eq!(names(&server), want);
        for tool in server.tools() {
            assert_eq!(
                effect(&tool.name),
                Some(Effect::Read),
                "{} in read-only mode",
                tool.name
            );
            assert_eq!(tool.annotations.unwrap().read_only_hint, Some(true));
        }
    }
}
