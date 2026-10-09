use portable_atomic::AtomicU64;
use crate::{
    registry::{BoxFuture, CarrierError, CarrierFactory, CarrierListener, CarrierSession},
    tcp_enc::EncryptedStream,
    tcp_mux::{LinkHandler, MuxLink, TcpMuxStream},
    types::{
        carrier_secret_for, decode_carrier_bind, encode_carrier_bind, random_link_nonce,
        read_carrier_bind_ack, write_carrier_bind_ack, CarrierStream, ConnectConfig,
        DatagramDropped, FlowId, ListenConfig, LinkNonce, OpenError, PeerIdentity, SessionStats,
        SocketKind, StreamHeader, TcpCarrierKeys, CARRIER_BIND_LEN, CARRIER_BIND_TIMEOUT,
        LINK_CONFIRM, LINK_NONCE_LEN,
    },
};
use async_trait::async_trait;
use bytes::Bytes;
use dashmap::DashMap;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex, MutexGuard, OnceLock,
};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, Notify};

/// client 每隧道维持的常驻复用链路数；多条分摊单条 TCP 丢包时的队头阻塞。
const TCP_MUX_LINKS: usize = 4;
const ACCEPT_QUEUE: usize = 1024;
const DGRAM_QUEUE: usize = 1024;
const MAX_DGRAM_PAYLOAD: usize = 65_507;
/// node 打开流时没有可用链路的最长等待（client 正在重连）。
const OPEN_WAIT: Duration = Duration::from_secs(10);
/// 所有链路断开后 client 连续重拨失败该次数即关闭会话，交由上层重连并上报状态。
const MAX_REDIAL_FAILURES: u32 = 3;
/// 握手确认帧后 client 声明的链路角色；旧版一访客一链路的角色（0/1）不再接受。
const LINK_ROLE_MUX: u8 = 2;

inventory::submit! {
    CarrierFactory {
        kind: "tcp",
        encryption: "chacha20-poly1305",
        socket: SocketKind::Tcp,
        listen: listen,
        connect: connect_factory,
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn tunnel_sessions() -> &'static DashMap<uuid::Uuid, Arc<TcpMuxSession>> {
    static SESSIONS: OnceLock<DashMap<uuid::Uuid, Arc<TcpMuxSession>>> = OnceLock::new();
    SESSIONS.get_or_init(DashMap::new)
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
                        tokio::time::sleep(Duration::from_millis(50)).await;
                        continue;
                    }
                };
                let node_config = node_config.clone();
                let session_tx = session_tx.clone();
                tokio::spawn(async move {
                    match finish_tcp_link(stream, remote, node_config).await {
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

/// 返回 `Some` 仅当新建隧道会话（需向 node 注册 session）；追加链路返回 `None`。
async fn finish_tcp_link(
    stream: TcpStream,
    remote: std::net::SocketAddr,
    node_config: Arc<arc_swap::ArcSwap<tz_proto::NodeConfig>>,
) -> Result<Option<Arc<dyn CarrierSession>>, CarrierError> {
    let mut stream = stream;
    let _ = stream.set_nodelay(true);
    let cn_residency = node_config.load().cn_residency;
    if let Err(err) = crate::cn_residency::enforce_peer_cn(cn_residency, remote).await {
        let _ = tokio::time::timeout(
            Duration::from_secs(2),
            stream.write_all(&[crate::types::BIND_ACK_RESIDENCY]),
        )
        .await;
        let _ = stream.shutdown().await;
        return Err(CarrierError::Io(err));
    }
    let (stream, tunnel_id, client_id) =
        tokio::time::timeout(CARRIER_BIND_TIMEOUT, node_handshake(stream, &node_config))
            .await
            .map_err(|_| {
                CarrierError::Io(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "tcp carrier handshake timed out",
                ))
            })?
            .map_err(CarrierError::Io)?;
    // DashMap 守卫是同步锁，不能跨 await 持有：并发接入的链路会把 worker 线程全部阻塞在分片锁上。
    let (session, created) = match tunnel_sessions().entry(tunnel_id) {
        dashmap::mapref::entry::Entry::Occupied(mut entry) => {
            if entry.get().is_closed() {
                let session = TcpMuxSession::new(tunnel_id, client_id, false);
                entry.insert(session.clone());
                (session, true)
            } else {
                (entry.get().clone(), false)
            }
        }
        dashmap::mapref::entry::Entry::Vacant(entry) => {
            let session = TcpMuxSession::new(tunnel_id, client_id, false);
            entry.insert(session.clone());
            (session, true)
        }
    };
    session.attach_link(stream);
    if !created {
        return Ok(None);
    }
    tracing::info!(%tunnel_id, %client_id, "tcp carrier session ready");
    Ok(Some(session as Arc<dyn CarrierSession>))
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
) -> std::io::Result<(EncryptedStream, uuid::Uuid, uuid::Uuid)> {
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
    if stream.read_u8().await? != LINK_ROLE_MUX {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "tcp carrier link role unsupported (tz-client too old, upgrade required)",
        ));
    }
    stream.write_all(LINK_CONFIRM).await?;
    stream.flush().await?;
    Ok((stream, tunnel_id, client_id))
}

