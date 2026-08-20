//! Body of `yupana_path_check`, feature-split on `golden-path`.
//!
//! The same shape the board tools use: the tool is always registered, and
//! without the engine the body is an honest refusal naming the feature. A
//! check that accepted a plan and silently did nothing would be read as an
//! approval — the exact failure the golden-path honesty rules forbid.

use super::*;

/// Body of `yupana_path_check`.
pub(super) fn path_check(
    _server: &YupanaMcpServer,
    req: &PathCheckRequest,
) -> Result<CallToolResult, McpError> {
    #[cfg(not(feature = "golden-path"))]
    {
        let _ = req;
        Err(internal(crate::errors::Error::Config(
            "yupana_path_check needs the `golden-path` feature; this server was built without \
             it. This plan was NOT checked — do not read this error as conformance."
                .to_string(),
        )))
    }
    #[cfg(feature = "golden-path")]
    {
        match crate::goldenpath::check(
            &req.paths,
            &req.follows_path,
            &req.steps,
            req.mode.unwrap_or_default(),
            req.deny.unwrap_or(false),
        ) {
            // An ERROR, not a result with an empty findings list: a tool
            // result is something a model reads as an answer, and a refusal
            // must not be one of those.
            crate::goldenpath::CheckOutcome::Refused { reason } => {
                Err(internal(crate::errors::Error::Config(reason)))
            }
            crate::goldenpath::CheckOutcome::Evaluated(report) => json_result(&report),
        }
    }
}
