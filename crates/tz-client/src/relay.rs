use std::{
    convert::Infallible,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::Duration,
};

use bytes::Bytes;
use http_body_util::{combinators::BoxBody, BodyExt, Full};
use hyper::{
    body::Incoming,
    client::conn::http1::SendRequest,
    header::{self, HeaderMap, HeaderName},
    Request, Response, StatusCode,
};
use hyper_util::rt::TokioIo;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use url::Url;

use crate::upstream::{
    connect_upstream, rewrite_location, rewrite_request, upstream_target, Upstream,
};
use tz_net::pump::{pump, PumpOptions};
use tz_net::{budget::Budget, meter::TrafficMeter, pool::BufferPool};
use tz_proto::ClientTunnelAssign;

type BoxError = Box<dyn std::error::Error + Send + Sync>;
type Body = BoxBody<Bytes, BoxError>;

/// 复用上游连接前等待其空闲的上限；超时视为连接已坏，改用新连接。
const UPSTREAM_READY_TIMEOUT: Duration = Duration::from_secs(10);

pub async fn relay_connection<C>(
    carrier: C,
    tunnel: &ClientTunnelAssign,
    upstream: Upstream,
    meter: TrafficMeter,
) -> anyhow::Result<()>
where
    C: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    if tunnel.protocol == "http" {
        if let Some(target) = tunnel.target_url.as_deref() {
            let target = Url::parse(target)?;
            return relay_http(carrier, tunnel.clone(), target, upstream, meter).await;
        }
    }
    let budget = Budget::new(128 * 1024);
    let pool = BufferPool::new(
        std::num::NonZeroUsize::new(32 * 1024).expect("chunk"),
        std::num::NonZeroUsize::new(4).expect("count"),
        &budget,
    )?;
    pump(
        carrier,
        upstream,
        pool,
        PumpOptions { left_to_right: None, right_to_left: None, meter },
    )
    .await?;
    Ok(())
}

/// 一条 carrier 流承载访客一条 keep-alive 连接上的多个请求：逐个请求改写 Host / 路径，
/// 并复用到上游的 HTTP/1 连接，避免每个请求都新建上游 TCP/TLS。
async fn relay_http<C>(
    carrier: C,
    tunnel: ClientTunnelAssign,
    target: Url,
    upstream: Upstream,
    meter: TrafficMeter,
) -> anyhow::Result<()>
where
    C: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let state = Arc::new(HttpRelayState {
        host_rewrite: tunnel
            .host_rewrite
            .clone()
            .unwrap_or_else(|| "$http_host".into()),
        tunnel,
        target,
        slot: tokio::sync::Mutex::new(UpstreamSlot {
            initial: Some(upstream),
            sender: None,
        }),
    });
    let io = TokioIo::new(MeteredIo { inner: carrier, meter });
    let service = hyper::service::service_fn(move |request| {
        let state = state.clone();
        async move { Ok::<_, Infallible>(state.handle(request).await) }
    });
    hyper::server::conn::http1::Builder::new()
        .serve_connection(io, service)
        .with_upgrades()
        .await
        .or_else(|error| {
            if error.is_incomplete_message() || error.is_closed() || error.is_canceled() {
                Ok(())
            } else {
                Err(anyhow::anyhow!("carrier http connection: {error}"))
            }
        })
}

struct UpstreamSlot {
    /// runtime 预先建立的上游连接（用于及时上报上游不可达）。
    initial: Option<Upstream>,
    sender: Option<SendRequest<Incoming>>,
}

struct HttpRelayState {
    tunnel: ClientTunnelAssign,
    target: Url,
    host_rewrite: String,
    slot: tokio::sync::Mutex<UpstreamSlot>,
}

impl HttpRelayState {
    async fn handle(&self, mut request: Request<Incoming>) -> Response<Body> {
        let tunnel_id = self.tunnel.tunnel_id;
        if let Err(error) = rewrite_request(&mut request, &self.host_rewrite, &self.target) {
            tracing::warn!(%tunnel_id, error = %format!("{error:#}"), "upstream request rewrite failed");
            return text_response(StatusCode::BAD_REQUEST, "invalid upstream request");
        }
        let wants_upgrade = request_wants_upgrade(request.headers());
        let client_upgrade = wants_upgrade.then(|| hyper::upgrade::on(&mut request));

        let mut sender = match self.take_sender().await {
            Ok(sender) => sender,
            Err(error) => return self.upstream_unavailable(error),
        };
        let mut response = match sender.try_send_request(request).await {
            Ok(response) => response,
            Err(mut error) => match error.take_message() {
                // 复用的连接恰好被上游关闭、请求未发出：换新连接重发一次。
                Some(request) => {
                    sender = match self.connect_sender().await {
                        Ok(sender) => sender,
                        Err(error) => return self.upstream_unavailable(error),
                    };
                    match sender.send_request(request).await {
                        Ok(response) => response,
                        Err(error) => {
                            tracing::warn!(%tunnel_id, %error, "upstream request failed");
                            return text_response(StatusCode::BAD_GATEWAY, "upstream request failed");
                        }
                    }
                }
                None => {
                    tracing::warn!(%tunnel_id, error = ?error.into_error(), "upstream request failed");
                    return text_response(StatusCode::BAD_GATEWAY, "upstream request failed");
                }
            },
        };

        let switching = response.status() == StatusCode::SWITCHING_PROTOCOLS;
        if switching {
            if let Some(client_upgrade) = client_upgrade {
                let upstream_upgrade = hyper::upgrade::on(&mut response);
                tokio::spawn(async move {
                    let (Ok(client), Ok(upstream)) = tokio::join!(client_upgrade, upstream_upgrade)
                    else {
                        return;
                    };
                    let mut client = TokioIo::new(client);
                    let mut upstream = TokioIo::new(upstream);
                    let _ = tokio::io::copy_bidirectional(&mut client, &mut upstream).await;
                });
            }
        } else {
            self.slot.lock().await.sender = Some(sender);
            // 上游要求关闭时由 sender 自行失效；carrier 流保持 keep-alive，下个请求换新上游连接。
            strip_connection_headers(response.headers_mut());
            let rewritten = response
                .headers()
                .get(header::LOCATION)
                .and_then(|value| value.to_str().ok())
                .and_then(|location| rewrite_location(location, &self.target))
                .and_then(|location| header::HeaderValue::from_str(&location).ok());
            if let Some(location) = rewritten {
                response.headers_mut().insert(header::LOCATION, location);
            }
        }
        response.map(|body| body.map_err(|error| Box::new(error) as BoxError).boxed())
    }

