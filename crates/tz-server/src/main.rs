use anyhow::Context;
use tracing_subscriber::EnvFilter;
use tz_server::NodeRuntime;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tz_pki::init_rustls_crypto_provider();
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();
    let mut args = std::env::args().skip(1);
    let mut board = None;
    let mut token = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--board" => board = args.next(),
            "--token" => token = args.next(),
            _ => {}
        }
    }
    let board = board.context("usage: tanzaku-server --board ws://127.0.0.1:9002/ws --token <node-token>")?;
    let token = token.context("missing --token")?;
    let board_ws = tz_agent::normalize_board_url(&board);
    if board_ws != board.trim() {
        tracing::info!(raw = %board, normalized = %board_ws, "board URL normalized (appended /ws)");
    }
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        board = %board_ws,
        "tanzaku-server starting"
    );
    let runtime = NodeRuntime::bootstrap(board_ws, token);
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
