//! MCP transports: stdio (default agent transport) and streamable-HTTP
//! (network-accessible, for the broker and remote agents).

use std::path::PathBuf;

use anyhow::Result;

use super::server::YupanaMcpServer;

/// Serve over stdio (the default agent transport).
pub async fn run_stdio(
    root: PathBuf,
    tenant: Option<String>,
    config: Option<PathBuf>,
) -> Result<()> {
    use rmcp::transport::stdio;
    use rmcp::ServiceExt;

    let server = YupanaMcpServer::new(root, tenant, config);
    let service = server.serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}

/// Serve over streamable-HTTP.
pub async fn run_http(
    root: PathBuf,
    tenant: Option<String>,
    config: Option<PathBuf>,
    bind: String,
    port: u16,
) -> Result<()> {
    use std::sync::Arc;
    use std::time::Duration;

    use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
    use rmcp::transport::streamable_http_server::tower::StreamableHttpService;
    use rmcp::transport::StreamableHttpServerConfig;
    use tokio_util::sync::CancellationToken;

    let ct = CancellationToken::new();
    let service: StreamableHttpService<YupanaMcpServer, LocalSessionManager> =
        StreamableHttpService::new(
            move || {
                Ok::<_, std::io::Error>(YupanaMcpServer::new(
                    root.clone(),
                    tenant.clone(),
                    config.clone(),
                ))
            },
            Arc::new(LocalSessionManager::default()),
            StreamableHttpServerConfig {
                stateful_mode: true,
                sse_keep_alive: Some(Duration::from_secs(15)),
                cancellation_token: ct.child_token(),
            },
        );

    let router = axum::Router::new().nest_service("/mcp", service);
    let addr = format!("{bind}:{port}");
    eprintln!("Yupana MCP server listening on http://{addr}/mcp");

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, router)
        .with_graceful_shutdown(async move {
            tokio::signal::ctrl_c().await.ok();
            ct.cancel();
        })
        .await?;
    Ok(())
}
