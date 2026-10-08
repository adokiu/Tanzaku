#[cfg(feature = "quic")]
use crate::{
    quic_io::{read_carrier_bind, write_carrier_bind, QuicIo},
    quic_transport::{bind_udp_endpoint, connect},
    registry::{BoxFuture, CarrierError, CarrierFactory, CarrierListener, CarrierSession},
    stream_preamble::{read_stream_preamble, write_stream_preamble},
    types::{
        carrier_tunnel_authorized, ConnectConfig, CarrierStream, DatagramDropped, FlowId,
        ListenConfig, OpenError, PeerIdentity, SessionStats, SocketKind, StreamHeader,
        CARRIER_BIND_TIMEOUT,
    },
};
#[cfg(feature = "quic")]
use async_trait::async_trait;
#[cfg(feature = "quic")]
use bytes::Bytes;
#[cfg(feature = "quic")]
use std::{
    net::SocketAddr,
    sync::{
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
        Arc,
    },
};
#[cfg(feature = "quic")]
use tokio::sync::{mpsc, Mutex};

#[cfg(feature = "quic")]
inventory::submit! {
    CarrierFactory {
        kind: "quic",
        encryption: "quic-plain",
        socket: SocketKind::Udp,
        listen: listen,
        connect: connect_factory,
    }
}

#[cfg(feature = "quic")]
fn listen(config: ListenConfig) -> BoxFuture<'static, Result<Box<dyn CarrierListener>, CarrierError>> {
    Box::pin(async move {
        let addr: SocketAddr = format!("{}:{}", config.bind_addr, config.port)
            .parse()
            .map_err(|err| CarrierError::Io(std::io::Error::new(std::io::ErrorKind::InvalidInput, err)))?;
        let endpoint = bind_udp_endpoint(addr, true).map_err(CarrierError::Io)?;
        let (session_tx, session_rx) = mpsc::channel(64);
        let accept_endpoint = endpoint.clone();
        let node_config = config.node_config;
        tokio::spawn(async move {
            loop {
                let Some(incoming) = accept_endpoint.accept().await else {
                    break;
                };
                let session_tx = session_tx.clone();
                let node_config = node_config.clone();
                tokio::spawn(async move {
                    match finish_quic_session(incoming, node_config).await {
                        Ok(session) => {
                            let _ = session_tx.send(session).await;
                        }
                        Err(err) => {
                            tracing::warn!(?err, "quic carrier session setup failed");
                        }
                    }
                });
            }
        });
        Ok(Box::new(QuicListener {
            endpoint,
            sessions: session_rx,
        }) as Box<dyn CarrierListener>)
    })
}

#[cfg(feature = "quic")]
async fn finish_quic_session(
    incoming: quinn::Incoming,
    node_config: Arc<arc_swap::ArcSwap<tz_proto::NodeConfig>>,
) -> Result<Arc<dyn CarrierSession>, CarrierError> {
    let connection = incoming
        .await
        .map_err(|err| CarrierError::Io(std::io::Error::other(err)))?;
    let (tunnel_id, client_id) =
        match tokio::time::timeout(CARRIER_BIND_TIMEOUT, read_carrier_bind(&connection)).await {
            Ok(Ok(bound)) => bound,
            Ok(Err(err)) => {
                connection.close(1u32.into(), b"bad bind");
                return Err(CarrierError::Io(err));
            }
            Err(_) => {
                connection.close(1u32.into(), b"bind timeout");
                return Err(CarrierError::Io(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "quic carrier bind timed out",
                )));
            }
        };
    let node = node_config.load();
    if !carrier_tunnel_authorized(node.as_ref(), tunnel_id, client_id) {
        connection.close(1u32.into(), b"unauthorized");
        return Err(CarrierError::Io(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "tunnel bind not authorized",
        )));
    }
    tracing::info!(
        remote = %connection.remote_address(),
        %tunnel_id,
        %client_id,
        "quic carrier session ready"
    );
    Ok(QuicCarrierSession::spawn(
        connection,
        PeerIdentity {
            client_id: Some(client_id),
            tunnel_id: Some(tunnel_id),
        },
        None,
    ))
}

