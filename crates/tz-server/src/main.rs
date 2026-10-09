use tracing_subscriber::EnvFilter;
use tz_server::NodeRuntime;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tz_pki::init_rustls_crypto_provider();
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();
    let connect = tz_agent::load_connect_config("server")?;
    let board_ws = tz_agent::normalize_board_url(&connect.board);
    if board_ws != connect.board.trim() {
        tracing::info!(
            raw = %connect.board,
            normalized = %board_ws,
            "board URL normalized (appended /ws)"
        );
    }
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        board = %board_ws,
        "tanzaku-server starting"
    );
    let runtime = NodeRuntime::bootstrap(board_ws, connect.token);
    for carrier in tz_carrier::registered_kinds() {
        let worker = runtime.clone();
        let carrier = carrier.to_string();
        tokio::spawn(async move {
            loop {
                let Some(listen) = worker.carrier_listen_config(&carrier) else {
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                    continue;
                };
                if let Err(error) = worker.serve_carrier(&carrier, listen).await {
                    tracing::warn!(carrier = %carrier, ?error, "carrier listener exited, retrying");
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                }
            }
        });
    }
    tokio::select! {
        () = runtime.run() => {}
        _ = tokio::signal::ctrl_c() => {
            tracing::info!("shutdown signal received");
        }
    }
    tz_server::firewall::teardown();
    Ok(())
}
