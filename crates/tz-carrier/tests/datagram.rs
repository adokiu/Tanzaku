use std::{net::SocketAddr, sync::Arc, time::Duration};

use bytes::Bytes;
use tz_carrier::{
    registry::{self, CarrierSession},
    types::{ConnectConfig, FlowId, ListenConfig},
};
use uuid::Uuid;

const SECRET: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";

fn node_config(tunnel_id: Uuid, client_id: Uuid, carrier: &str) -> tz_proto::NodeConfig {
    tz_proto::NodeConfig {
        node_id: Uuid::new_v4(),
        revision: 1,
        bind_addr: "127.0.0.1".into(),
        public_host: "127.0.0.1".into(),
        carrier_ports: serde_json::Value::Null,
        tcp_port_ranges: serde_json::Value::Null,
        udp_port_ranges: serde_json::Value::Null,
        port_exclude: serde_json::Value::Null,
        http_shared_port: 0,
        https_shared_port: 0,
        guard_policy: serde_json::Value::Null,
        cn_http_filing: false,
        cn_residency: false,
        domain_whitelist: Vec::new(),
        trusted_proxies: Vec::new(),
        board_ca_pem: String::new(),
        authorized_client_fingerprints: Vec::new(),
        certificate_fingerprint: None,
        tunnels: vec![tz_proto::TunnelSpec {
            tunnel_id,
            revision: 1,
            protocol: "udp".into(),
            carrier: carrier.into(),
            remote_port: None,
            target_host: None,
            target_port: None,
            target_url: None,
            client_id: Some(client_id),
            client_fingerprint: None,
            speed_limit_mbps: 0,
            max_conns: 0,
            max_new_conns_per_sec: 0,
            domains: Vec::new(),
            carrier_secret: if carrier == "tcp" { SECRET.into() } else { String::new() },
        }],
        http_domain_routes: Vec::new(),
        tls_certificates: Vec::new(),
        host_metrics_interval_secs: 1,
    }
}

fn free_port(udp: bool) -> u16 {
    if udp {
        std::net::UdpSocket::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
    } else {
        std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
    }
}

async fn recv(session: &Arc<dyn CarrierSession>) -> (FlowId, Bytes) {
    tokio::time::timeout(Duration::from_secs(5), session.recv_datagram())
        .await
        .expect("datagram timed out")
        .expect("session closed")
}

async fn send_until_received(
    from: &Arc<dyn CarrierSession>,
    to: &Arc<dyn CarrierSession>,
    flow: FlowId,
    payload: Bytes,
) {
    from.send_datagram(flow, payload.clone()).expect("send datagram");
    let (got_flow, got) = recv(to).await;
    assert_eq!(got_flow, flow);
    assert_eq!(got, payload);
}

async fn exercise(carrier: &str, reconnect: bool) {
    let factory = registry::find(carrier).expect("carrier registered");
    let (tunnel_id, client_id) = (Uuid::new_v4(), Uuid::new_v4());
    let port = free_port(carrier == "quic");
    let config = Arc::new(arc_swap::ArcSwap::from_pointee(node_config(tunnel_id, client_id, carrier)));
    let mut listener = (factory.listen)(ListenConfig {
        bind_addr: "127.0.0.1".into(),
        port,
        node_config: config,
    })
    .await
    .expect("listen");
    let server_addr: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
    let connect = || {
        (factory.connect)(ConnectConfig {
            server_addr,
            tunnel_id,
            client_id,
            carrier_secret: if carrier == "tcp" { SECRET.into() } else { String::new() },
        })
    };

    let client = connect().await.expect("connect");
    let node = tokio::time::timeout(Duration::from_secs(5), listener.accept())
        .await
        .expect("accept timed out")
        .expect("accept");

    for size in [64_usize, 1400, 4000] {
        let payload = Bytes::from(vec![size as u8; size]);
        send_until_received(&client, &node, FlowId(7), payload.clone()).await;
        send_until_received(&node, &client, FlowId(7), payload).await;
    }

    if reconnect {
        client.close("test reconnect");
        tokio::time::sleep(Duration::from_millis(200)).await;
        let client = connect().await.expect("reconnect");
        tokio::time::sleep(Duration::from_millis(200)).await;
        let payload = Bytes::from(vec![9_u8; 1400]);
        send_until_received(&client, &node, FlowId(8), payload.clone()).await;
        send_until_received(&node, &client, FlowId(8), payload).await;
    }
}

#[tokio::test]
async fn tcp_carrier_datagrams_survive_client_reconnect() {
    exercise("tcp", true).await;
}

#[tokio::test]
async fn quic_carrier_fragments_large_datagrams() {
    exercise("quic", false).await;
}
