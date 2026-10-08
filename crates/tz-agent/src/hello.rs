use tz_proto::{AgentCapabilities, HelloMessage, RunningTunnel};

use crate::os_name;

/// 构建 Client/Node 握手信息（`os` 与 komari-agent `monitoring.OSName()` 一致）。
pub fn build_hello_message(
    revision: i64,
    capabilities: AgentCapabilities,
    running_tunnels: Vec<RunningTunnel>,
) -> HelloMessage {
    HelloMessage {
        revision,
        capabilities,
        version: Some(env!("CARGO_PKG_VERSION").into()),
        os: Some(os_name::os_name()),
        arch: Some(std::env::consts::ARCH.into()),
        public_ipv4: None,
        public_ipv6: None,
        running_tunnels,
    }
}
