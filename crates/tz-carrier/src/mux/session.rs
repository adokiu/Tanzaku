use super::frame::{decode_open, encode_open, FrameType, MuxFrame};
use super::stream;
use super::{
    MAX_MUX_PAYLOAD, MUX_ACCEPT_QUEUE, MUX_DATAGRAM_QUEUE, MUX_DECODE_BUFFER, MUX_IO_SCRATCH,
    MUX_STREAM_READ_QUEUE,
};
use crate::types::{DatagramDropped, FlowId, OpenError, StreamHeader};
use bytes::{Bytes, BytesMut};
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{mpsc, Mutex, Notify};
pub enum Role {
    Client,
    Server,
}

pub(crate) enum StreamCommand {
    Open {
        stream_id: u32,
        header: StreamHeader,
    },
    Data {
        stream_id: u32,
        payload: Bytes,
    },
    Fin {
        stream_id: u32,
    },
    Dgram {
        flow: FlowId,
        payload: Bytes,
    },
}

struct StreamSlot {
    read_tx: mpsc::Sender<Bytes>,
}

struct PendingAccept {
    stream_id: u32,
    header: StreamHeader,
    read_rx: mpsc::Receiver<Bytes>,
}

pub struct MuxSession {
    role: Role,
    next_stream_id: AtomicU64,
    command_tx: mpsc::Sender<StreamCommand>,
    streams: Arc<Mutex<HashMap<u32, StreamSlot>>>,
    accept_tx: mpsc::Sender<PendingAccept>,
    accept_rx: Mutex<mpsc::Receiver<PendingAccept>>,
    datagram_tx: mpsc::Sender<(FlowId, Bytes)>,
    datagram_rx: Mutex<mpsc::Receiver<(FlowId, Bytes)>>,
    closed: Notify,
    is_closed: AtomicBool,
    shutdown: Notify,
    write_notify: Arc<Notify>,
    read_drain: Arc<Notify>,
    deferred_read: Mutex<Option<(u32, Bytes)>>,
    read_paused: AtomicBool,
}

