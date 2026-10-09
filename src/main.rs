use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use axum_master_data_api::{
    api::{build_app, AppState},
    auth::JwtKeys,
    config::AppConfig,
    repository::postgres::{connect, PgMasterDataRepository, MIGRATOR},
    service::MasterDataService,
    telemetry,
};
use secrecy::ExposeSecret;
use tokio::signal;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cfg = AppConfig::load().context("loading configuration")?;
    telemetry::init(cfg.log.format);

    let pool = connect(
        cfg.database.url.expose_secret(),
        cfg.database.max_connections,
        Duration::from_secs(cfg.database.acquire_timeout_secs),
    )
    .await
    .context("connecting to database")?;
    MIGRATOR.run(&pool).await.context("running migrations")?;

    let state = AppState {
        service: Arc::new(MasterDataService::new(Arc::new(PgMasterDataRepository::new(
            pool,
        )))),
        jwt: JwtKeys::new(cfg.auth.jwt_secret.expose_secret(), &cfg.auth.write_scope),
    };
    let app = build_app(state, Duration::from_secs(cfg.server.request_timeout_secs));

    let addr = format!("{}:{}", cfg.server.host, cfg.server.port);
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .with_context(|| format!("binding {addr}"))?;
    tracing::info!(%addr, "listening");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("server error")?;
    tracing::info!("shut down cleanly");
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async { signal::ctrl_c().await.expect("ctrl_c handler") };
    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("SIGTERM handler")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
    tracing::info!("shutdown signal received");
}
