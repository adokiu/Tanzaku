use std::net::{IpAddr, SocketAddr};
use thiserror::Error;
use uuid::Uuid;

pub use crate::carrier_stream::CarrierStream;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FlowId(pub u64);

pub fn flow_id_from_peer(peer: SocketAddr) -> FlowId {
    match peer.ip() {
        IpAddr::V4(v4) => {
            let ip = u32::from(v4) as u64;
            FlowId((ip << 16) | u64::from(peer.port()))
        }
        IpAddr::V6(v6) => {
            let octets = v6.octets();
            let hi = u64::from_be_bytes(octets[0..8].try_into().expect("ipv6 octets"));
            let lo = u64::from_be_bytes(octets[8..16].try_into().expect("ipv6 octets"));
            FlowId(hi ^ lo ^ u64::from(peer.port()))
        }
    }
}

#[derive(Debug, Clone)]
pub struct StreamHeader {
    pub tunnel_id: Uuid,
    pub src_addr: SocketAddr,
}

#[derive(Debug, Clone)]
pub struct PeerIdentity {
    pub client_id: Option<Uuid>,
    pub tunnel_id: Option<Uuid>,
}

/// 建连前导：magic + tunnel_id + client_id。
pub const CARRIER_BIND_MAGIC: &[u8; 4] = b"TZB1";
pub const CARRIER_BIND_LEN: usize = 4 + 16 + 16;

pub fn encode_carrier_bind(tunnel_id: Uuid, client_id: Uuid) -> [u8; CARRIER_BIND_LEN] {
    let mut out = [0_u8; CARRIER_BIND_LEN];
    out[..4].copy_from_slice(CARRIER_BIND_MAGIC);
    out[4..20].copy_from_slice(tunnel_id.as_bytes());
    out[20..].copy_from_slice(client_id.as_bytes());
    out
}

pub fn decode_carrier_bind(buf: &[u8; CARRIER_BIND_LEN]) -> Option<(Uuid, Uuid)> {
    if &buf[..4] != CARRIER_BIND_MAGIC {
        return None;
    }
    let tunnel_id = Uuid::from_slice(&buf[4..20]).ok()?;
    let client_id = Uuid::from_slice(&buf[20..]).ok()?;
    Some((tunnel_id, client_id))
}

pub const CARRIER_BIND_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

pub const BIND_ACK_OK: u8 = 0;
pub const BIND_ACK_DENIED: u8 = 1;
/// 中国大陆节点按对端 IP 地理拒绝（residency）。
pub const BIND_ACK_RESIDENCY: u8 = 2;

/// 每条 TCP 链路双方各出的随机数，参与派生链路密钥，避免同一密钥下 nonce 复用。
pub const LINK_NONCE_LEN: usize = 16;
pub type LinkNonce = [u8; LINK_NONCE_LEN];

#[cfg(feature = "tcp")]
pub fn random_link_nonce() -> std::io::Result<LinkNonce> {
    let mut nonce = [0_u8; LINK_NONCE_LEN];
    getrandom::fill(&mut nonce).map_err(|err| std::io::Error::other(err.to_string()))?;
    Ok(nonce)
}

/// 加密后首帧，用于确认双方持有同一 carrier_secret。
pub const LINK_CONFIRM: &[u8; 4] = b"TZK1";

pub async fn read_carrier_bind_ack(stream: &mut tokio::net::TcpStream) -> std::io::Result<LinkNonce> {
    let mut ack = [0_u8; 1];
    tokio::io::AsyncReadExt::read_exact(stream, &mut ack)
        .await
        .map_err(|err| {
            if err.kind() == std::io::ErrorKind::UnexpectedEof {
                std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "node closed carrier link before bind ack (check node log: tcp carrier link setup failed)",
                )
            } else {
                err
            }
        })?;
    match ack[0] {
        BIND_ACK_OK => {}
        BIND_ACK_RESIDENCY => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "carrier bind rejected by cn residency (peer IP geo not CN or lookup failed)",
            ));
        }
        _ => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "carrier bind rejected",
            ));
        }
    }
    let mut nonce = [0_u8; LINK_NONCE_LEN];
    tokio::io::AsyncReadExt::read_exact(stream, &mut nonce).await?;
    Ok(nonce)
}