#[cfg(feature = "quic")]
fn client_udp_bind_addr(_server_addr: SocketAddr) -> SocketAddr {
    SocketAddr::from(([0, 0, 0, 0, 0, 0, 0, 0], 0))
}

#[cfg(feature = "quic")]
fn connect_factory(
    config: ConnectConfig,
) -> BoxFuture<'static, Result<Arc<dyn CarrierSession>, CarrierError>> {
    Box::pin(async move {
        let client_bind = client_udp_bind_addr(config.server_addr);
        // Endpoint 必须与 Connection 同寿：提前 drop 会立刻掐断刚建好的会话，触发重连风暴。
        let endpoint = bind_udp_endpoint(client_bind, false).map_err(CarrierError::Io)?;
        let connection = connect(&endpoint, config.server_addr)
            .await
            .map_err(CarrierError::Io)?;
        write_carrier_bind(&connection, config.tunnel_id, config.client_id)
            .await
            .map_err(CarrierError::Io)?;
        tracing::info!(
            remote = %connection.remote_address(),
            tunnel_id = %config.tunnel_id,
            "quic carrier connected"
        );
        Ok(QuicCarrierSession::spawn(
            connection,
            PeerIdentity {
                client_id: Some(config.client_id),
                tunnel_id: Some(config.tunnel_id),
            },
            Some(endpoint),
        ))
    })
}

#[cfg(feature = "quic")]
struct QuicListener {
    endpoint: quinn::Endpoint,
    sessions: mpsc::Receiver<Arc<dyn CarrierSession>>,
}

#[cfg(feature = "quic")]
#[async_trait]
impl CarrierListener for QuicListener {
    async fn accept(&mut self) -> Result<Arc<dyn CarrierSession>, CarrierError> {
        self.sessions
            .recv()
            .await
            .ok_or(CarrierError::Unavailable)
    }

    fn close(&self, reason: &str) {
        self.endpoint.close(0u32.into(), reason.as_bytes());
    }
}

/// flow(8) + message(4) + index(1) + count(1)。超过单个 QUIC datagram 的 UDP 报文拆片发送。
#[cfg(feature = "quic")]
const FRAGMENT_HEADER_LEN: usize = 14;
#[cfg(feature = "quic")]
const REASSEMBLY_MAX_PENDING: usize = 1024;
#[cfg(feature = "quic")]
const REASSEMBLY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

#[cfg(feature = "quic")]
struct Fragment {
    flow: FlowId,
    message: u32,
    index: u8,
    count: u8,
    /// 与 quinn 收到的 datagram 共享内存，不拷贝。
    payload: Bytes,
}

#[cfg(feature = "quic")]
impl Fragment {
    fn parse(bytes: Bytes) -> Option<Self> {
        if bytes.len() < FRAGMENT_HEADER_LEN {
            return None;
        }
        let flow = FlowId(u64::from_be_bytes(bytes[..8].try_into().ok()?));
        let message = u32::from_be_bytes(bytes[8..12].try_into().ok()?);
        let (index, count) = (bytes[12], bytes[13]);
        if count == 0 || index >= count {
            return None;
        }
        Some(Self {
            flow,
            message,
            index,
            count,
            payload: bytes.slice(FRAGMENT_HEADER_LEN..),
        })
    }
}

#[cfg(feature = "quic")]
struct PartialMessage {
    parts: Vec<Option<Bytes>>,
    received: u8,
    started: std::time::Instant,
}

#[cfg(feature = "quic")]
#[derive(Default)]
struct Reassembly {
    pending: std::collections::HashMap<(u64, u32), PartialMessage>,
}

