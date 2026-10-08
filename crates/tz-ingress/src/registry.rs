use crate::types::TunnelEndpoint;
use std::{future::Future, pin::Pin};

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub struct IngressFactory {
    pub kind: &'static str,
    pub serve: fn(TunnelEndpoint) -> BoxFuture<'static, Result<(), IngressError>>,
}

inventory::collect!(IngressFactory);

pub fn find(kind: &str) -> Option<&'static IngressFactory> {
    inventory::iter::<IngressFactory>()
        .find(|factory| factory.kind == kind)
}

pub fn kinds() -> Vec<&'static str> {
    let mut kinds: Vec<_> = inventory::iter::<IngressFactory>()
        .map(|factory| factory.kind)
        .collect();
    kinds.sort_unstable();
    kinds
}

#[derive(Debug, thiserror::Error)]
pub enum IngressError {
    #[error("ingress IO failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("ingress stopped")]
    Stopped,
}
