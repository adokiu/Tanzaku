//! TCP carrier 多路复用：每隧道少量常驻加密链路，访客流以帧复用其上（单连接分帧 + 合并写）。
//! 帧格式（`EncryptedStream` 明文内）：`type u8 | stream_id u32 BE | len u32 BE | payload`。
//! 每流信用流控：接收方消费后回 WINDOW，慢流只耗尽自己的窗口，不阻塞同链路其他流。
//! stream_id 由打开方分配：client 奇数，node 偶数。

use portable_atomic::AtomicU64;
use crate::stream_preamble::{decode_open_payload, encode_open_payload};
use crate::tcp_enc::EncryptedStream;
use crate::types::{FlowId, StreamHeader};
use bytes::{Buf, BufMut, Bytes, BytesMut};
use std::collections::{HashMap, VecDeque};
use std::io;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::task::{Context, Poll, Waker};
use std::time::{Duration, Instant};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio::sync::{mpsc, watch};

const FRAME_OPEN: u8 = 1;
const FRAME_DATA: u8 = 2;
const FRAME_WINDOW: u8 = 3;
const FRAME_FIN: u8 = 4;
const FRAME_RST: u8 = 5;
const FRAME_DGRAM: u8 = 6;
const FRAME_PING: u8 = 7;

const FRAME_HEADER_LEN: usize = 9;
const MAX_DATA_FRAME: usize = 16 * 1024;
const MAX_FRAME_PAYLOAD: usize = 70 * 1024;
/// 单流接收窗口：吞吐上限约为 窗口 / RTT，同时也是慢消费者单流最多占用的缓冲。
const STREAM_WINDOW: usize = 512 * 1024;
const WINDOW_UPDATE_THRESHOLD: usize = STREAM_WINDOW / 4;
/// 一次 write 合并的帧字节上限（只合并已排队的，不等待凑批）。
const WRITE_BATCH_BYTES: usize = 256 * 1024;
const PING_INTERVAL: Duration = Duration::from_secs(15);
const LINK_IDLE_TIMEOUT: Duration = Duration::from_secs(45);
/// 链路待发数据报字节上限，超出直接丢弃（UDP 语义），避免慢链路无限堆积。
const DGRAM_LINK_BACKLOG: usize = 1024 * 1024;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

enum Outbound {
    Open { id: u32, payload: Vec<u8> },
    Data { id: u32, payload: Bytes },
    Window { id: u32, increment: u32 },
    Fin { id: u32 },
    Rst { id: u32 },
    Dgram { flow: FlowId, payload: Bytes },
    Ping,
}

struct StreamState {
    recv_buf: VecDeque<Bytes>,
    recv_window_left: usize,
    consumed: usize,
    recv_fin: bool,
    peer_reset: bool,
    link_lost: bool,
    send_credit: usize,
    fin_sent: bool,
    read_waker: Option<Waker>,
    write_waker: Option<Waker>,
}

struct StreamShared {
    state: Mutex<StreamState>,
}

impl StreamShared {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(StreamState {
                recv_buf: VecDeque::new(),
                recv_window_left: STREAM_WINDOW,
                consumed: 0,
                recv_fin: false,
                peer_reset: false,
                link_lost: false,
                send_credit: STREAM_WINDOW,
                fin_sent: false,
                read_waker: None,
                write_waker: None,
            }),
        })
    }
}

/// 链路收到对端打开的流、数据报时交给所属会话。
pub struct LinkHandler {
    pub accept_tx: mpsc::Sender<(StreamHeader, TcpMuxStream)>,
    pub dgram_in_tx: mpsc::Sender<(FlowId, Bytes)>,
}

pub type LinkClosed = Box<dyn FnOnce(&Arc<MuxLink>) + Send>;

pub struct MuxLink {
    client_side: bool,
    out_tx: mpsc::UnboundedSender<Outbound>,
    streams: Mutex<HashMap<u32, Arc<StreamShared>>>,
    next_id: AtomicU32,
    closed: AtomicBool,
    shutdown: watch::Sender<bool>,
    epoch: Instant,
    last_rx_ms: AtomicU64,
    dgram_backlog: AtomicUsize,
}

