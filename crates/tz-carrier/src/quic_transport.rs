#[cfg(feature = "quic")]
use crate::quic_crypto::{PlainQuicCrypto, QUIC_VERSION_CUSTOM};
#[cfg(feature = "quic")]
use quinn::{ClientConfig, Connection, ConnectionError, Endpoint, EndpointConfig, ServerConfig};
#[cfg(feature = "quic")]
use std::{net::SocketAddr, sync::Arc, time::Duration};

#[cfg(feature = "quic")]
pub fn transport_config() -> quinn::TransportConfig {
    let mut transport = quinn::TransportConfig::default();
    if let Ok(idle) = Duration::from_secs(600).try_into() {
        transport.max_idle_timeout(Some(idle));
    }
    transport.keep_alive_interval(Some(Duration::from_secs(15)));
    // BBR 按带宽与 RTT 控速，不靠填满缓冲探测：吞吐更高、排队延迟更低（同 EasyTier）。
    transport.congestion_controller_factory(Arc::new(FlooredFactory {
        inner: Arc::new(quinn::congestion::BbrConfig::default()),
    }));
    transport.enable_segmentation_offload(true);
    transport
        .max_concurrent_bidi_streams(256u32.into())
        .max_concurrent_uni_streams(256u32.into());
    // 1 MiB ≈ 1Gbps 下 8ms 排队：吸收突发又不至于缓冲膨胀拉高延迟；按需增长，空闲不占内存。
    transport.datagram_receive_buffer_size(Some(1024 * 1024));
    transport.datagram_send_buffer_size(1024 * 1024);
    transport
}

/// 拥塞窗口下限。carrier 承载的 UDP 隧道流量自带端到端控速（应用码率 / 内层 TCP），
/// QUIC 层窗口因丢包塌缩会把数据报堆在发送缓冲里被丢弃；下限只防塌缩，正常时由 BBR 决定。
#[cfg(feature = "quic")]
const MIN_CONGESTION_WINDOW: u64 = 2 * 1024 * 1024;

#[cfg(feature = "quic")]
struct FlooredFactory {
    inner: Arc<quinn::congestion::BbrConfig>,
}

#[cfg(feature = "quic")]
impl quinn::congestion::ControllerFactory for FlooredFactory {
    fn build(
        self: Arc<Self>,
        now: std::time::Instant,
        current_mtu: u16,
    ) -> Box<dyn quinn::congestion::Controller> {
        Box::new(Floored {
            inner: quinn::congestion::ControllerFactory::build(
                self.inner.clone(),
                now,
                current_mtu,
            ),
        })
    }
}

#[cfg(feature = "quic")]
struct Floored {
    inner: Box<dyn quinn::congestion::Controller>,
}

#[cfg(feature = "quic")]
impl quinn::congestion::Controller for Floored {
    fn on_sent(&mut self, now: std::time::Instant, bytes: u64, last_packet_number: u64) {
        self.inner.on_sent(now, bytes, last_packet_number);
    }

    fn on_ack(
        &mut self,
        now: std::time::Instant,
        sent: std::time::Instant,
        bytes: u64,
        app_limited: bool,
        rtt: &quinn_proto::RttEstimator,
    ) {
        self.inner.on_ack(now, sent, bytes, app_limited, rtt);
    }

    fn on_end_acks(
        &mut self,
        now: std::time::Instant,
        in_flight: u64,
        app_limited: bool,
        largest_packet_num_acked: Option<u64>,
    ) {
        self.inner
            .on_end_acks(now, in_flight, app_limited, largest_packet_num_acked);
    }

    fn on_congestion_event(
        &mut self,
        now: std::time::Instant,
        sent: std::time::Instant,
        is_persistent_congestion: bool,
        lost_bytes: u64,
    ) {
        self.inner
            .on_congestion_event(now, sent, is_persistent_congestion, lost_bytes);
    }

    fn on_mtu_update(&mut self, new_mtu: u16) {
        self.inner.on_mtu_update(new_mtu);
    }

    fn window(&self) -> u64 {
        self.inner.window().max(MIN_CONGESTION_WINDOW)
    }

    fn clone_box(&self) -> Box<dyn quinn::congestion::Controller> {
        Box::new(Floored {
            inner: self.inner.clone_box(),
        })
    }

    fn initial_window(&self) -> u64 {
        self.inner.initial_window().max(MIN_CONGESTION_WINDOW)
    }

    fn into_any(self: Box<Self>) -> Box<dyn std::any::Any> {
        self
    }
}

#[cfg(feature = "quic")]
pub fn server_config() -> ServerConfig {
    let mut config = ServerConfig::with_crypto(Arc::new(PlainQuicCrypto));
    config.transport_config(Arc::new(transport_config()));
    config
}

#[cfg(feature = "quic")]
pub fn client_config() -> ClientConfig {
    let mut config = ClientConfig::new(Arc::new(PlainQuicCrypto));
    config.transport_config(Arc::new(transport_config()));
    config
}

#[cfg(feature = "quic")]
pub fn client_config_custom_version() -> ClientConfig {
    let mut config = client_config();
    config.version(QUIC_VERSION_CUSTOM);
    config
}

#[cfg(feature = "quic")]
pub fn endpoint_config() -> EndpointConfig {
    let mut config = EndpointConfig::default();
    // 上限；实际大小由 quinn 的 MTU 探测从 1200 起逐步确认，路径 MTU 较小时自动回落。
    let _ = config.max_udp_payload_size(1472);
    config.supported_versions(vec![QUIC_VERSION_CUSTOM, 1]);
    config
}

#[cfg(feature = "quic")]
pub async fn connect(endpoint: &Endpoint, addr: SocketAddr) -> std::io::Result<Connection> {
    match endpoint
        .connect_with(client_config_custom_version(), addr, "tanzaku")
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidInput, err.to_string()))?
        .await
    {
        Ok(connection) => Ok(connection),
        Err(ConnectionError::VersionMismatch) => endpoint
            .connect_with(client_config(), addr, "tanzaku")
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidInput, err.to_string()))?
            .await
            .map_err(std::io::Error::other),
        Err(error) => Err(std::io::Error::other(error)),
    }
}

#[cfg(feature = "quic")]
const UDP_SOCKET_BUFFER: usize = 4 * 1024 * 1024;

#[cfg(feature = "quic")]
pub fn bind_udp_endpoint(
    addr: std::net::SocketAddr,
    server: bool,
) -> std::io::Result<Endpoint> {
    let socket = std::net::UdpSocket::bind(addr)?;
    tz_net::sockopt::set_udp_buffers(&socket, UDP_SOCKET_BUFFER);
    let runtime = Arc::new(quinn::TokioRuntime);
    if server {
        Endpoint::new(
            endpoint_config(),
            Some(server_config()),
            socket,
            runtime,
        )
    } else {
        Endpoint::new(endpoint_config(), None, socket, runtime)
    }
}
