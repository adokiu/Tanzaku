use crate::{
    registry::{BoxFuture, CarrierError, CarrierFactory, CarrierListener, CarrierSession},
    stream_preamble::{read_stream_preamble, write_stream_preamble},
    tcp_enc::EncryptedStream,
    types::{
        carrier_secret_for, decode_carrier_bind, encode_carrier_bind, random_link_nonce,
        read_carrier_bind_ack, write_carrier_bind_ack, CarrierStream, ConnectConfig,
        DatagramDropped, FlowId, LinkNonce, ListenConfig, OpenError, PeerIdentity,
        SessionStats, SocketKind, StreamHeader, TcpCarrierKeys, CARRIER_BIND_LEN,
        CARRIER_BIND_TIMEOUT, LINK_CONFIRM, LINK_NONCE_LEN,
    },
};
use async_trait::async_trait;
use bytes::Bytes;
use dashmap::DashMap;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc, OnceLock,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, watch, Mutex, Notify};

/// 数据面长连数量（TCP/HTTP 隧道每条访客占一条）。
const TCP_LINK_POOL: usize = 8;
/// UDP 报文在控制连接上的帧类型。
const DGRAM_FRAME: u8 = 0x06;
const DGRAM_QUEUE: usize = 1024;
/// 控制链路一次 flush 合并的数据报上限（只合并已排队的，不等待凑批，不增加延迟）。
const DGRAM_WRITE_BATCH: usize = 64;
const DGRAM_WRITE_BATCH_BYTES: usize = 64 * 1024;
const MAX_DGRAM_PAYLOAD: usize = 65_507;
/// 握手确认帧后 client 声明的链路角色。
const LINK_ROLE_DATA: u8 = 0;
const LINK_ROLE_CONTROL: u8 = 1;

#[derive(Clone, Copy, PartialEq, Eq)]
enum LinkRole {
    /// 承载 UDP 数据报；新的控制链路会接管并替换旧的（client 重连后 node 侧池子仍在）。
    Control,
    Data,
}

impl LinkRole {
    fn wire(self) -> u8 {
        match self {
            Self::Control => LINK_ROLE_CONTROL,
            Self::Data => LINK_ROLE_DATA,
        }
    }
}

inventory::submit! {
    CarrierFactory {
        kind: "tcp",
        encryption: "chacha20-poly1305",
        socket: SocketKind::Tcp,
        listen: listen,
        connect: connect_factory,
    }
}

fn tunnel_pools() -> &'static DashMap<uuid::Uuid, Arc<TcpPoolSession>> {
    static POOLS: OnceLock<DashMap<uuid::Uuid, Arc<TcpPoolSession>>> = OnceLock::new();
    POOLS.get_or_init(DashMap::new)
}

fn listen(config: ListenConfig) -> BoxFuture<'static, Result<Box<dyn CarrierListener>, CarrierError>> {
    Box::pin(async move {
        let listener = TcpListener::bind((config.bind_addr.as_str(), config.port)).await?;
        let (session_tx, session_rx) = mpsc::channel(64);
        let node_config = config.node_config;
        let accept_task = tokio::spawn(async move {
            loop {
                let (stream, remote) = match listener.accept().await {
                    Ok(accepted) => accepted,
                    Err(err) => {
                        tracing::warn!(?err, "tcp carrier accept failed");
                        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                        continue;
                    }
                };
                let node_config = node_config.clone();
                let session_tx = session_tx.clone();
                tokio::spawn(async move {
                    match finish_tcp_link(stream, node_config).await {
                        Ok(Some(session)) => {
                            let _ = session_tx.send(session).await;
                        }
                        Ok(None) => {}
                        Err(err) => {
                            tracing::warn!(%remote, ?err, "tcp carrier link setup failed");
                        }
                    }
                });
            }
        });
        Ok(Box::new(TcpCarrierListener {
            sessions: session_rx,
            accept_task,
        }) as Box<dyn CarrierListener>)
    })
}

