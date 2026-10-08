use std::pin::Pin;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};
use url::Url;

use crate::upstream::{prepend_base_path, rewrite_host_header, Upstream};
use tz_net::pump::{pump, PumpOptions};
use tz_net::{budget::Budget, pool::BufferPool};
use tz_proto::ClientTunnelAssign;

pub async fn relay_connection<C>(
    mut carrier: C,
    tunnel: &ClientTunnelAssign,
    upstream: Upstream,
    meter: tz_net::meter::TrafficMeter,
) -> anyhow::Result<()>
where
    C: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let budget = Budget::new(128 * 1024);
    let pool = BufferPool::new(
        std::num::NonZeroUsize::new(32 * 1024).expect("chunk"),
        std::num::NonZeroUsize::new(4).expect("count"),
        &budget,
    )?;

    let needs_host_rewrite = tunnel.protocol == "http"
        && tunnel
            .host_rewrite
            .as_ref()
            .is_some_and(|value| !value.is_empty() && value != "$http_host")
        && tunnel.target_url.is_some();

    if !needs_host_rewrite {
        pump(
            carrier,
            upstream,
            pool,
            PumpOptions {
                left_to_right: None,
                right_to_left: None,
                meter: meter.clone(),
            },
        )
        .await?;
        return Ok(());
    }

    let host_rewrite = tunnel.host_rewrite.clone().unwrap_or_default();
    let target_url = Url::parse(tunnel.target_url.as_ref().expect("target_url"))?;
    let mut prefix = vec![0_u8; 64 * 1024];
    let mut total = 0_usize;
    loop {
        let read = carrier
            .read(&mut prefix[total..])
            .await
            .map_err(|err| anyhow::anyhow!(err))?;
        if read == 0 {
            return Ok(());
        }
        total += read;
        if prefix[..total].windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
        if total >= prefix.len() {
            break;
        }
    }
    let _ = rewrite_host_header(&mut prefix[..total], &host_rewrite, &target_url);
    total = prepend_base_path(&mut prefix, total, &target_url);

    let (mut upstream_read, mut upstream_write) = split_upstream(upstream);
    upstream_write
        .write_all(&prefix[..total])
        .await
        .map_err(|err| anyhow::anyhow!(err))?;
    upstream_write
        .flush()
        .await
        .map_err(|err| anyhow::anyhow!(err))?;

    let prefixed_carrier = PrefixedReader {
        prefix: prefix[..total].to_vec(),
        offset: 0,
        inner: carrier,
    };

    pump(
        prefixed_carrier,
        UpstreamIoPair {
            read: upstream_read,
            write: upstream_write,
        },
        pool,
        PumpOptions {
            left_to_right: None,
            right_to_left: None,
            meter,
        },
    )
    .await?;
    Ok(())
}

struct PrefixedReader<C> {
    prefix: Vec<u8>,
    offset: usize,
    inner: C,
}

impl<C: AsyncRead + AsyncWrite + Unpin> AsyncWrite for PrefixedReader<C> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }

    fn poll_flush(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

impl<C: AsyncRead + Unpin> AsyncRead for PrefixedReader<C> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        if self.offset < self.prefix.len() {
            let remaining = &self.prefix[self.offset..];
            let take = remaining.len().min(buf.remaining());
            buf.put_slice(&remaining[..take]);
            self.offset += take;
            return std::task::Poll::Ready(Ok(()));
        }
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

fn split_upstream(upstream: Upstream) -> (UpstreamReadHalf, UpstreamWriteHalf) {
    match upstream {
        Upstream::Plain(stream) => {
            let (read, write) = stream.into_split();
            (
                UpstreamReadHalf::Plain(read),
                UpstreamWriteHalf::Plain(write),
            )
        }
        Upstream::Tls(stream) => {
            let (read, write) = tokio::io::split(stream);
            (
                UpstreamReadHalf::Tls(read),
                UpstreamWriteHalf::Tls(write),
            )
        }
    }
}

enum UpstreamReadHalf {
    Plain(tokio::net::tcp::OwnedReadHalf),
    Tls(tokio::io::ReadHalf<tokio_rustls::client::TlsStream<tokio::net::TcpStream>>),
}

enum UpstreamWriteHalf {
    Plain(tokio::net::tcp::OwnedWriteHalf),
    Tls(tokio::io::WriteHalf<tokio_rustls::client::TlsStream<tokio::net::TcpStream>>),
}

impl AsyncRead for UpstreamReadHalf {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match &mut *self {
            Self::Plain(stream) => Pin::new(stream).poll_read(cx, buf),
            Self::Tls(stream) => Pin::new(stream).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for UpstreamWriteHalf {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        match &mut *self {
            Self::Plain(stream) => Pin::new(stream).poll_write(cx, buf),
            Self::Tls(stream) => Pin::new(stream).poll_write(cx, buf),
        }
    }

    fn poll_flush(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match &mut *self {
            Self::Plain(stream) => Pin::new(stream).poll_flush(cx),
            Self::Tls(stream) => Pin::new(stream).poll_flush(cx),
        }
    }

    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match &mut *self {
            Self::Plain(stream) => Pin::new(stream).poll_shutdown(cx),
            Self::Tls(stream) => Pin::new(stream).poll_shutdown(cx),
        }
    }
}

struct UpstreamIoPair {
    read: UpstreamReadHalf,
    write: UpstreamWriteHalf,
}

impl AsyncRead for UpstreamIoPair {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        Pin::new(&mut self.read).poll_read(cx, buf)
    }
}

impl AsyncWrite for UpstreamIoPair {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        Pin::new(&mut self.write).poll_write(cx, buf)
    }

    fn poll_flush(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        Pin::new(&mut self.write).poll_flush(cx)
    }

    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        Pin::new(&mut self.write).poll_shutdown(cx)
    }
}
