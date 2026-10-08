use crate::{
    limits::pump_options,
    registry::{BoxFuture, IngressError, IngressFactory},
    types::TunnelEndpoint,
};
#[cfg(feature = "http")]
use crate::http_l7::DedicatedEndpoint;

inventory::submit! {
    IngressFactory {
        kind: "http",
        serve: serve,
    }
}

fn serve(endpoint: TunnelEndpoint) -> BoxFuture<'static, Result<(), IngressError>> {
    Box::pin(async move {
        let listener = match crate::bind::tcp_listener(endpoint.bind_addr.as_str(), endpoint.port)
            .await
        {
            Ok(listener) => listener,
            Err(err) => {
                if let Ok(mut slot) = endpoint.bind_failed.lock() {
                    *slot = Some((endpoint.port, err.to_string()));
                }
                return Err(IngressError::Io(err));
            }
        };
        let budget = tz_net::budget::Budget::new(128 * 1024);
        let pool = tz_net::pool::BufferPool::new(
            std::num::NonZeroUsize::new(32 * 1024).expect("chunk size"),
            std::num::NonZeroUsize::new(4).expect("buffer count"),
            &budget,
        )
        .map_err(|_| IngressError::Stopped)?;
        loop {
            tokio::select! {
                _ = endpoint.shutdown.cancelled() => return Ok(()),
                accepted = listener.accept() => {
                    let (inbound, peer) = accepted?;
                    spawn_http_connection(inbound, peer, endpoint.clone(), pool.clone());
                }
            }
        }
    })
}

fn spawn_http_connection(
    inbound: tokio::net::TcpStream,
    peer: std::net::SocketAddr,
    endpoint: TunnelEndpoint,
    pool: tz_net::pool::BufferPool,
) {
    #[cfg(feature = "http")]
    {
        let dedicated = DedicatedEndpoint {
            tunnel_id: endpoint.tunnel_id,
            client_session: endpoint.carrier_session(),
            http_hosts: endpoint.http_hosts.clone(),
            traffic: endpoint.traffic.clone(),
            speed_limit_mbps: endpoint.speed_limit_mbps,
            max_conns: endpoint.max_conns,
            guard: endpoint.guard.clone(),
            pool: pool.clone(),
            rate_clock: endpoint.rate_clock.clone(),
            filing: endpoint.filing.clone(),
        };
        tokio::spawn(async move {
            if !endpoint.allow_new_conn().await {
                return;
            }
            if endpoint.filing.load().enabled {
                let mut buf = [0_u8; 8];
                let peeked = tokio::time::timeout(
                    std::time::Duration::from_millis(800),
                    inbound.peek(&mut buf),
                )
                .await;
                if let Ok(Ok(n)) = peeked {
                    if crate::http_block::classify_first_packet(&buf[..n])
                        == crate::http_block::FirstPacketKind::Tls
                    {
                        let Ok(acceptor) = crate::filing::block_acceptor() else {
                            return;
                        };
                        let Ok(Ok(tls)) = tokio::time::timeout(
                            std::time::Duration::from_secs(8),
                            acceptor.accept(inbound),
                        )
                        .await
                        else {
                            return;
                        };
                        let _ = crate::http_l7::serve_dedicated_hyper(tls, peer, dedicated).await;
                        return;
                    }
                }
            }
            let _ = crate::http_l7::serve_dedicated_hyper(inbound, peer, dedicated).await;
        });
        return;
    }
    #[cfg(not(feature = "http"))]
    {
        use tz_carrier::types::StreamHeader;
        use tz_guard::types::{ConnCtx, SourceTrust};
        use tz_net::pump::pump;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let guard = endpoint.guard.clone();
        let session = endpoint.carrier_session();
        let tunnel_id = endpoint.tunnel_id;
        tokio::spawn(async move {
            let ctx = ConnCtx {
                peer: peer.ip(),
                trust: SourceTrust::Tcp,
                trusted_proxy: false,
                tunnel_id: Some(endpoint.tunnel_id),
            };
            let verdict = guard.check_conn(&ctx);
            if guard.record_if_denied(&verdict, peer.ip(), Some(endpoint.tunnel_id)) {
                endpoint.traffic.record_reject_guard();
                if let Ok(std_stream) = inbound.into_std() {
                    let socket = socket2::Socket::from(std_stream);
                    let _ = socket.set_linger(Some(std::time::Duration::ZERO));
                }
                return;
            }
            if !endpoint.allow_new_conn().await {
                return;
            }
            let mut inbound = inbound;
            let Some(_conn_slot) = ConnGuard::acquire(&endpoint) else {
                return;
            };
            let header = StreamHeader {
                tunnel_id,
                src_addr: peer,
            };
            let Ok(mut carrier) = session.open_stream(header).await else {
                return;
            };
            let mut prefix = vec![0_u8; 64 * 1024];
            let mut total = 0_usize;
            while total < prefix.len() {
                let read = match inbound.read(&mut prefix[total..]).await {
                    Ok(read) => read,
                    Err(_) => return,
                };
                if read == 0 {
                    return;
                }
                total += read;
                if prefix[..total].windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
            total = crate::http_headers::inject_forwarded_headers(&mut prefix, total, peer);
            if carrier.write_all(&prefix[..total]).await.is_err() {
                return;
            }
            let relay = pump(
                inbound,
                carrier,
                pool,
                pump_options(
                    &endpoint.rate_clock,
                    endpoint.speed_limit_mbps,
                    endpoint.traffic.meter.clone(),
                ),
            );
            tokio::select! {
                _ = relay => {}
                _ = endpoint.traffic.connections_closed() => {}
            }
        });
    }
}

#[cfg(not(feature = "http"))]
struct ConnGuard(TunnelEndpoint);

#[cfg(not(feature = "http"))]
impl ConnGuard {
    fn acquire(endpoint: &TunnelEndpoint) -> Option<Self> {
        endpoint
            .try_acquire_conn()
            .then(|| Self(endpoint.clone()))
    }
}

#[cfg(not(feature = "http"))]
impl Drop for ConnGuard {
    fn drop(&mut self) {
        self.0.release_conn();
    }
}
