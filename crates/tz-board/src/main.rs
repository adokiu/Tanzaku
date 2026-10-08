use anyhow::Context;
use std::{net::SocketAddr, sync::Arc};
use tokio::{net::TcpListener, sync::watch};
use tracing_subscriber::EnvFilter;
use tz_board::{config, setup};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tz_pki::init_rustls_crypto_provider();
    init_tracing();
    let path = config::resolve_config_path()?;
    tracing::info!(path = %path.display(), "using board configuration");
    let config = config::BoardConfig::load(&path)?;
    let mode = setup::RuntimeMode::start(config.clone()).await?;
    let state = Arc::new(setup::AppState::new(mode, path, config.listen.clone()));
    let _redis_worker = setup::start_redis_worker(state.clone());
    tz_board::start_background_tasks(state.clone());
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let _signal_task = tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            let _ = shutdown_tx.send(true);
        }
    });
    let listeners = config::Listeners::bind(&config.listen).await?;
    tracing::info!("board listeners ready");
    tokio::try_join!(
        run_listener(
            listeners.admin,
            setup::admin_router(state.clone()),
            shutdown_rx.clone()
        ),
        run_listener(
            listeners.user,
            setup::user_router(state.clone()),
            shutdown_rx.clone()
        ),
        run_listener(
            listeners.node,
            setup::node_control_router(state.clone()),
            shutdown_rx.clone()
        ),
        run_listener(
            listeners.agent,
            setup::agent_control_router(state),
            shutdown_rx
        ),
    )
    .context("board listener stopped unexpectedly")?;
    Ok(())
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(filter)
        .init();
}

async fn run_listener(
    listener: TcpListener,
    router: axum::Router,
    mut shutdown: watch::Receiver<bool>,
) -> anyhow::Result<()> {
    axum::serve(
        listener,
        router.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async move {
        if !*shutdown.borrow() {
            let _ = shutdown.changed().await;
        }
    })
    .await?;
    Ok(())
}