/// 返回 `Some` 仅当新建隧道池（需向 node 注册 session）；追加链路返回 `None`。
async fn finish_tcp_link(
    stream: TcpStream,
    node_config: Arc<arc_swap::ArcSwap<tz_proto::NodeConfig>>,
) -> Result<Option<Arc<dyn CarrierSession>>, CarrierError> {
    let (stream, tunnel_id, client_id, role) =
        tokio::time::timeout(CARRIER_BIND_TIMEOUT, node_handshake(stream, &node_config))
            .await
            .map_err(|_| {
                CarrierError::Io(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "tcp carrier handshake timed out",
                ))
            })?
            .map_err(CarrierError::Io)?;
    match tunnel_pools().entry(tunnel_id) {
        dashmap::mapref::entry::Entry::Occupied(entry) => {
            entry.get().attach_node_link(stream, role).await;
            Ok(None)
        }
        dashmap::mapref::entry::Entry::Vacant(entry) => {
            let session = TcpPoolSession::new(tunnel_id, client_id, false);
            session.attach_node_link(stream, role).await;
            entry.insert(session.clone());
            tracing::info!(%tunnel_id, %client_id, "tcp carrier pool ready");
            Ok(Some(session as Arc<dyn CarrierSession>))
        }
    }
}

fn confirm_mismatch() -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::PermissionDenied,
        "tcp carrier key confirmation failed (carrier_secret mismatch)",
    )
}

/// node 侧：bind + client 随机数 → 鉴权 → ack + server 随机数 → 互验加密确认帧。
async fn node_handshake(
    mut stream: TcpStream,
    node_config: &arc_swap::ArcSwap<tz_proto::NodeConfig>,
) -> std::io::Result<(EncryptedStream, uuid::Uuid, uuid::Uuid, LinkRole)> {
    let mut bind = [0_u8; CARRIER_BIND_LEN];
    stream.read_exact(&mut bind).await?;
    let (tunnel_id, client_id) = decode_carrier_bind(&bind)
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid bind"))?;
    let mut client_nonce: LinkNonce = [0_u8; LINK_NONCE_LEN];
    stream.read_exact(&mut client_nonce).await?;
    let Some(secret) = carrier_secret_for(node_config.load().as_ref(), tunnel_id, client_id)
    else {
        let _ = write_carrier_bind_ack(&mut stream, None).await;
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "tunnel bind not authorized or carrier_secret missing",
        ));
    };
    let server_nonce = random_link_nonce()?;
    write_carrier_bind_ack(&mut stream, Some(&server_nonce)).await?;
    let keys = TcpCarrierKeys::derive(&secret, tunnel_id, client_id, &client_nonce, &server_nonce);
    let (read_key, write_key) = keys.node_link();
    let mut stream = EncryptedStream::new(stream, read_key, write_key);
    let mut confirm = [0_u8; LINK_CONFIRM.len()];
    stream
        .read_exact(&mut confirm)
        .await
        .map_err(|_| confirm_mismatch())?;
    if &confirm != LINK_CONFIRM {
        return Err(confirm_mismatch());
    }
    let role = match stream.read_u8().await? {
        LINK_ROLE_CONTROL => LinkRole::Control,
        LINK_ROLE_DATA => LinkRole::Data,
        _ => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "invalid tcp carrier link role",
            ))
        }
    };
    stream.write_all(LINK_CONFIRM).await?;
    stream.flush().await?;
    Ok((stream, tunnel_id, client_id, role))
}

/// client 侧：与 [`node_handshake`] 对称。
async fn client_handshake(
    server_addr: std::net::SocketAddr,
    secret: &str,
    tunnel_id: uuid::Uuid,
    client_id: uuid::Uuid,
    role: LinkRole,
) -> std::io::Result<EncryptedStream> {
    let setup = async {
        let mut stream = TcpStream::connect(server_addr).await?;
        let _ = stream.set_nodelay(true);
        let client_nonce = random_link_nonce()?;
        let mut hello = [0_u8; CARRIER_BIND_LEN + LINK_NONCE_LEN];
        hello[..CARRIER_BIND_LEN].copy_from_slice(&encode_carrier_bind(tunnel_id, client_id));
        hello[CARRIER_BIND_LEN..].copy_from_slice(&client_nonce);
        stream.write_all(&hello).await?;
        stream.flush().await?;
        let server_nonce = read_carrier_bind_ack(&mut stream).await?;
        let keys = TcpCarrierKeys::derive(secret, tunnel_id, client_id, &client_nonce, &server_nonce);
        let (read_key, write_key) = keys.client_link();
        let mut stream = EncryptedStream::new(stream, read_key, write_key);
        let mut confirm_and_role = [0_u8; LINK_CONFIRM.len() + 1];
        confirm_and_role[..LINK_CONFIRM.len()].copy_from_slice(LINK_CONFIRM);
        confirm_and_role[LINK_CONFIRM.len()] = role.wire();
        stream.write_all(&confirm_and_role).await?;
        stream.flush().await?;
        let mut confirm = [0_u8; LINK_CONFIRM.len()];
        stream
            .read_exact(&mut confirm)
            .await
            .map_err(|_| confirm_mismatch())?;
        if &confirm != LINK_CONFIRM {
            return Err(confirm_mismatch());
        }
        Ok(stream)
    };
    tokio::time::timeout(CARRIER_BIND_TIMEOUT, setup)
        .await
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "tcp carrier handshake timed out"))?
}