impl MuxLink {
    pub fn spawn(
        stream: EncryptedStream,
        client_side: bool,
        handler: LinkHandler,
        on_closed: LinkClosed,
    ) -> Arc<Self> {
        let (out_tx, mut out_rx) = mpsc::unbounded_channel();
        let (shutdown, _) = watch::channel(false);
        let link = Arc::new(Self {
            client_side,
            out_tx,
            streams: Mutex::new(HashMap::new()),
            next_id: AtomicU32::new(if client_side { 1 } else { 2 }),
            closed: AtomicBool::new(false),
            shutdown,
            epoch: Instant::now(),
            last_rx_ms: AtomicU64::new(0),
            dgram_backlog: AtomicUsize::new(0),
        });
        let (mut reader, mut writer) = tokio::io::split(stream);

        let reader_link = link.clone();
        let mut reader_shutdown = link.shutdown.subscribe();
        tokio::spawn(async move {
            let result = tokio::select! {
                result = read_loop(&reader_link, &mut reader, &handler) => result,
                _ = wait_shutdown(&mut reader_shutdown) => Ok(()),
            };
            if let Err(error) = result {
                tracing::debug!(?error, "tcp carrier link read ended");
            }
            reader_link.close();
            reader_link.fail_streams();
            on_closed(&reader_link);
        });

        let writer_link = link.clone();
        let mut writer_shutdown = link.shutdown.subscribe();
        tokio::spawn(async move {
            let result = tokio::select! {
                result = write_loop(&writer_link, &mut writer, &mut out_rx) => result,
                _ = wait_shutdown(&mut writer_shutdown) => Ok(()),
            };
            if let Err(error) = result {
                tracing::debug!(?error, "tcp carrier link write ended");
            }
            writer_link.close();
            let _ = tokio::time::timeout(Duration::from_secs(2), writer.shutdown()).await;
        });
        link
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    pub fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        let _ = self.shutdown.send_replace(true);
    }

    pub fn stream_count(&self) -> usize {
        lock(&self.streams).len()
    }

    /// 最近两个心跳周期内收到过对端帧；半开链路（对端已消失）会落在这之外。
    pub fn recently_alive(&self) -> bool {
        let now = self.epoch.elapsed().as_millis() as u64;
        now.saturating_sub(self.last_rx_ms.load(Ordering::Relaxed))
            < 2 * PING_INTERVAL.as_millis() as u64
    }

    pub fn open_stream(self: &Arc<Self>, header: &StreamHeader) -> Option<TcpMuxStream> {
        if self.is_closed() {
            return None;
        }
        let id = self.next_id.fetch_add(2, Ordering::Relaxed);
        if id > u32::MAX - 4 {
            self.close();
            return None;
        }
        let shared = StreamShared::new();
        lock(&self.streams).insert(id, shared.clone());
        let stream = TcpMuxStream {
            id,
            shared,
            link: self.clone(),
        };
        // 插入后再查一次：与 close → fail_streams 的顺序配合，保证不会留下永远等不到结果的流。
        if self.is_closed() {
            return None;
        }
        let payload = encode_open_payload(header.tunnel_id, header.src_addr);
        if !self.send(Outbound::Open { id, payload }) {
            return None;
        }
        Some(stream)
    }

    pub fn send_datagram(&self, flow: FlowId, payload: Bytes) -> bool {
        if self.is_closed() || self.dgram_backlog.load(Ordering::Relaxed) > DGRAM_LINK_BACKLOG {
            return false;
        }
        self.dgram_backlog.fetch_add(payload.len(), Ordering::Relaxed);
        self.send(Outbound::Dgram { flow, payload })
    }

    fn send(&self, frame: Outbound) -> bool {
        !self.is_closed() && self.out_tx.send(frame).is_ok()
    }