/// client 侧：与 [`node_handshake`] 对称。
async fn client_handshake(
    server_addr: std::net::SocketAddr,
    secret: &str,
    tunnel_id: uuid::Uuid,
    client_id: uuid::Uuid,
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
        confirm_and_role[LINK_CONFIRM.len()] = LINK_ROLE_MUX;
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
        let stream = client_handshake(config.server_addr, &secret, config.tunnel_id, config.client_id)
            .await
            .map_err(CarrierError::Io)?;
        let session = TcpMuxSession::new(config.tunnel_id, config.client_id, true);
        *lock(&session.dial_target) = Some(DialTarget {
            server_addr: config.server_addr,
            tunnel_id: config.tunnel_id,
            client_id: config.client_id,
            secret: Arc::from(secret),
        });
        session.attach_link(stream);
        session.maintain_links();
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

/// 一个隧道的 carrier 会话：若干条常驻复用链路。node 侧跨 client 重连保留，新链路直接挂入。
struct TcpMuxSession {
    tunnel_id: uuid::Uuid,
    peer: PeerIdentity,
    client_side: bool,
    links: Mutex<Vec<Arc<MuxLink>>>,
    links_changed: Notify,
    accept_tx: mpsc::Sender<(StreamHeader, TcpMuxStream)>,
    accept_rx: tokio::sync::Mutex<mpsc::Receiver<(StreamHeader, TcpMuxStream)>>,
    dgram_in_tx: mpsc::Sender<(FlowId, Bytes)>,
    datagram_rx: tokio::sync::Mutex<mpsc::Receiver<(FlowId, Bytes)>>,
    closed: AtomicBool,
    closed_notify: Notify,
    dial_target: Mutex<Option<DialTarget>>,
    streams_opened: AtomicU64,
}

impl TcpMuxSession {
    fn new(tunnel_id: uuid::Uuid, client_id: uuid::Uuid, client_side: bool) -> Arc<Self> {
        let (accept_tx, accept_rx) = mpsc::channel(ACCEPT_QUEUE);
        let (dgram_in_tx, dgram_in_rx) = mpsc::channel(DGRAM_QUEUE);
        Arc::new(Self {
            tunnel_id,
            peer: PeerIdentity {
                client_id: Some(client_id),
                tunnel_id: Some(tunnel_id),
            },
            client_side,
            links: Mutex::new(Vec::new()),
            links_changed: Notify::new(),
            accept_tx,
            accept_rx: tokio::sync::Mutex::new(accept_rx),
            dgram_in_tx,
            datagram_rx: tokio::sync::Mutex::new(dgram_in_rx),
            closed: AtomicBool::new(false),
            closed_notify: Notify::new(),
            dial_target: Mutex::new(None),
            streams_opened: AtomicU64::new(0),
        })
    }

    fn attach_link(self: &Arc<Self>, stream: EncryptedStream) {
        let weak = Arc::downgrade(self);
        let handler = LinkHandler {
            accept_tx: self.accept_tx.clone(),
            dgram_in_tx: self.dgram_in_tx.clone(),
        };
        let link = MuxLink::spawn(
            stream,
            self.client_side,
            handler,
            Box::new(move |link: &Arc<MuxLink>| {
                if let Some(session) = weak.upgrade() {
                    session.detach_link(link);
                }
            }),
        );
        if self.is_closed() {
            link.close();
            return;
        }
        {
            let mut links = lock(&self.links);
            links.retain(|existing| !existing.is_closed());
            links.push(link);
        }
        self.links_changed.notify_waiters();
    }

    fn detach_link(&self, link: &Arc<MuxLink>) {
        lock(&self.links).retain(|existing| !Arc::ptr_eq(existing, link) && !existing.is_closed());
        self.links_changed.notify_waiters();
    }

    fn alive_links(&self) -> usize {
        lock(&self.links).iter().filter(|link| !link.is_closed()).count()
    }

    /// 新流优先放到最近有心跳、承载流最少的链路上。
    fn pick_stream_link(&self) -> Option<Arc<MuxLink>> {
        lock(&self.links)
            .iter()
            .filter(|link| !link.is_closed())
            .min_by_key(|link| (!link.recently_alive(), link.stream_count()))
            .cloned()
    }

    /// 数据报固定走首条可用链路，保持同一 flow 的顺序。
    fn pick_datagram_link(&self) -> Option<Arc<MuxLink>> {
        let links = lock(&self.links);
        links
            .iter()
            .find(|link| !link.is_closed() && link.recently_alive())
            .or_else(|| links.iter().find(|link| !link.is_closed()))
            .cloned()
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

    async fn dial_link(self: &Arc<Self>) -> bool {
        let Some(target) = lock(&self.dial_target).clone() else {
            return false;
        };
        if self.is_closed() {
            return false;
        }
        match client_handshake(target.server_addr, &target.secret, target.tunnel_id, target.client_id).await {
            Ok(stream) => {
                self.attach_link(stream);
                true
            }
            Err(err) => {
                tracing::warn!(tunnel_id = %target.tunnel_id, ?err, "tcp carrier link dial failed");
                false
            }
        }
    }

    /// client 侧把常驻链路补到 [`TCP_MUX_LINKS`]；链路全断且重拨持续失败时关闭会话。
    fn maintain_links(self: &Arc<Self>) {
        let session = self.clone();
        tokio::spawn(async move {
            let mut backoff = Duration::from_millis(200);
            let mut failures = 0_u32;
            while !session.is_closed() {
                let changed = session.links_changed.notified();
                tokio::pin!(changed);
                changed.as_mut().enable();
                if session.alive_links() >= TCP_MUX_LINKS {
                    tokio::select! {
                        _ = &mut changed => {}
                        _ = tokio::time::sleep(Duration::from_secs(5)) => {}
                    }
                    continue;
                }
                if session.dial_link().await {
                    failures = 0;
                    backoff = Duration::from_millis(200);
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    continue;
                }
                if session.alive_links() == 0 {
                    failures += 1;
                    if failures >= MAX_REDIAL_FAILURES {
                        session.close("tcp carrier links lost");
                        break;
                    }
                }
                tokio::select! {
                    _ = session.wait_closed() => break,
                    _ = tokio::time::sleep(backoff) => {}
                }
                backoff = (backoff * 2).min(Duration::from_secs(5));
            }
        });
    }
}

#[async_trait]
impl CarrierSession for TcpMuxSession {
    async fn open_stream(&self, header: StreamHeader) -> Result<CarrierStream, OpenError> {
        let deadline = tokio::time::Instant::now() + OPEN_WAIT;
        loop {
            if self.is_closed() {
                return Err(OpenError::Closed);
            }
            let changed = self.links_changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if let Some(link) = self.pick_stream_link() {
                if let Some(stream) = link.open_stream(&header) {
                    self.streams_opened.fetch_add(1, Ordering::Relaxed);
                    return Ok(CarrierStream::tcp_mux(stream));
                }
                continue;
            }
            tokio::select! {
                _ = &mut changed => {}
                _ = tokio::time::sleep_until(deadline) => return Err(OpenError::Closed),
            }
        }
    }

    async fn accept_stream(&self) -> Result<(StreamHeader, CarrierStream), OpenError> {
        if self.is_closed() {
            return Err(OpenError::Closed);
        }
        let mut accept_rx = self.accept_rx.lock().await;
        tokio::select! {
            accepted = accept_rx.recv() => accepted
                .map(|(header, stream)| (header, CarrierStream::tcp_mux(stream)))
                .ok_or(OpenError::Closed),
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
        let Some(link) = self.pick_datagram_link() else {
            return Err(DatagramDropped::QueueFull);
        };
        if link.send_datagram(flow, data) {
            Ok(())
        } else {
            Err(DatagramDropped::QueueFull)
        }
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
        SessionStats {
            streams_opened: self.streams_opened.load(Ordering::Relaxed),
            ..SessionStats::default()
        }
    }

    fn close(&self, reason: &str) {
        if self.closed.swap(true, Ordering::SeqCst) {
            return;
        }
        tracing::info!(tunnel_id = %self.tunnel_id, reason, "tcp carrier session closed");
        let links: Vec<Arc<MuxLink>> = std::mem::take(&mut *lock(&self.links));
        for link in links {
            link.close();
        }
        self.closed_notify.notify_waiters();
        self.links_changed.notify_waiters();
        if !self.client_side {
            tunnel_sessions().remove_if(&self.tunnel_id, |_, session| std::ptr::eq(Arc::as_ptr(session), self));
        }
    }

    fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }
}