fn connect_factory(
    config: ConnectConfig,
) -> BoxFuture<'static, Result<Arc<dyn CarrierSession>, CarrierError>> {
    Box::pin(async move {
        let secret = config.carrier_secret.trim().to_string();
        if secret.is_empty() {
            return Err(CarrierError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "tcp carrier missing carrier_secret from board",
            )));
        }
        let stream = client_handshake(
            config.server_addr,
            &secret,
            config.tunnel_id,
            config.client_id,
            LinkRole::Control,
        )
        .await
        .map_err(CarrierError::Io)?;
        let session = TcpPoolSession::new(config.tunnel_id, config.client_id, true);
        session.set_dial_target(DialTarget {
            server_addr: config.server_addr,
            tunnel_id: config.tunnel_id,
            client_id: config.client_id,
            secret: Arc::from(secret),
        });
        session.spawn_control_plane(stream);
        session.maintain_pool();
        Ok(session as Arc<dyn CarrierSession>)
    })
}

#[derive(Clone)]
struct DialTarget {
    server_addr: std::net::SocketAddr,
    tunnel_id: uuid::Uuid,
    client_id: uuid::Uuid,
    secret: Arc<str>,
}

struct TcpCarrierListener {
    sessions: mpsc::Receiver<Arc<dyn CarrierSession>>,
    accept_task: tokio::task::JoinHandle<()>,
}

#[async_trait]
impl CarrierListener for TcpCarrierListener {
    async fn accept(&mut self) -> Result<Arc<dyn CarrierSession>, CarrierError> {
        self.sessions.recv().await.ok_or(CarrierError::Unavailable)
    }

    fn close(&self, reason: &str) {
        tracing::info!(reason, "tcp carrier listener closed");
        self.accept_task.abort();
    }
}

struct TcpPoolSession {
    tunnel_id: uuid::Uuid,
    peer: PeerIdentity,
    client_side: bool,
    idle: Mutex<Vec<EncryptedStream>>,
    wait: Notify,
    accept_tx: mpsc::Sender<(StreamHeader, CarrierStream)>,
    accept_rx: Mutex<mpsc::Receiver<(StreamHeader, CarrierStream)>>,
    dgram_out_tx: mpsc::Sender<(FlowId, Bytes)>,
    dgram_out_rx: Arc<Mutex<mpsc::Receiver<(FlowId, Bytes)>>>,
    dgram_in_tx: mpsc::Sender<(FlowId, Bytes)>,
    datagram_rx: Mutex<mpsc::Receiver<(FlowId, Bytes)>>,
    /// 每接入一条控制链路 +1；旧控制链路任务看到变化后退出。关闭会话时也 +1。
    control_generation: watch::Sender<u64>,
    closed: Arc<AtomicBool>,
    closed_notify: Notify,
    dial_target: Mutex<Option<DialTarget>>,
    stats: SessionStats,
    streams_opened: AtomicU64,
}

async fn superseded(generation_rx: &mut watch::Receiver<u64>, generation: u64) {
    loop {
        if *generation_rx.borrow_and_update() != generation {
            return;
        }
        if generation_rx.changed().await.is_err() {
            return;
        }
    }
}

impl TcpPoolSession {
    fn new(tunnel_id: uuid::Uuid, client_id: uuid::Uuid, client_side: bool) -> Arc<Self> {
        let (accept_tx, accept_rx) = mpsc::channel(64);
        let (dgram_out_tx, dgram_out_rx) = mpsc::channel(DGRAM_QUEUE);
        let (dgram_in_tx, dgram_in_rx) = mpsc::channel(DGRAM_QUEUE);
        Arc::new(Self {
            tunnel_id,
            peer: PeerIdentity {
                client_id: Some(client_id),
                tunnel_id: Some(tunnel_id),
            },
            client_side,
            idle: Mutex::new(Vec::new()),
            wait: Notify::new(),
            accept_tx,
            accept_rx: Mutex::new(accept_rx),
            dgram_out_tx,
            dgram_out_rx: Arc::new(Mutex::new(dgram_out_rx)),
            dgram_in_tx,
            datagram_rx: Mutex::new(dgram_in_rx),
            control_generation: watch::channel(0).0,
            closed: Arc::new(AtomicBool::new(false)),
            closed_notify: Notify::new(),
            dial_target: Mutex::new(None),
            stats: SessionStats::default(),
            streams_opened: AtomicU64::new(0),
        })
    }