    fn touch(&self) {
        self.last_rx_ms
            .store(self.epoch.elapsed().as_millis() as u64, Ordering::Relaxed);
    }

    fn get_stream(&self, id: u32) -> Option<Arc<StreamShared>> {
        lock(&self.streams).get(&id).cloned()
    }

    fn remove_stream(&self, id: u32) {
        lock(&self.streams).remove(&id);
    }

    fn fail_streams(&self) {
        let streams: Vec<Arc<StreamShared>> = lock(&self.streams).drain().map(|(_, s)| s).collect();
        for shared in streams {
            let (read_waker, write_waker) = {
                let mut state = lock(&shared.state);
                state.link_lost = true;
                (state.read_waker.take(), state.write_waker.take())
            };
            if let Some(waker) = read_waker {
                waker.wake();
            }
            if let Some(waker) = write_waker {
                waker.wake();
            }
        }
    }

    fn handle_frame(
        self: &Arc<Self>,
        kind: u8,
        id: u32,
        payload: Bytes,
        handler: &LinkHandler,
    ) -> io::Result<()> {
        match kind {
            FRAME_OPEN => {
                let peer_parity = if self.client_side { 0 } else { 1 };
                if id == 0 || id % 2 != peer_parity {
                    return Err(invalid("tcp carrier open with bad stream id"));
                }
                let (tunnel_id, src_addr) = decode_open_payload(&payload)?;
                let shared = StreamShared::new();
                {
                    let mut streams = lock(&self.streams);
                    if streams.contains_key(&id) {
                        return Err(invalid("tcp carrier duplicate stream id"));
                    }
                    streams.insert(id, shared.clone());
                }
                let stream = TcpMuxStream {
                    id,
                    shared,
                    link: self.clone(),
                };
                if handler
                    .accept_tx
                    .try_send((StreamHeader { tunnel_id, src_addr }, stream))
                    .is_err()
                {
                    // 被退回的流在此处析构，向对端发 FIN/RST。
                    tracing::warn!(%tunnel_id, "tcp carrier accept queue full; stream rejected");
                }
            }
            FRAME_DATA => {
                let Some(shared) = self.get_stream(id) else {
                    return Ok(());
                };
                let waker = {
                    let mut state = lock(&shared.state);
                    if state.recv_fin || state.peer_reset {
                        None
                    } else if payload.len() > state.recv_window_left {
                        return Err(invalid("tcp carrier stream window exceeded"));
                    } else {
                        state.recv_window_left -= payload.len();
                        if !payload.is_empty() {
                            state.recv_buf.push_back(payload);
                        }
                        state.read_waker.take()
                    }
                };
                if let Some(waker) = waker {
                    waker.wake();
                }
            }
            FRAME_WINDOW => {
                if payload.len() != 4 {
                    return Err(invalid("tcp carrier bad window frame"));
                }
                let increment = u32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]);
                if let Some(shared) = self.get_stream(id) {
                    let waker = {
                        let mut state = lock(&shared.state);
                        state.send_credit = state.send_credit.saturating_add(increment as usize);
                        state.write_waker.take()
                    };
                    if let Some(waker) = waker {
                        waker.wake();
                    }
                }
            }
            FRAME_FIN => {
                if let Some(shared) = self.get_stream(id) {
                    let waker = {
                        let mut state = lock(&shared.state);
                        state.recv_fin = true;
                        state.read_waker.take()
                    };
                    if let Some(waker) = waker {
                        waker.wake();
                    }
                }
            }
            FRAME_RST => {
                let shared = lock(&self.streams).remove(&id);
                if let Some(shared) = shared {
                    let (read_waker, write_waker) = {
                        let mut state = lock(&shared.state);
                        state.peer_reset = true;
                        (state.read_waker.take(), state.write_waker.take())
                    };
                    if let Some(waker) = read_waker {
                        waker.wake();
                    }
                    if let Some(waker) = write_waker {
                        waker.wake();
                    }
                }
            }
            FRAME_DGRAM => {
                if payload.len() < 8 {
                    return Err(invalid("tcp carrier bad datagram frame"));
                }
                let flow = u64::from_be_bytes([
                    payload[0], payload[1], payload[2], payload[3], payload[4], payload[5],
                    payload[6], payload[7],
                ]);
                let _ = handler
                    .dgram_in_tx
                    .try_send((FlowId(flow), payload.slice(8..)));
            }
            FRAME_PING => {}
            _ => return Err(invalid("tcp carrier unknown frame type")),
        }
        Ok(())
    }

    fn encode(&self, frame: Outbound, buf: &mut BytesMut) {
        match frame {
            Outbound::Open { id, payload } => {
                put_header(buf, FRAME_OPEN, id, payload.len());
                buf.extend_from_slice(&payload);
            }
            Outbound::Data { id, payload } => {
                put_header(buf, FRAME_DATA, id, payload.len());
                buf.extend_from_slice(&payload);
            }
            Outbound::Window { id, increment } => {
                put_header(buf, FRAME_WINDOW, id, 4);
                buf.put_u32(increment);
            }
            Outbound::Fin { id } => put_header(buf, FRAME_FIN, id, 0),
            Outbound::Rst { id } => put_header(buf, FRAME_RST, id, 0),
            Outbound::Dgram { flow, payload } => {
                self.dgram_backlog.fetch_sub(
                    payload.len().min(self.dgram_backlog.load(Ordering::Relaxed)),
                    Ordering::Relaxed,
                );
                put_header(buf, FRAME_DGRAM, 0, 8 + payload.len());
                buf.put_u64(flow.0);
                buf.extend_from_slice(&payload);
            }
            Outbound::Ping => put_header(buf, FRAME_PING, 0, 0),
        }
    }
}

