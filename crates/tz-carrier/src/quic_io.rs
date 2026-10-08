use quinn::{RecvStream, SendStream};
use std::{
    pin::Pin,
    task::{Context, Poll},
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, ReadBuf};

use crate::types::{decode_carrier_bind, encode_carrier_bind, CARRIER_BIND_LEN};

pub struct QuicIo {
    send: SendStream,
    recv: RecvStream,
}

impl QuicIo {
    pub fn new(send: SendStream, recv: RecvStream) -> Self {
        Self { send, recv }
    }
}

impl AsyncRead for QuicIo {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.recv).poll_read(cx, buf)
    }
}

impl AsyncWrite for QuicIo {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        match Pin::new(&mut self.send).poll_write(cx, buf) {
            Poll::Ready(Ok(written)) => Poll::Ready(Ok(written)),
            Poll::Ready(Err(err)) => Poll::Ready(Err(err.into())),
            Poll::Pending => Poll::Pending,
        }
    }

    fn poll_flush(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.send)
            .poll_flush(cx)
            .map_err(std::io::Error::other)
    }

    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.send)
            .poll_shutdown(cx)
            .map_err(std::io::Error::other)
    }
}

pub async fn write_carrier_bind(
    connection: &quinn::Connection,
    tunnel_id: uuid::Uuid,
    client_id: uuid::Uuid,
) -> std::io::Result<()> {
    let (mut send, mut recv) = connection.open_bi().await.map_err(std::io::Error::other)?;
    send.write_all(&encode_carrier_bind(tunnel_id, client_id))
        .await
        .map_err(std::io::Error::other)?;
    send.finish().map_err(std::io::Error::other)?;
    let _ = recv.read_to_end(64).await;
    Ok(())
}

pub async fn read_carrier_bind(
    connection: &quinn::Connection,
) -> std::io::Result<(uuid::Uuid, uuid::Uuid)> {
    let (_send, mut recv) = connection.accept_bi().await.map_err(std::io::Error::other)?;
    let mut bind = [0_u8; CARRIER_BIND_LEN];
    recv.read_exact(&mut bind).await.map_err(std::io::Error::other)?;
    decode_carrier_bind(&bind).ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid carrier bind preamble")
    })
}
