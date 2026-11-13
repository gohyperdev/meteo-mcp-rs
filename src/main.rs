use anyhow::Result;
use meteo_mcp_rs::server::MeteoServer;
use rmcp::{ServiceExt, transport::stdio};
use tracing_subscriber::{EnvFilter, fmt};

#[tokio::main]
async fn main() -> Result<()> {
    // Log exclusively to stderr so stdout remains clean for JSON-RPC messages.
    fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();

    let service = MeteoServer::new().serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}
