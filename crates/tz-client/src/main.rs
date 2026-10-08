use anyhow::Context;
use tracing_subscriber::EnvFilter;
use tz_client::AgentRuntime;

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
    let board = board.context("usage: tanzaku-client --board ws://127.0.0.1:9003/ws --token <client-token>")?;
    let token = token.context("missing --token")?;
    let board_ws = tz_agent::normalize_board_url(&board);
    if board_ws != board.trim() {
        tracing::info!(raw = %board, normalized = %board_ws, "board URL normalized (appended /ws)");
    }
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        board = %board_ws,
        "tanzaku-client starting"
    );
    let runtime = AgentRuntime::bootstrap(board_ws, token);
    runtime.run().await;
}
