//! 隧道 revision 更新时旧监听异步退出，新监听短暂重试，等端口释放。

use std::{future::Future, io, time::Duration};

const RETRY_INTERVAL: Duration = Duration::from_millis(100);
const MAX_ATTEMPTS: usize = 30;

async fn retry_addr_in_use<T, F, Fut>(mut bind: F) -> io::Result<T>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = io::Result<T>>,
{
    let mut attempt = 1;
    loop {
        match bind().await {
            Err(err) if err.kind() == io::ErrorKind::AddrInUse && attempt < MAX_ATTEMPTS => {
                attempt += 1;
                tokio::time::sleep(RETRY_INTERVAL).await;
            }
            other => return other,
        }
    }
}

pub async fn tcp_listener(addr: &str, port: u16) -> io::Result<tokio::net::TcpListener> {
    retry_addr_in_use(|| tokio::net::TcpListener::bind((addr, port))).await
}

pub async fn udp_socket(addr: &str, port: u16) -> io::Result<tokio::net::UdpSocket> {
    retry_addr_in_use(|| tokio::net::UdpSocket::bind((addr, port))).await
}
