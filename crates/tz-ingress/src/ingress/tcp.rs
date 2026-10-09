use crate::{
    http_block,
    limits::pump_options,
    registry::{BoxFuture, IngressError, IngressFactory},
    types::TunnelEndpoint,
};
use tz_carrier::types::StreamHeader;
use tz_guard::types::{ConnCtx, Reason, SourceTrust};
use tz_net::{pool::BufferPool, pump::pump};

inventory::submit! {
    IngressFactory {
        kind: "tcp",
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
        let pool = BufferPool::new(
            std::num::NonZeroUsize::new(32 * 1024).expect("chunk size"),
            std::num::NonZeroUsize::new(4).expect("buffer count"),
            &budget,
        )
        .map_err(|_| IngressError::Stopped)?;
        if let Some(on_bound) = &endpoint.on_ingress_bound {
            on_bound();
        }
        loop {
            tokio::select! {
                _ = endpoint.shutdown.cancelled() => return Ok(()),
                accepted = listener.accept() => {
                    let (inbound, peer) = accepted?;
                    spawn_tcp_connection(inbound, peer, endpoint.clone(), pool.clone());
                }
            }
        }
    })
}

fn spawn_tcp_connection(
    inbound: tokio::net::TcpStream,
    peer: std::net::SocketAddr,
    endpoint: TunnelEndpoint,
    pool: BufferPool,
) {
    let guard = endpoint.guard.clone();
    let block_cert_resolver = endpoint.l4_block_cert_resolver.clone();
    let session = endpoint.carrier_session();
    let tunnel_id = endpoint.tunnel_id;
    tokio::spawn(async move {
        let ctx = ConnCtx {
            peer: peer.ip(),
            trust: SourceTrust::Tcp,
            trusted_proxy: false,
            tunnel_id: Some(tunnel_id),
        };
        // ACL 优先：拒绝时 linger=0 发 RST（内核防火墙未同步时的兜底）。
        let verdict = guard.check_conn(&ctx);
        if guard.record_if_denied(&verdict, peer.ip(), Some(tunnel_id)) {
            endpoint.traffic.record_reject_guard();
            reset_tcp(inbound);
            return;
        }
        if !endpoint.allow_new_conn().await {
            return;
        }
        let Some(_conn_slot) = ConnGuard::acquire(&endpoint) else {
            return;
        };

        let inbound = if guard.block_http_on_l4() {
            match http_block::maybe_block_browser_http(inbound, &guard, block_cert_resolver).await {
                Ok(http_block::BlockOutcome::Blocked) => {
                    guard.report_deny(
                        &Reason::BlockHttpOnL4,
                        peer.ip(),
                        Some(tunnel_id),
                        "blocked",
                    );
                    endpoint.traffic.record_reject_guard();
                    return;
                }
                Ok(http_block::BlockOutcome::Continue(stream)) => stream,
                Err(err) => {
                    // stream 已在 maybe_block_browser_http 内消费并随 Err 丢弃关闭。
                    tracing::warn!(%peer, %err, "http block page failed");
                    guard.report_deny(
                        &Reason::BlockHttpOnL4,
                        peer.ip(),
                        Some(tunnel_id),
                        format!("page_send_failed: {err}"),
                    );
                    endpoint.traffic.record_reject_guard();
                    return;
                }
            }
        } else {
            inbound
        };

        let header = StreamHeader {
            tunnel_id,
            src_addr: peer,
        };
        let Ok(carrier) = session.open_stream(header).await else {
            tracing::warn!(
                %tunnel_id,
                %peer,
                "ingress failed to open mux stream on carrier (session may have timed out; check quic idle / client online)"
            );
            return;
        };
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
            _ = endpoint.traffic.connections_closed() => {
                tracing::debug!(%tunnel_id, %peer, "tunnel stopped; closing active connection");
            }
        }
    });
}

fn reset_tcp(stream: tokio::net::TcpStream) {
    if let Ok(std_stream) = stream.into_std() {
        let socket = socket2::Socket::from(std_stream);
        let _ = socket.set_linger(Some(std::time::Duration::ZERO));
    }
}

struct ConnGuard(TunnelEndpoint);

impl ConnGuard {
    fn acquire(endpoint: &TunnelEndpoint) -> Option<Self> {
        endpoint
            .try_acquire_conn()
            .then(|| Self(endpoint.clone()))
    }
}

impl Drop for ConnGuard {
    fn drop(&mut self) {
        self.0.release_conn();
    }
}