    fn set_dial_target(self: &Arc<Self>, target: DialTarget) {
        if let Ok(mut guard) = self.dial_target.try_lock() {
            *guard = Some(target);
        }
    }

    async fn dial_data_link(self: &Arc<Self>) {
        let Some(target) = self.dial_target.lock().await.clone() else {
            return;
        };
        if self.closed.load(Ordering::SeqCst) {
            return;
        }
        match client_handshake(
            target.server_addr,
            &target.secret,
            target.tunnel_id,
            target.client_id,
            LinkRole::Data,
        )
        .await
        {
            Ok(stream) => self.spawn_data_link(stream),
            Err(err) => {
                tracing::warn!(tunnel_id = %target.tunnel_id, ?err, "tcp carrier data link handshake failed");
            }
        }
    }

    async fn attach_node_link(self: &Arc<Self>, stream: EncryptedStream, role: LinkRole) {
        if self.closed.load(Ordering::SeqCst) {
            return;
        }
        match role {
            LinkRole::Control => self.spawn_control_plane(stream),
            LinkRole::Data => self.push_idle(stream).await,
        }
    }

    /// client 预建数据长连，bind 后等待 node 写入 stream 头。
    fn spawn_data_link(self: &Arc<Self>, stream: EncryptedStream) {
        if self.closed.load(Ordering::SeqCst) {
            return;
        }
        let accept_tx = self.accept_tx.clone();
        let session = self.clone();
        tokio::spawn(async move {
            let mut stream = stream;
            match read_stream_preamble(&mut stream).await {
                Ok(header) => {
                    let _ = accept_tx
                        .send((header, CarrierStream::tcp_encrypted(stream)))
                        .await;
                }
                Err(err) => tracing::debug!(?err, "tcp stream preamble failed"),
            }
            session.dial_data_link().await;
        });
    }

    fn spawn_control_plane(self: &Arc<Self>, stream: EncryptedStream) {
        if self.closed.load(Ordering::SeqCst) {
            return;
        }
        let mut generation = 0;
        self.control_generation.send_modify(|current| {
            *current += 1;
            generation = *current;
        });
        let (mut reader, mut writer) = tokio::io::split(stream);

        let out_rx = self.dgram_out_rx.clone();
        let closed = Arc::clone(&self.closed);
        let mut writer_generation = self.control_generation.subscribe();
        tokio::spawn(async move {
            let mut out_rx = tokio::select! {
                guard = out_rx.lock_owned() => guard,
                _ = superseded(&mut writer_generation, generation) => return,
            };
            let mut batch = Vec::new();
            loop {
                let first = tokio::select! {
                    next = out_rx.recv() => match next {
                        Some(next) => next,
                        None => break,
                    },
                    _ = superseded(&mut writer_generation, generation) => break,
                };
                if closed.load(Ordering::SeqCst) {
                    break;
                }
                batch.clear();
                let mut next = Some(first);
                let mut count = 0;
                while let Some((flow, payload)) = next.take() {
                    if payload.len() <= MAX_DGRAM_PAYLOAD {
                        let len = payload.len() as u16;
                        batch.push(DGRAM_FRAME);
                        batch.extend_from_slice(&len.to_be_bytes());
                        batch.extend_from_slice(&flow.0.to_be_bytes());
                        batch.extend_from_slice(&payload);
                    }
                    count += 1;
                    if count < DGRAM_WRITE_BATCH && batch.len() < DGRAM_WRITE_BATCH_BYTES {
                        next = out_rx.try_recv().ok();
                    }
                }
                if writer.write_all(&batch).await.is_err() || writer.flush().await.is_err() {
                    break;
                }
                if batch.capacity() > 4 * DGRAM_WRITE_BATCH_BYTES {
                    batch = Vec::new();
                }
            }
        });

        let in_tx = self.dgram_in_tx.clone();
        let session = self.clone();
        let mut reader_generation = self.control_generation.subscribe();
        tokio::spawn(async move {
            let read_loop = async {
                let mut frame = [0_u8; 3];
                loop {
                    if reader.read_exact(&mut frame).await.is_err() {
                        break;
                    }
                    if frame[0] != DGRAM_FRAME {
                        break;
                    }
                    let len = u16::from_be_bytes([frame[1], frame[2]]) as usize;
                    let mut flow_bytes = [0_u8; 8];
                    if reader.read_exact(&mut flow_bytes).await.is_err() {
                        break;
                    }
                    let mut payload = vec![0_u8; len];
                    if reader.read_exact(&mut payload).await.is_err() {
                        break;
                    }
                    let flow = FlowId(u64::from_be_bytes(flow_bytes));
                    if in_tx.send((flow, Bytes::from(payload))).await.is_err() {
                        break;
                    }
                }
            };
            tokio::select! {
                _ = read_loop => {
                    if session.client_side {
                        session.close("tcp carrier control link lost");
                    }
                }
                _ = superseded(&mut reader_generation, generation) => {}
            }
        });
    }

