use tracing_subscriber::EnvFilter;
use tz_client::AgentRuntime;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tz_pki::init_rustls_crypto_provider();
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();
    let connect = tz_agent::load_connect_config("client")?;
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
        "tanzaku-client starting"
    );
    let runtime = AgentRuntime::bootstrap(board_ws, connect.token);
    runtime.run().await;
}
