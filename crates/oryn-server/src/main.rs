//! `oryn-server` — run the Oryn HTTP API.
//!
//! Bind address comes from `--addr` / `$ORYN_ADDR` (default `127.0.0.1:8787`).

use std::net::SocketAddr;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "oryn_server=info,tower_http=info".into()),
        )
        .init();

    let addr: SocketAddr = std::env::args()
        .skip_while(|a| a != "--addr")
        .nth(1)
        .or_else(|| std::env::var("ORYN_ADDR").ok())
        .unwrap_or_else(|| "127.0.0.1:8787".to_string())
        .parse()?;

    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(
        "oryn-server listening on http://{addr}  (backend: {})",
        oryn_cuda::backend()
    );
    oryn_server::serve(listener).await?;
    Ok(())
}
