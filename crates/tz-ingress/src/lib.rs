#[cfg(feature = "http")]
pub mod http_l7;
pub mod filing;
pub mod http_block;
pub mod http_headers;
pub mod limits;
pub mod registry;
pub mod shared_http;
pub mod shared_https;
mod bind;
mod udp_src;
pub mod shutdown;
pub mod traffic;
pub mod types;

include!(concat!(env!("OUT_DIR"), "/ingress.inc.rs"));

pub fn registered_kinds() -> Vec<&'static str> {
    registry::kinds()
}
