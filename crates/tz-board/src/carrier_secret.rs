//! TCP carrier 隧道级共享密钥：仅存 Board 内存，经已认证的控制通道下发给 node 与 client。
//! 创建/修改/恢复/client 重连时轮换为最新密钥：先推 node，再推 client（client 主动连 server）。
//! Board 重启后随 node/client 重连由 `for_carrier` 惰性重新生成。

use dashmap::DashMap;
use std::sync::LazyLock;
use uuid::Uuid;

static SECRETS: LazyLock<DashMap<Uuid, String>> = LazyLock::new(DashMap::new);

fn generate() -> String {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes).expect("os random source unavailable");
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// QUIC 自带 TLS，不需要额外密钥。
pub fn for_carrier(tunnel_id: Uuid, carrier: &str) -> String {
    if carrier != "tcp" {
        return String::new();
    }
    SECRETS.entry(tunnel_id).or_insert_with(generate).clone()
}

pub fn rotate(tunnel_id: Uuid) {
    SECRETS.insert(tunnel_id, generate());
}

pub fn forget(tunnel_id: Uuid) {
    SECRETS.remove(&tunnel_id);
}