    async fn wait_closed(&self) {
        let notified = self.closed_notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        if self.closed.load(Ordering::SeqCst) {
            return;
        }
        notified.await;
    }

    async fn push_idle(&self, stream: EncryptedStream) {
        if self.closed.load(Ordering::SeqCst) {
            return;
        }
        self.idle.lock().await.push(stream);
        self.wait.notify_waiters();
    }

    fn maintain_pool(self: &Arc<Self>) {
        let session = self.clone();
        tokio::spawn(async move {
            for _ in 0..TCP_LINK_POOL {
                session.dial_data_link().await;
            }
        });
    }

    async fn take_idle(&self) -> Option<EncryptedStream> {
        let deadline = tokio::time::Instant::now() + CARRIER_BIND_TIMEOUT;
        loop {
            if let Some(stream) = self.idle.lock().await.pop() {
                return Some(stream);
            }
            if self.closed.load(Ordering::SeqCst) {
                return None;
            }
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return None;
            }
            tokio::select! {
                _ = self.wait.notified() => {}
                _ = tokio::time::sleep(remaining) => return None,
            }
        }
    }
}

#[async_trait]
impl CarrierSession for TcpPoolSession {
    async fn open_stream(&self, header: StreamHeader) -> Result<CarrierStream, OpenError> {
        if self.is_closed() {
            return Err(OpenError::Closed);
        }
        self.streams_opened.fetch_add(1, Ordering::Relaxed);
        let mut stream = self.take_idle().await.ok_or(OpenError::Closed)?;
        write_stream_preamble(&mut stream, &header)
            .await
            .map_err(|_| OpenError::Rejected)?;
        Ok(CarrierStream::tcp_encrypted(stream))
    }

    async fn accept_stream(
        &self,
    ) -> Result<(StreamHeader, CarrierStream), OpenError> {
        if self.is_closed() {
            return Err(OpenError::Closed);
        }
        let mut accept_rx = self.accept_rx.lock().await;
        tokio::select! {
            accepted = accept_rx.recv() => accepted.ok_or(OpenError::Closed),
            _ = self.wait_closed() => Err(OpenError::Closed),
        }
    }

    fn send_datagram(&self, flow: FlowId, data: Bytes) -> Result<(), DatagramDropped> {
        if self.is_closed() {
            return Err(DatagramDropped::Closed);
        }
        if data.len() > MAX_DGRAM_PAYLOAD {
            return Err(DatagramDropped::QueueFull);
        }
        self.dgram_out_tx
            .try_send((flow, data))
            .map_err(|_| DatagramDropped::QueueFull)
    }

    async fn recv_datagram(&self) -> Result<(FlowId, Bytes), OpenError> {
        if self.is_closed() {
            return Err(OpenError::Closed);
        }
        let mut datagram_rx = self.datagram_rx.lock().await;
        tokio::select! {
            received = datagram_rx.recv() => received.ok_or(OpenError::Closed),
            _ = self.wait_closed() => Err(OpenError::Closed),
        }
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
        tracing::info!(reason, "tcp carrier pool closed");
        self.control_generation.send_modify(|current| *current += 1);
        self.closed_notify.notify_waiters();
        self.wait.notify_waiters();
        if !self.client_side {
            tunnel_pools().remove(&self.tunnel_id);
        }
    }

    fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }
}
