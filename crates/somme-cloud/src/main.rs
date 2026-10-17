use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Context;
use somme_cloud::config::Config;
use somme_cloud::http::router;
use somme_cloud::platform::TursoPlatform;
use somme_cloud::service::AccountService;
use somme_cloud::store::AccountStore;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _ = dotenvy::dotenv();
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();
    let config = Config::from_env()?;
    let store = AccountStore::open(&config.data_dir).await?;
    let platform = TursoPlatform::new(&config)?;
    let service = Arc::new(AccountService::new(store, Arc::new(platform), config.token_ttl_days));
    let listener = tokio::net::TcpListener::bind(config.addr)
        .await
        .with_context(|| format!("cannot listen on {}", config.addr))?;
    tracing::info!(addr = %config.addr, "somme cloud listening");
    axum::serve(
        listener,
        router(service, config.client_ip_header.clone()).into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async {
        let _ = tokio::signal::ctrl_c().await;
    })
    .await?;
    Ok(())
}