impl MuxSession {
    pub fn spawn_io<T>(role: Role, io: T) -> Arc<Self>
    where
        T: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let (command_tx, command_rx) = stream::command_channel();
        let (datagram_tx, datagram_rx) = mpsc::channel(MUX_DATAGRAM_QUEUE);
        let (accept_tx, accept_rx) = mpsc::channel(MUX_ACCEPT_QUEUE);
        let write_notify = Arc::new(Notify::new());
        let read_drain = Arc::new(Notify::new());
        let session = Arc::new(Self {
            role,
            next_stream_id: AtomicU64::new(1),
            command_tx,
            streams: Arc::new(Mutex::new(HashMap::new())),
            accept_tx,
            accept_rx: Mutex::new(accept_rx),
            datagram_tx,
            datagram_rx: Mutex::new(datagram_rx),
            closed: Notify::new(),
            is_closed: AtomicBool::new(false),
            shutdown: Notify::new(),
            write_notify: write_notify.clone(),
            read_drain: read_drain.clone(),
            deferred_read: Mutex::new(None),
            read_paused: AtomicBool::new(false),
        });
        let worker = session.clone();
        tokio::spawn(async move {
            let (reader, writer) = tokio::io::split(io);
            let read_worker = worker.clone();
            let write_worker = worker.clone();
            let read_task = tokio::spawn(async move {
                if let Err(error) = read_worker.read_loop(reader).await {
                    tracing::warn!(?error, "mux read half stopped");
                }
            });
            let write_task = tokio::spawn(async move {
                if let Err(error) = write_worker.write_loop(writer, command_rx).await {
                    tracing::warn!(?error, "mux write half stopped");
                }
            });
            let _ = tokio::join!(read_task, write_task);
            worker.is_closed.store(true, Ordering::Release);
            worker.streams.lock().await.clear();
            worker.closed.notify_waiters();
        });
        session
    }

    pub fn is_closed(&self) -> bool {
        self.is_closed.load(Ordering::Acquire)
    }

    pub fn close(&self) {
        self.is_closed.store(true, Ordering::Release);
        self.shutdown.notify_one();
        self.closed.notify_waiters();
    }

    pub async fn open_stream(&self, header: StreamHeader) -> Result<super::stream::MuxStream, OpenError> {
        if self.is_closed() {
            return Err(OpenError::Closed);
        }
        let stream_id = self
            .next_stream_id
            .fetch_add(1, Ordering::Relaxed)
            .try_into()
            .map_err(|_| OpenError::BudgetExceeded)?;
        let (read_tx, read_rx) = mpsc::channel(MUX_STREAM_READ_QUEUE);
        self.streams.lock().await.insert(stream_id, StreamSlot { read_tx });
        self.command_tx
            .send(StreamCommand::Open { stream_id, header })
            .await
            .map_err(|_| OpenError::Closed)?;
        Ok(super::stream::MuxStream::new(
            stream_id,
            self.command_tx.clone(),
            read_rx,
            self.write_notify.clone(),
            self.read_drain.clone(),
        ))
    }

    pub async fn accept_stream(&self) -> Result<(StreamHeader, super::stream::MuxStream), OpenError> {
        let mut rx = self.accept_rx.lock().await;
        let closed = self.closed.notified();
        tokio::pin!(closed);
        closed.as_mut().enable();
        if self.is_closed() {
            return Err(OpenError::Closed);
        }
        let pending = tokio::select! {
            pending = rx.recv() => pending.ok_or(OpenError::Closed)?,
            _ = closed => return Err(OpenError::Closed),
        };
        Ok((
            pending.header,
            super::stream::MuxStream::new(
                pending.stream_id,
                self.command_tx.clone(),
                pending.read_rx,
                self.write_notify.clone(),
                self.read_drain.clone(),
            ),
        ))
    }

    pub fn send_datagram(&self, flow: FlowId, data: Bytes) -> Result<(), DatagramDropped> {
        if self.is_closed() {
            return Err(DatagramDropped::Closed);
        }
        if data.len() > MAX_MUX_PAYLOAD {
            return Err(DatagramDropped::QueueFull);
        }
        self.command_tx
            .try_send(StreamCommand::Dgram { flow, payload: data })
            .map_err(|error| match error {
                mpsc::error::TrySendError::Full(_) => DatagramDropped::QueueFull,
                mpsc::error::TrySendError::Closed(_) => DatagramDropped::Closed,
            })
    }

    pub async fn recv_datagram(&self) -> Result<(FlowId, Bytes), OpenError> {
        let mut rx = self.datagram_rx.lock().await;
        let closed = self.closed.notified();
        tokio::pin!(closed);
        closed.as_mut().enable();
        if self.is_closed() {
            return Err(OpenError::Closed);
        }
        tokio::select! {
            item = rx.recv() => item.ok_or(OpenError::Closed),
            _ = closed => Err(OpenError::Closed),
        }
    }

    async fn write_loop<T>(
        self: Arc<Self>,
        mut io: T,
        mut command_rx: mpsc::Receiver<StreamCommand>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>>
    where
        T: AsyncWrite + Unpin,
    {
        loop {
            tokio::select! {
                _ = self.shutdown.notified() => break,
                command = command_rx.recv() => {
                    let Some(command) = command else { break; };
                    self.write_command(&mut io, command).await?;
                    self.drain_commands(&mut io, &mut command_rx).await?;
                }
            }
        }
        self.shutdown.notify_one();
        Ok(())
    }

    async fn read_loop<T>(
        self: Arc<Self>,
        mut io: T,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>>
    where
        T: AsyncRead + Unpin,
    {
        let mut read_buf = BytesMut::with_capacity(MUX_IO_SCRATCH);
        let mut scratch = [0u8; MUX_IO_SCRATCH];
        loop {
            self.flush_deferred_read().await;
            self.decode_read_buf(&mut read_buf).await;
            let can_read_io = !self.read_paused.load(Ordering::Acquire);
            tokio::select! {
                _ = self.shutdown.notified() => break,
                read = io.read(&mut scratch), if can_read_io => {
                    let amount = read?;
                    if amount == 0 {
                        break;
                    }
                    read_buf.extend_from_slice(&scratch[..amount]);
                    if read_buf.len() > MUX_DECODE_BUFFER {
                        return Err("mux decode buffer exceeded".into());
                    }
                    self.decode_read_buf(&mut read_buf).await;
                }
                _ = self.read_drain.notified(), if self.read_paused.load(Ordering::Acquire) => {}
            }
        }
        self.shutdown.notify_one();
        Ok(())
    }

    async fn decode_read_buf(&self, read_buf: &mut BytesMut) {
        loop {
            if self.read_paused.load(Ordering::Acquire) {
                break;
            }
            let frame = match MuxFrame::decode(read_buf) {
                Ok(Some(frame)) => frame,
                Ok(None) => break,
                Err(error) => {
                    tracing::warn!(?error, "invalid mux frame");
                    break;
                }
            };
            if self.handle_frame(frame).await.is_err() {
                break;
            }
        }
    }

    async fn flush_deferred_read(&self) {
        loop {
            let next = {
                let mut guard = self.deferred_read.lock().await;
                guard.take()
            };
            let Some((stream_id, payload)) = next else {
                self.read_paused.store(false, Ordering::Release);
                return;
            };
            let read_tx = {
                let streams = self.streams.lock().await;
                streams
                    .get(&stream_id)
                    .map(|slot| slot.read_tx.clone())
            };
            let Some(read_tx) = read_tx else {
                continue;
            };
            match read_tx.try_send(payload) {
                Ok(()) => {}
                Err(mpsc::error::TrySendError::Full(payload)) => {
                    *self.deferred_read.lock().await = Some((stream_id, payload));
                    self.read_paused.store(true, Ordering::Release);
                    return;
                }
                Err(mpsc::error::TrySendError::Closed(_)) => {}
            }
        }
    }

    async fn write_command<T>(
        &self,
        io: &mut T,
        command: StreamCommand,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>>
    where
        T: AsyncWrite + Unpin,
    {
        let frame = self.encode_command(command);
        let mut encoded = BytesMut::with_capacity(MuxFrame::HEADER_LEN + frame.payload.len());
        frame.encode(&mut encoded);
        io.write_all(&encoded).await?;
        self.write_notify.notify_waiters();
        Ok(())
    }

    async fn drain_commands<T>(
        &self,
        io: &mut T,
        command_rx: &mut mpsc::Receiver<StreamCommand>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>>
    where
        T: AsyncWrite + Unpin,
    {
        while let Ok(command) = command_rx.try_recv() {
            self.write_command(io, command).await?;
        }
        self.write_notify.notify_waiters();
        Ok(())
    }

    async fn handle_frame(&self, frame: MuxFrame) -> Result<(), OpenError> {
        match frame.frame_type {
            FrameType::Open => {
                let (tunnel_id, src_addr) =
                    decode_open(&frame.payload).map_err(|_| OpenError::Rejected)?;
                let (read_tx, read_rx) = mpsc::channel(MUX_STREAM_READ_QUEUE);
                self.streams.lock().await.insert(
                    frame.stream_id,
                    StreamSlot { read_tx },
                );
                self.accept_tx
                    .try_send(PendingAccept {
                        stream_id: frame.stream_id,
                        header: StreamHeader {
                            tunnel_id,
                            src_addr,
                        },
                        read_rx,
                    })
                    .map_err(|_| OpenError::Rejected)?;
            }
            FrameType::Data => {
                if frame.payload.len() > MAX_MUX_PAYLOAD {
                    return Err(OpenError::Rejected);
                }
                let read_tx = {
                    let streams = self.streams.lock().await;
                    streams
                        .get(&frame.stream_id)
                        .map(|slot| slot.read_tx.clone())
                };
                if let Some(read_tx) = read_tx {
                    match read_tx.try_send(frame.payload) {
                        Ok(()) => {}
                        Err(mpsc::error::TrySendError::Full(payload)) => {
                            *self.deferred_read.lock().await = Some((frame.stream_id, payload));
                            self.read_paused.store(true, Ordering::Release);
                        }
                        Err(mpsc::error::TrySendError::Closed(_)) => {
                            return Err(OpenError::Closed);
                        }
                    }
                }
            }
            FrameType::Fin | FrameType::Rst => {
                self.streams.lock().await.remove(&frame.stream_id);
            }
            FrameType::Dgram => {
                if frame.payload.len() > MAX_MUX_PAYLOAD.saturating_add(8) {
                    return Err(OpenError::Rejected);
                }
                if frame.payload.len() >= 8 {
                    let flow = FlowId(u64::from_be_bytes(
                        frame.payload[0..8].try_into().unwrap_or([0; 8]),
                    ));
                    let _ = self.datagram_tx.try_send((flow, frame.payload.slice(8..)));
                }
            }
            FrameType::Ping | FrameType::Window => {}
        }
        Ok(())
    }

    fn encode_command(&self, command: StreamCommand) -> MuxFrame {
        match command {
            StreamCommand::Open { stream_id, header } => {
                encode_open(stream_id, header.tunnel_id, header.src_addr)
            }
            StreamCommand::Data { stream_id, payload } => MuxFrame {
                frame_type: FrameType::Data,
                flags: 0,
                stream_id,
                payload,
            },
            StreamCommand::Fin { stream_id } => MuxFrame {
                frame_type: FrameType::Fin,
                flags: 0,
                stream_id,
                payload: Bytes::new(),
            },
            StreamCommand::Dgram { flow, payload } => {
                let mut merged = BytesMut::with_capacity(8 + payload.len());
                merged.extend_from_slice(&flow.0.to_be_bytes());
                merged.extend_from_slice(&payload);
                MuxFrame {
                    frame_type: FrameType::Dgram,
                    flags: 0,
                    stream_id: 0,
                    payload: merged.freeze(),
                }
            }
        }
    }
}
