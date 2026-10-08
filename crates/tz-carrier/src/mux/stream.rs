use super::session::StreamCommand;
use super::{MAX_MUX_PAYLOAD, MUX_COMMAND_QUEUE};
use bytes::Bytes;
use std::{
    io,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll, Waker},
};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::{mpsc, Notify};

pub struct MuxStream {
    stream_id: u32,
    command_tx: mpsc::Sender<StreamCommand>,
    read_rx: mpsc::Receiver<Bytes>,
    read_buffer: Option<Bytes>,
    write_notify: Arc<Notify>,
    read_drain: Arc<Notify>,
    write_waker: Option<Waker>,
}

impl MuxStream {
    pub(crate) fn new(
        stream_id: u32,
        command_tx: mpsc::Sender<StreamCommand>,
        read_rx: mpsc::Receiver<Bytes>,
        write_notify: Arc<Notify>,
        read_drain: Arc<Notify>,
    ) -> Self {
        Self {
            stream_id,
            command_tx,
            read_rx,
            read_buffer: None,
            write_notify,
            read_drain,
            write_waker: None,
        }
    }

    pub fn stream_id(&self) -> u32 {
        self.stream_id
    }
}

impl AsyncRead for MuxStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        loop {
            if let Some(chunk) = self.read_buffer.as_ref() {
                if !chunk.is_empty() {
                    let amount = chunk.len().min(buf.remaining());
                    buf.put_slice(&chunk[..amount]);
                    if amount == chunk.len() {
                        self.read_buffer = None;
                        self.read_drain.notify_one();
                    } else {
                        self.read_buffer = Some(chunk.slice(amount..));
                    }
                    return Poll::Ready(Ok(()));
                }
                self.read_buffer = None;
            }
            match self.read_rx.poll_recv(cx) {
                Poll::Ready(Some(chunk)) => {
                    self.read_drain.notify_one();
                    self.read_buffer = Some(chunk);
                }
                Poll::Ready(None) => {
                    self.read_drain.notify_one();
                    return Poll::Ready(Ok(()));
                }
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

impl AsyncWrite for MuxStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        if buf.is_empty() {
            return Poll::Ready(Ok(0));
        }
        let chunk_len = buf.len().min(MAX_MUX_PAYLOAD);
        let payload = Bytes::copy_from_slice(&buf[..chunk_len]);
        match self.command_tx.try_send(StreamCommand::Data {
            stream_id: self.stream_id,
            payload,
        }) {
            Ok(()) => Poll::Ready(Ok(chunk_len)),
            Err(mpsc::error::TrySendError::Full(_)) => {
                self.write_waker = Some(cx.waker().clone());
                let mut notified = std::pin::pin!(self.write_notify.notified());
                match notified.as_mut().poll(cx) {
                    Poll::Ready(()) => {
                        cx.waker().wake_by_ref();
                    }
                    Poll::Pending => {}
                }
                Poll::Pending
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                Poll::Ready(Err(io::ErrorKind::BrokenPipe.into()))
            }
        }
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        let _ = self.command_tx.try_send(StreamCommand::Fin {
            stream_id: self.stream_id,
        });
        Poll::Ready(Ok(()))
    }
}

impl Drop for MuxStream {
    fn drop(&mut self) {
        let _ = self.command_tx.try_send(StreamCommand::Fin {
            stream_id: self.stream_id,
        });
    }
}

impl Unpin for MuxStream {}

pub(crate) fn command_channel() -> (
    mpsc::Sender<StreamCommand>,
    mpsc::Receiver<StreamCommand>,
) {
    mpsc::channel(MUX_COMMAND_QUEUE)
}