fn put_header(buf: &mut BytesMut, kind: u8, id: u32, len: usize) {
    buf.put_u8(kind);
    buf.put_u32(id);
    buf.put_u32(len as u32);
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

async fn wait_shutdown(rx: &mut watch::Receiver<bool>) {
    loop {
        if *rx.borrow_and_update() {
            return;
        }
        if rx.changed().await.is_err() {
            return;
        }
    }
}

async fn read_loop<R: AsyncRead + Unpin>(
    link: &Arc<MuxLink>,
    reader: &mut R,
    handler: &LinkHandler,
) -> io::Result<()> {
    let mut header = [0_u8; FRAME_HEADER_LEN];
    loop {
        tokio::time::timeout(LINK_IDLE_TIMEOUT, reader.read_exact(&mut header))
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "tcp carrier link idle timeout"))??;
        let kind = header[0];
        let id = u32::from_be_bytes([header[1], header[2], header[3], header[4]]);
        let len = u32::from_be_bytes([header[5], header[6], header[7], header[8]]) as usize;
        if len > MAX_FRAME_PAYLOAD {
            return Err(invalid("tcp carrier frame too large"));
        }
        let payload = if len == 0 {
            Bytes::new()
        } else {
            let mut payload = BytesMut::zeroed(len);
            tokio::time::timeout(LINK_IDLE_TIMEOUT, reader.read_exact(&mut payload[..]))
                .await
                .map_err(|_| {
                    io::Error::new(io::ErrorKind::TimedOut, "tcp carrier frame body timeout")
                })??;
            payload.freeze()
        };
        link.touch();
        link.handle_frame(kind, id, payload, handler)?;
    }
}

async fn write_loop<W: AsyncWrite + Unpin>(
    link: &MuxLink,
    writer: &mut W,
    out_rx: &mut mpsc::UnboundedReceiver<Outbound>,
) -> io::Result<()> {
    let mut buf = BytesMut::with_capacity(64 * 1024);
    let mut ping = tokio::time::interval(PING_INTERVAL);
    ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        let first = tokio::select! {
            next = out_rx.recv() => match next {
                Some(frame) => frame,
                None => return Ok(()),
            },
            _ = ping.tick() => Outbound::Ping,
        };
        buf.clear();
        link.encode(first, &mut buf);
        while buf.len() < WRITE_BATCH_BYTES {
            match out_rx.try_recv() {
                Ok(frame) => link.encode(frame, &mut buf),
                Err(_) => break,
            }
        }
        writer.write_all(&buf).await?;
        writer.flush().await?;
        if buf.capacity() > 4 * WRITE_BATCH_BYTES {
            buf = BytesMut::with_capacity(64 * 1024);
        }
    }
}