    async fn take_sender(&self) -> anyhow::Result<SendRequest<Incoming>> {
        let (initial, reused) = {
            let mut slot = self.slot.lock().await;
            (slot.initial.take(), slot.sender.take())
        };
        if let Some(mut sender) = reused {
            if matches!(
                tokio::time::timeout(UPSTREAM_READY_TIMEOUT, sender.ready()).await,
                Ok(Ok(()))
            ) {
                return Ok(sender);
            }
        }
        match initial {
            Some(upstream) => handshake(upstream, self.tunnel.tunnel_id).await,
            None => self.connect_sender().await,
        }
    }

    async fn connect_sender(&self) -> anyhow::Result<SendRequest<Incoming>> {
        let upstream = connect_upstream(&self.tunnel).await?;
        handshake(upstream, self.tunnel.tunnel_id).await
    }

    fn upstream_unavailable(&self, error: anyhow::Error) -> Response<Body> {
        tracing::warn!(
            tunnel_id = %self.tunnel.tunnel_id,
            target = %upstream_target(&self.tunnel),
            error = %format!("{error:#}"),
            "upstream connection failed"
        );
        text_response(StatusCode::BAD_GATEWAY, "upstream unavailable")
    }
}

async fn handshake(upstream: Upstream, tunnel_id: uuid::Uuid) -> anyhow::Result<SendRequest<Incoming>> {
    let (sender, connection) = hyper::client::conn::http1::Builder::new()
        .handshake(TokioIo::new(upstream))
        .await
        .map_err(|error| anyhow::anyhow!("upstream http handshake: {error}"))?;
    tokio::spawn(async move {
        if let Err(error) = connection.with_upgrades().await {
            tracing::debug!(%tunnel_id, %error, "upstream http connection ended");
        }
    });
    Ok(sender)
}

fn request_wants_upgrade(headers: &HeaderMap) -> bool {
    let connection_upgrade = headers
        .get_all(header::CONNECTION)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .any(|value| value.split(',').any(|token| token.trim().eq_ignore_ascii_case("upgrade")));
    connection_upgrade && headers.contains_key(header::UPGRADE)
}

fn strip_connection_headers(headers: &mut HeaderMap) {
    let listed: Vec<HeaderName> = headers
        .get_all(header::CONNECTION)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .filter_map(|name| HeaderName::from_bytes(name.trim().as_bytes()).ok())
        .collect();
    for name in listed {
        headers.remove(name);
    }
    headers.remove(header::CONNECTION);
    headers.remove("keep-alive");
}

fn text_response(status: StatusCode, message: &str) -> Response<Body> {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(
            Full::new(Bytes::from(format!("{message}\n")))
                .map_err(|never| -> BoxError { match never {} })
                .boxed(),
        )
        .expect("response")
}

/// carrier 方向的字节计量：读入计 left_to_right（访客→上游），写出计 right_to_left。
struct MeteredIo<C> {
    inner: C,
    meter: TrafficMeter,
}

impl<C: AsyncRead + Unpin> AsyncRead for MeteredIo<C> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let before = buf.filled().len();
        let result = Pin::new(&mut self.inner).poll_read(cx, buf);
        if let Poll::Ready(Ok(())) = &result {
            self.meter.left_to_right.add(buf.filled().len() - before);
        }
        result
    }
}

impl<C: AsyncWrite + Unpin> AsyncWrite for MeteredIo<C> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        let result = Pin::new(&mut self.inner).poll_write(cx, buf);
        if let Poll::Ready(Ok(written)) = &result {
            self.meter.right_to_left.add(*written);
        }
        result
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn metered_io_counts_both_directions() {
        let (client, server) = tokio::io::duplex(1024);
        let meter = TrafficMeter::default();
        let mut metered = MeteredIo { inner: server, meter: meter.clone() };
        let mut client = client;
        client.write_all(b"hello").await.unwrap();
        let mut buf = [0_u8; 5];
        metered.read_exact(&mut buf).await.unwrap();
        metered.write_all(b"abc").await.unwrap();
        let snapshot = meter.snapshot();
        assert_eq!(snapshot.left_to_right, 5);
        assert_eq!(snapshot.right_to_left, 3);
    }

    #[test]
    fn connection_headers_are_stripped() {
        let mut headers = HeaderMap::new();
        headers.insert(header::CONNECTION, "close, x-custom".parse().unwrap());
        headers.insert("x-custom", "1".parse().unwrap());
        headers.insert("keep-alive", "timeout=5".parse().unwrap());
        strip_connection_headers(&mut headers);
        assert!(headers.is_empty());
    }
}