#[cfg(feature = "quic")]
impl Reassembly {
    fn push(&mut self, fragment: Fragment) -> Option<Bytes> {
        if fragment.count == 1 {
            return Some(fragment.payload);
        }
        if self.pending.len() >= REASSEMBLY_MAX_PENDING {
            let now = std::time::Instant::now();
            self.pending
                .retain(|_, partial| now.duration_since(partial.started) < REASSEMBLY_TIMEOUT);
            if self.pending.len() >= REASSEMBLY_MAX_PENDING {
                self.pending.clear();
            }
        }
        let key = (fragment.flow.0, fragment.message);
        let partial = self.pending.entry(key).or_insert_with(|| PartialMessage {
            parts: vec![None; usize::from(fragment.count)],
            received: 0,
            started: std::time::Instant::now(),
        });
        let slot = partial.parts.get_mut(usize::from(fragment.index))?;
        if slot.is_none() {
            *slot = Some(fragment.payload);
            partial.received += 1;
        }
        if partial.received < fragment.count {
            return None;
        }
        let partial = self.pending.remove(&key)?;
        let total = partial.parts.iter().flatten().map(Bytes::len).sum();
        let mut payload = bytes::BytesMut::with_capacity(total);
        for part in partial.parts.into_iter().flatten() {
            payload.extend_from_slice(&part);
        }
        Some(payload.freeze())
    }
}

#[cfg(feature = "quic")]
struct QuicCarrierSession {
    next_message: AtomicU32,
    connection: quinn::Connection,
    /// Client 侧持有，防止 Endpoint drop 关掉 Connection。
    _endpoint: Option<quinn::Endpoint>,
    accept_rx: Mutex<mpsc::Receiver<(StreamHeader, CarrierStream)>>,
    datagram_rx: tokio::sync::Mutex<mpsc::Receiver<(FlowId, Bytes)>>,
    closed: AtomicBool,
    peer: PeerIdentity,
    stats: SessionStats,
    streams_opened: AtomicU64,
}

#[cfg(feature = "quic")]
impl QuicCarrierSession {
    fn spawn(
        connection: quinn::Connection,
        peer: PeerIdentity,
        endpoint: Option<quinn::Endpoint>,
    ) -> Arc<dyn CarrierSession> {
        let (accept_tx, accept_rx) = mpsc::channel(64);
        let (datagram_tx, datagram_rx) = mpsc::channel(1024);
        let conn = connection.clone();
        tokio::spawn(async move {
            loop {
                let accepted = conn.accept_bi().await;
                let Ok((send, recv)) = accepted else {
                    break;
                };
                let mut io = QuicIo::new(send, recv);
                let header = match read_stream_preamble(&mut io).await {
                    Ok(header) => header,
                    Err(err) => {
                        tracing::debug!(?err, "quic stream preamble failed");
                        continue;
                    }
                };
                if accept_tx
                    .send((header, CarrierStream::quic(io)))
                    .await
                    .is_err()
                {
                    break;
                }
            }
        });
        let conn = connection.clone();
        tokio::spawn(async move {
            let mut reassembly = Reassembly::default();
            loop {
                let Ok(bytes) = conn.read_datagram().await else {
                    break;
                };
                let Some(fragment) = Fragment::parse(bytes) else {
                    continue;
                };
                let flow = fragment.flow;
                let Some(payload) = reassembly.push(fragment) else {
                    continue;
                };
                if datagram_tx.send((flow, payload)).await.is_err() {
                    break;
                }
            }
        });
        Arc::new(Self {
            next_message: AtomicU32::new(0),
            connection,
            _endpoint: endpoint,
            accept_rx: Mutex::new(accept_rx),
            datagram_rx: tokio::sync::Mutex::new(datagram_rx),
            closed: AtomicBool::new(false),
            peer,
            stats: SessionStats::default(),
            streams_opened: AtomicU64::new(0),
        })
    }
}