pub async fn write_carrier_bind_ack(
    stream: &mut tokio::net::TcpStream,
    server_nonce: Option<&LinkNonce>,
) -> std::io::Result<()> {
    match server_nonce {
        Some(nonce) => {
            let mut out = [0_u8; 1 + LINK_NONCE_LEN];
            out[0] = BIND_ACK_OK;
            out[1..].copy_from_slice(nonce);
            tokio::io::AsyncWriteExt::write_all(stream, &out).await?;
        }
        None => tokio::io::AsyncWriteExt::write_all(stream, &[BIND_ACK_DENIED]).await?,
    }
    tokio::io::AsyncWriteExt::flush(stream).await
}

pub fn carrier_secret_for(
    config: &tz_proto::NodeConfig,
    tunnel_id: Uuid,
    client_id: Uuid,
) -> Option<String> {
    config
        .tunnels
        .iter()
        .find(|tunnel| tunnel.tunnel_id == tunnel_id && tunnel.client_id == Some(client_id))
        .map(|tunnel| tunnel.carrier_secret.trim().to_string())
        .filter(|secret| !secret.is_empty())
}

pub fn carrier_tunnel_authorized(
    config: &tz_proto::NodeConfig,
    tunnel_id: Uuid,
    client_id: Uuid,
) -> bool {
    config.tunnels.iter().any(|tunnel| {
        tunnel.tunnel_id == tunnel_id && tunnel.client_id == Some(client_id)
    })
}

#[derive(Debug, Clone, Copy, Default)]
pub struct SessionStats {
    pub streams_opened: u64,
    pub streams_closed: u64,
    pub bytes_sent: u64,
    pub bytes_received: u64,
    pub datagrams_sent: u64,
    pub datagrams_received: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SocketKind {
    Tcp,
    Udp,
}

impl SocketKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Tcp => "tcp",
            Self::Udp => "udp",
        }
    }
}

#[derive(Debug, Error)]
pub enum OpenError {
    #[error("carrier stream was rejected")]
    Rejected,
    #[error("carrier session is closed")]
    Closed,
    #[error("carrier budget exceeded")]
    BudgetExceeded,
}

#[derive(Debug, Error)]
pub enum DatagramDropped {
    #[error("datagram queue is full")]
    QueueFull,
    #[error("carrier session is closed")]
    Closed,
}

#[derive(Clone)]
pub struct ListenConfig {
    pub bind_addr: String,
    pub port: u16,
    pub node_config: std::sync::Arc<arc_swap::ArcSwap<tz_proto::NodeConfig>>,
}

#[derive(Debug, Clone)]
pub struct ConnectConfig {
    pub server_addr: SocketAddr,
    pub tunnel_id: Uuid,
    pub client_id: Uuid,
    /// Board 下发的 TCP carrier 隧道密钥（QUIC 忽略）。
    pub carrier_secret: String,
}

/// 单条链路的双向 AEAD 密钥（client→server / server→client）。
#[derive(Debug, Clone, Copy)]
pub struct TcpCarrierKeys {
    pub client_to_server: [u8; 32],
    pub server_to_client: [u8; 32],
}

#[cfg(feature = "tcp")]
impl TcpCarrierKeys {
    pub fn derive(
        carrier_secret: &str,
        tunnel_id: Uuid,
        client_id: Uuid,
        client_nonce: &LinkNonce,
        server_nonce: &LinkNonce,
    ) -> Self {
        use hkdf::Hkdf;
        use sha2::Sha256;
        let mut salt = [0_u8; 2 * LINK_NONCE_LEN];
        salt[..LINK_NONCE_LEN].copy_from_slice(client_nonce);
        salt[LINK_NONCE_LEN..].copy_from_slice(server_nonce);
        let hk = Hkdf::<Sha256>::new(Some(&salt), carrier_secret.trim().as_bytes());
        let mut info = Vec::with_capacity(64);
        let expand = |label: &[u8], info: &mut Vec<u8>, out: &mut [u8; 32]| {
            info.clear();
            info.extend_from_slice(label);
            info.extend_from_slice(tunnel_id.as_bytes());
            info.extend_from_slice(client_id.as_bytes());
            hk.expand(info, out).expect("tcp carrier key length");
        };
        let mut client_to_server = [0_u8; 32];
        let mut server_to_client = [0_u8; 32];
        expand(b"tanzaku-tcp-carrier-v2-c2s", &mut info, &mut client_to_server);
        expand(b"tanzaku-tcp-carrier-v2-s2c", &mut info, &mut server_to_client);
        Self {
            client_to_server,
            server_to_client,
        }
    }

    pub fn node_link(self) -> ([u8; 32], [u8; 32]) {
        (self.client_to_server, self.server_to_client)
    }

    pub fn client_link(self) -> ([u8; 32], [u8; 32]) {
        (self.server_to_client, self.client_to_server)
    }
}