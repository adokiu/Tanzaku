pub mod carrier_stream;
#[cfg(feature = "tcp")]
pub mod tcp_enc;
pub mod mux;
pub mod registry;
pub mod stream_preamble;
pub mod types;

#[cfg(feature = "quic")]
pub mod quic_crypto;
#[cfg(feature = "quic")]
pub mod quic_io;
#[cfg(feature = "quic")]
pub mod quic_transport;

include!(concat!(env!("OUT_DIR"), "/carriers.inc.rs"));

pub use registry::RegisteredCarrier;

pub fn registered_kinds() -> Vec<&'static str> {
    registry::kinds()
}

pub fn registered_carriers() -> Vec<RegisteredCarrier> {
    registry::registered()
}
