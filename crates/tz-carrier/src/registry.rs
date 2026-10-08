use crate::types::{ConnectConfig, ListenConfig, SocketKind};
use async_trait::async_trait;
use std::{future::Future, pin::Pin, sync::Arc};

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub struct CarrierFactory {
    pub kind: &'static str,
    pub encryption: &'static str,
    pub socket: SocketKind,
    pub listen: fn(ListenConfig) -> BoxFuture<'static, Result<Box<dyn CarrierListener>, CarrierError>>,
    pub connect: fn(ConnectConfig) -> BoxFuture<'static, Result<Arc<dyn CarrierSession>, CarrierError>>,
}

inventory::collect!(CarrierFactory);

pub fn find(kind: &str) -> Option<&'static CarrierFactory> {
    inventory::iter::<CarrierFactory>()
        .find(|factory| factory.kind == kind)
}

pub fn kinds() -> Vec<&'static str> {
    let mut kinds: Vec<_> = inventory::iter::<CarrierFactory>()
        .map(|factory| factory.kind)
        .collect();
    kinds.sort_unstable();
    kinds
}

#[derive(Clone, Copy, Debug)]
pub struct RegisteredCarrier {
    pub kind: &'static str,
    pub socket: &'static str,
}

pub fn registered() -> Vec<RegisteredCarrier> {
    let mut carriers: Vec<_> = inventory::iter::<CarrierFactory>()
        .map(|factory| RegisteredCarrier {
            kind: factory.kind,
            socket: factory.socket.as_str(),
        })
        .collect();
    carriers.sort_by_key(|carrier| carrier.kind);
    carriers
}

#[derive(Debug, thiserror::Error)]
pub enum CarrierError {
    #[error("carrier IO failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("carrier TLS failed: {0}")]
    Tls(String),
    #[error("carrier is unavailable")]
    Unavailable,
}

#[async_trait]
pub trait CarrierListener: Send + Sync {
    async fn accept(&mut self) -> Result<Arc<dyn CarrierSession>, CarrierError>;
    fn close(&self, reason: &str);
}

#[async_trait]
pub trait CarrierSession: Send + Sync {
    async fn open_stream(
        &self,
        header: crate::types::StreamHeader,
    ) -> Result<crate::types::CarrierStream, crate::types::OpenError>;
    async fn accept_stream(
        &self,
    ) -> Result<(crate::types::StreamHeader, crate::types::CarrierStream), crate::types::OpenError>;
    fn send_datagram(
        &self,
        flow: crate::types::FlowId,
        data: bytes::Bytes,
    ) -> Result<(), crate::types::DatagramDropped>;
    /// 发送队列满时等待而不是丢弃，把背压传回调用方（由其内核 socket 缓冲吸收突发）。
    async fn send_datagram_wait(
        &self,
        flow: crate::types::FlowId,
        data: bytes::Bytes,
    ) -> Result<(), crate::types::DatagramDropped> {
        self.send_datagram(flow, data)
    }
    async fn recv_datagram(&self) -> Result<(crate::types::FlowId, bytes::Bytes), crate::types::OpenError>;
    fn peer(&self) -> &crate::types::PeerIdentity;
    fn stats(&self) -> crate::types::SessionStats;
    fn close(&self, reason: &str);
    fn is_closed(&self) -> bool;
}
