use std::{
    io,
    pin::Pin,
    task::{Context, Poll},
};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;

#[cfg(feature = "quic")]
use crate::quic_io::QuicIo;
#[cfg(feature = "tcp")]
use crate::tcp_enc::EncryptedStream;

pub enum CarrierStream {
    #[cfg(feature = "quic")]
    Quic(QuicIo),
    Tcp(TcpStream),
    #[cfg(feature = "tcp")]
    TcpEncrypted(EncryptedStream),
}

impl CarrierStream {
    #[cfg(feature = "quic")]
    pub fn quic(io: QuicIo) -> Self {
        Self::Quic(io)
    }

    pub fn tcp(stream: TcpStream) -> Self {
        Self::Tcp(stream)
    }

    #[cfg(feature = "tcp")]
    pub fn tcp_encrypted(stream: EncryptedStream) -> Self {
        Self::TcpEncrypted(stream)
    }
}

impl AsyncRead for CarrierStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match self.get_mut() {
            #[cfg(feature = "quic")]
            Self::Quic(io) => Pin::new(io).poll_read(cx, buf),
            Self::Tcp(stream) => Pin::new(stream).poll_read(cx, buf),
            #[cfg(feature = "tcp")]
            Self::TcpEncrypted(stream) => Pin::new(stream).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for CarrierStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            #[cfg(feature = "quic")]
            Self::Quic(io) => Pin::new(io).poll_write(cx, buf),
            Self::Tcp(stream) => Pin::new(stream).poll_write(cx, buf),
            #[cfg(feature = "tcp")]
            Self::TcpEncrypted(stream) => Pin::new(stream).poll_write(cx, buf),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            #[cfg(feature = "quic")]
            Self::Quic(io) => Pin::new(io).poll_flush(cx),
            Self::Tcp(stream) => Pin::new(stream).poll_flush(cx),
            #[cfg(feature = "tcp")]
            Self::TcpEncrypted(stream) => Pin::new(stream).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            #[cfg(feature = "quic")]
            Self::Quic(io) => Pin::new(io).poll_shutdown(cx),
            Self::Tcp(stream) => Pin::new(stream).poll_shutdown(cx),
            #[cfg(feature = "tcp")]
            Self::TcpEncrypted(stream) => Pin::new(stream).poll_shutdown(cx),
        }
    }
}
