use bytes::{Buf, BufMut, Bytes, BytesMut};
use std::net::SocketAddr;
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum FrameType {
    Open = 1,
    Data = 2,
    Window = 3,
    Fin = 4,
    Rst = 5,
    Dgram = 6,
    Ping = 7,
}

impl TryFrom<u8> for FrameType {
    type Error = FrameError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::Open),
            2 => Ok(Self::Data),
            3 => Ok(Self::Window),
            4 => Ok(Self::Fin),
            5 => Ok(Self::Rst),
            6 => Ok(Self::Dgram),
            7 => Ok(Self::Ping),
            _ => Err(FrameError::UnknownType(value)),
        }
    }
}

#[derive(Debug, Error)]
pub enum FrameError {
    #[error("incomplete frame")]
    Incomplete,
    #[error("unknown frame type {0}")]
    UnknownType(u8),
    #[error("frame payload is invalid")]
    InvalidPayload,
}

#[derive(Debug, Clone)]
pub struct MuxFrame {
    pub frame_type: FrameType,
    pub flags: u8,
    pub stream_id: u32,
    pub payload: Bytes,
}

impl MuxFrame {
    pub const HEADER_LEN: usize = 10;

    pub fn encode(&self, buffer: &mut BytesMut) {
        buffer.put_u8(self.frame_type as u8);
        buffer.put_u8(self.flags);
        buffer.put_u32(self.stream_id);
        buffer.put_u32(
            u32::try_from(self.payload.len()).expect("mux frame payload length fits in u32"),
        );
        buffer.extend_from_slice(&self.payload);
    }

    pub fn decode(buffer: &mut BytesMut) -> Result<Option<Self>, FrameError> {
        if buffer.len() < Self::HEADER_LEN {
            return Ok(None);
        }
        let frame_type = FrameType::try_from(buffer[0])?;
        let flags = buffer[1];
        let mut header = &buffer[2..Self::HEADER_LEN];
        let stream_id = header.get_u32();
        let length = header.get_u32() as usize;
        if length > super::MAX_MUX_PAYLOAD {
            return Err(FrameError::InvalidPayload);
        }
        if buffer.len() < Self::HEADER_LEN + length {
            return Ok(None);
        }
        buffer.advance(Self::HEADER_LEN);
        let payload = buffer.split_to(length).freeze();
        Ok(Some(Self {
            frame_type,
            flags,
            stream_id,
            payload,
        }))
    }
}

pub fn encode_open(stream_id: u32, tunnel_id: Uuid, src_addr: SocketAddr) -> MuxFrame {
    MuxFrame {
        frame_type: FrameType::Open,
        flags: 0,
        stream_id,
        payload: crate::stream_preamble::encode_open_payload(tunnel_id, src_addr).into(),
    }
}

pub fn decode_open(payload: &[u8]) -> Result<(Uuid, SocketAddr), FrameError> {
    if payload.len() < 17 {
        return Err(FrameError::InvalidPayload);
    }
    let tunnel_id = Uuid::from_bytes(payload[0..16].try_into().map_err(|_| FrameError::InvalidPayload)?);
    match payload[16] {
        4 if payload.len() >= 23 => {
            let ip = std::net::Ipv4Addr::new(payload[17], payload[18], payload[19], payload[20]);
            let port = u16::from_be_bytes([payload[21], payload[22]]);
            Ok((tunnel_id, SocketAddr::from((ip, port))))
        }
        6 if payload.len() >= 35 => {
            let ip = std::net::Ipv6Addr::from(<[u8; 16]>::try_from(&payload[17..33]).map_err(|_| FrameError::InvalidPayload)?);
            let port = u16::from_be_bytes([payload[33], payload[34]]);
            Ok((tunnel_id, SocketAddr::from((ip, port))))
        }
        _ => Err(FrameError::InvalidPayload),
    }
}
