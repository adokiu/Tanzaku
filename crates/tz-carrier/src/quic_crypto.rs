//! 自定义 QUIC 包完整性（无 TLS），与对端版本协商一致。

use bytes::{Buf, BytesMut};
use quinn_proto::crypto::{
    ClientConfig, ExportKeyingMaterialError, KeyPair, Keys, ServerConfig, Session,
    UnsupportedVersion,
};
use quinn_proto::transport_parameters::TransportParameters;
use quinn_proto::{
    ConnectError, ConnectionId, Side, TransportError,
    crypto::{CryptoError, HeaderKey, PacketKey},
};
use seahash::SeaHasher;
use std::any::Any;
use std::{hash::Hasher, sync::Arc};

/// 自定义 QUIC 版本（0x545A4231 = "TZB1"）。
pub const QUIC_VERSION_CUSTOM: u32 = 0x545A_4231;

#[derive(Debug, Clone, Copy)]
struct PacketIntegrityKey {
    pn_bound: bool,
}

impl PacketIntegrityKey {
    fn for_version(version: u32) -> Self {
        Self {
            pn_bound: version == QUIC_VERSION_CUSTOM,
        }
    }

    fn keys(self) -> Keys {
        Keys {
            header: KeyPair {
                local: Box::new(self),
                remote: Box::new(self),
            },
            packet: KeyPair {
                local: Box::new(self),
                remote: Box::new(self),
            },
        }
    }

    fn checksum(slices: &[&[u8]]) -> u64 {
        let mut hasher = SeaHasher::default();
        for slice in slices {
            hasher.write(&(slice.len() as u64).to_le_bytes());
            hasher.write(slice);
        }
        hasher.finish()
    }

    fn checksum_with_packet(packet: u64, slices: &[&[u8]]) -> u64 {
        let mut hasher = SeaHasher::default();
        hasher.write(&packet.to_le_bytes());
        for slice in slices {
            hasher.write(&(slice.len() as u64).to_le_bytes());
            hasher.write(slice);
        }
        hasher.finish()
    }

    fn checksum_for(&self, packet: u64, slices: &[&[u8]]) -> u64 {
        if self.pn_bound {
            Self::checksum_with_packet(packet, slices)
        } else {
            Self::checksum(slices)
        }
    }
}

impl HeaderKey for PacketIntegrityKey {
    fn decrypt(&self, _: usize, _: &mut [u8]) {}
    fn encrypt(&self, _: usize, _: &mut [u8]) {}
    fn sample_size(&self) -> usize {
        0
    }
}

impl PacketKey for PacketIntegrityKey {
    fn encrypt(&self, packet: u64, buf: &mut [u8], header_len: usize) {
        let (header, rest) = buf.split_at_mut(header_len);
        let (payload, tag) = rest.split_at_mut(rest.len() - self.tag_len());
        let checksum = self.checksum_for(packet, &[header, payload]);
        tag.copy_from_slice(&checksum.to_be_bytes());
    }

    fn decrypt(
        &self,
        packet: u64,
        header: &[u8],
        payload: &mut BytesMut,
    ) -> Result<(), CryptoError> {
        let tag = payload.split_off(payload.len() - self.tag_len()).get_u64();
        let checksum = self.checksum_for(packet, &[header, payload]);
        if checksum != tag {
            return Err(CryptoError);
        }
        Ok(())
    }

    fn tag_len(&self) -> usize {
        8
    }

    fn confidentiality_limit(&self) -> u64 {
        u64::MAX
    }

    fn integrity_limit(&self) -> u64 {
        1 << 36
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HandshakeState {
    EmitInitial,
    EmitHandshake,
    Done,
}

#[derive(Debug)]
struct QuicHandshakeSession {
    side: Side,
    key: PacketIntegrityKey,
    state: HandshakeState,
    local: TransportParameters,
    remote: Option<TransportParameters>,
}

impl QuicHandshakeSession {
    fn new(side: Side, version: u32, params: TransportParameters) -> Self {
        Self {
            side,
            key: PacketIntegrityKey::for_version(version),
            state: HandshakeState::EmitInitial,
            local: params,
            remote: None,
        }
    }
}

impl Session for QuicHandshakeSession {
    fn initial_keys(&self, _: &ConnectionId, _: Side) -> Keys {
        self.key.keys()
    }

    fn handshake_data(&self) -> Option<Box<dyn Any>> {
        self.remote.map(|params| Box::new(params) as Box<dyn Any>)
    }

    fn peer_identity(&self) -> Option<Box<dyn Any>> {
        None
    }

    fn early_crypto(&self) -> Option<(Box<dyn HeaderKey>, Box<dyn PacketKey>)> {
        None
    }

    fn early_data_accepted(&self) -> Option<bool> {
        Some(false)
    }

    fn is_handshaking(&self) -> bool {
        self.remote.is_none() || self.state != HandshakeState::Done
    }

    fn read_handshake(&mut self, mut buf: &[u8]) -> Result<bool, TransportError> {
        if self.remote.is_none() {
            self.remote = Some(
                TransportParameters::read(self.side, &mut buf)
                    .expect("failed to read transport parameters"),
            );
        }
        Ok(true)
    }

    fn transport_parameters(&self) -> Result<Option<TransportParameters>, TransportError> {
        Ok(self.remote)
    }

    fn write_handshake(&mut self, buf: &mut Vec<u8>) -> Option<Keys> {
        match self.state {
            HandshakeState::EmitInitial => {
                if self.side.is_client() {
                    self.local.write(buf);
                }
                self.state = HandshakeState::EmitHandshake;
                Some(self.key.keys())
            }
            HandshakeState::EmitHandshake => {
                if self.side.is_server() {
                    self.local.write(buf);
                }
                self.state = HandshakeState::Done;
                Some(self.key.keys())
            }
            HandshakeState::Done => None,
        }
    }

    fn next_1rtt_keys(&mut self) -> Option<KeyPair<Box<dyn PacketKey>>> {
        Some(self.key.keys().packet)
    }

    fn is_valid_retry(&self, _: &ConnectionId, _: &[u8], _: &[u8]) -> bool {
        true
    }

    fn export_keying_material(
        &self,
        _: &mut [u8],
        _: &[u8],
        _: &[u8],
    ) -> Result<(), ExportKeyingMaterialError> {
        Ok(())
    }
}

#[derive(Debug)]
pub struct PlainQuicCrypto;

impl ClientConfig for PlainQuicCrypto {
    fn start_session(
        self: Arc<Self>,
        version: u32,
        _server_name: &str,
        params: &TransportParameters,
    ) -> Result<Box<dyn Session>, ConnectError> {
        Ok(Box::new(QuicHandshakeSession::new(
            Side::Client,
            version,
            *params,
        )))
    }
}

impl ServerConfig for PlainQuicCrypto {
    fn initial_keys(&self, version: u32, _: &ConnectionId) -> Result<Keys, UnsupportedVersion> {
        Ok(PacketIntegrityKey::for_version(version).keys())
    }

    fn retry_tag(&self, _: u32, _: &ConnectionId, _: &[u8]) -> [u8; 16] {
        [0u8; 16]
    }

    fn start_session(
        self: Arc<Self>,
        version: u32,
        params: &TransportParameters,
    ) -> Box<dyn Session> {
        Box::new(QuicHandshakeSession::new(
            Side::Server,
            version,
            *params,
        ))
    }
}