pub struct TcpMuxStream {
    id: u32,
    shared: Arc<StreamShared>,
    link: Arc<MuxLink>,
}

impl AsyncRead for TcpMuxStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        let mut window_update = 0_usize;
        let result = {
            let mut guard = lock(&this.shared.state);
            let state = &mut *guard;
            if !state.recv_buf.is_empty() {
                let mut copied = 0_usize;
                while buf.remaining() > 0 {
                    let Some(front) = state.recv_buf.front_mut() else {
                        break;
                    };
                    let take = front.len().min(buf.remaining());
                    buf.put_slice(&front[..take]);
                    copied += take;
                    if take < front.len() {
                        front.advance(take);
                        break;
                    }
                    state.recv_buf.pop_front();
                }
                state.consumed += copied;
                if state.consumed >= WINDOW_UPDATE_THRESHOLD && !state.recv_fin && !state.peer_reset {
                    window_update = state.consumed;
                    state.recv_window_left += window_update;
                    state.consumed = 0;
                }
                Poll::Ready(Ok(()))
            } else if state.recv_fin {
                Poll::Ready(Ok(()))
            } else if state.peer_reset {
                Poll::Ready(Err(io::Error::new(
                    io::ErrorKind::ConnectionReset,
                    "tcp carrier stream reset by peer",
                )))
            } else if state.link_lost {
                Poll::Ready(Err(io::Error::new(
                    io::ErrorKind::ConnectionAborted,
                    "tcp carrier link lost",
                )))
            } else {
                state.read_waker = Some(cx.waker().clone());
                Poll::Pending
            }
        };
        if window_update > 0 {
            let _ = this.link.send(Outbound::Window {
                id: this.id,
                increment: window_update as u32,
            });
        }
        result
    }
}

impl AsyncWrite for TcpMuxStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        if buf.is_empty() {
            return Poll::Ready(Ok(0));
        }
        let take = {
            let mut state = lock(&this.shared.state);
            if state.peer_reset || state.link_lost || state.fin_sent || this.link.is_closed() {
                return Poll::Ready(Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "tcp carrier stream closed",
                )));
            }
            if state.send_credit == 0 {
                state.write_waker = Some(cx.waker().clone());
                return Poll::Pending;
            }
            let take = buf.len().min(state.send_credit).min(MAX_DATA_FRAME);
            state.send_credit -= take;
            take
        };
        let payload = Bytes::copy_from_slice(&buf[..take]);
        if !this.link.send(Outbound::Data { id: this.id, payload }) {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "tcp carrier link closed",
            )));
        }
        Poll::Ready(Ok(take))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        let send_fin = {
            let mut state = lock(&this.shared.state);
            let send_fin = !state.fin_sent && !state.peer_reset && !state.link_lost;
            state.fin_sent = true;
            send_fin
        };
        if send_fin {
            let _ = this.link.send(Outbound::Fin { id: this.id });
        }
        Poll::Ready(Ok(()))
    }
}

impl Drop for TcpMuxStream {
    fn drop(&mut self) {
        let (send_fin, send_rst) = {
            let state = lock(&self.shared.state);
            let alive = !state.peer_reset && !state.link_lost;
            (alive && !state.fin_sent, alive && !state.recv_fin)
        };
        self.link.remove_stream(self.id);
        // 先 FIN 让对端读到已发数据后正常 EOF；对端仍在发送时再 RST 让其停止。
        if send_fin {
            let _ = self.link.send(Outbound::Fin { id: self.id });
        }
        if send_rst {
            let _ = self.link.send(Outbound::Rst { id: self.id });
        }
    }
}