#[cfg(feature = "quic")]
impl QuicCarrierSession {
    fn fragments(&self, flow: FlowId, data: &Bytes) -> Result<Vec<Bytes>, DatagramDropped> {
        if self.is_closed() {
            return Err(DatagramDropped::Closed);
        }
        let max = self
            .connection
            .max_datagram_size()
            .ok_or(DatagramDropped::Closed)?;
        let chunk = max.saturating_sub(FRAGMENT_HEADER_LEN).max(1);
        let count = data.len().div_ceil(chunk).max(1);
        let count = u8::try_from(count).map_err(|_| DatagramDropped::QueueFull)?;
        let message = self.next_message.fetch_add(1, Ordering::Relaxed);
        let mut frames = Vec::with_capacity(usize::from(count));
        for index in 0..count {
            let start = usize::from(index) * chunk;
            let end = (start + chunk).min(data.len());
            let mut frame = bytes::BytesMut::with_capacity(FRAGMENT_HEADER_LEN + end - start);
            frame.extend_from_slice(&flow.0.to_be_bytes());
            frame.extend_from_slice(&message.to_be_bytes());
            frame.extend_from_slice(&[index, count]);
            frame.extend_from_slice(&data[start..end]);
            frames.push(frame.freeze());
        }
        Ok(frames)
    }
}

#[cfg(feature = "quic")]
fn map_send_error(error: quinn::SendDatagramError) -> DatagramDropped {
    match error {
        quinn::SendDatagramError::ConnectionLost(_) => DatagramDropped::Closed,
        _ => DatagramDropped::QueueFull,
    }
}

#[cfg(feature = "quic")]
#[async_trait]
impl CarrierSession for QuicCarrierSession {
    async fn open_stream(&self, header: StreamHeader) -> Result<CarrierStream, OpenError> {
        if self.is_closed() {
            return Err(OpenError::Closed);
        }
        self.streams_opened.fetch_add(1, Ordering::Relaxed);
        let (mut send, recv) = self
            .connection
            .open_bi()
            .await
            .map_err(|_| OpenError::Closed)?;
        write_stream_preamble(&mut send, &header)
            .await
            .map_err(|_| OpenError::Rejected)?;
        Ok(CarrierStream::quic(QuicIo::new(send, recv)))
    }

    async fn accept_stream(
        &self,
    ) -> Result<(StreamHeader, CarrierStream), OpenError> {
        if self.is_closed() {
            return Err(OpenError::Closed);
        }
        self.accept_rx
            .lock()
            .await
            .recv()
            .await
            .ok_or(OpenError::Closed)
    }

    fn send_datagram(&self, flow: FlowId, data: Bytes) -> Result<(), DatagramDropped> {
        for frame in self.fragments(flow, &data)? {
            self.connection
                .send_datagram(frame)
                .map_err(map_send_error)?;
        }
        Ok(())
    }

    async fn send_datagram_wait(&self, flow: FlowId, data: Bytes) -> Result<(), DatagramDropped> {
        for frame in self.fragments(flow, &data)? {
            self.connection
                .send_datagram_wait(frame)
                .await
                .map_err(map_send_error)?;
        }
        Ok(())
    }

    async fn recv_datagram(&self) -> Result<(FlowId, Bytes), OpenError> {
        let mut rx = self.datagram_rx.lock().await;
        rx.recv().await.ok_or(OpenError::Closed)
    }

    fn peer(&self) -> &PeerIdentity {
        &self.peer
    }

    fn stats(&self) -> SessionStats {
        self.stats
    }

    fn close(&self, reason: &str) {
        if self.closed.swap(true, Ordering::SeqCst) {
            return;
        }
        tracing::info!(reason, "quic carrier session closed");
        self.connection.close(0u32.into(), reason.as_bytes());
    }

    fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst) || self.connection.close_reason().is_some()
    }
}
