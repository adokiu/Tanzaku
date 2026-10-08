use crate::types::StreamHeader;
use bytes::{BufMut, BytesMut};
use std::io;
use std::net::SocketAddr;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use uuid::Uuid;

pub const STREAM_HEADER_LIMIT: u16 = 512;
pub const STREAM_HEADER_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

pub fn encode_open_payload(tunnel_id: Uuid, src_addr: SocketAddr) -> Vec<u8> {
    let mut payload = BytesMut::with_capacity(18);
    payload.extend_from_slice(tunnel_id.as_bytes());
    match src_addr {
        SocketAddr::V4(addr) => {
            payload.put_u8(4);
            payload.extend_from_slice(&addr.ip().octets());
            payload.put_u16(addr.port());
        }
        SocketAddr::V6(addr) => {
            payload.put_u8(6);
            payload.extend_from_slice(&addr.ip().octets());
            payload.put_u16(addr.port());
        }
    }
    payload.to_vec()
}

pub fn decode_open_payload(payload: &[u8]) -> io::Result<(Uuid, SocketAddr)> {
    if payload.len() < 17 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "stream header too short",
        ));
    }
    let tunnel_id = Uuid::from_bytes(
        payload[0..16]
            .try_into()
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "bad tunnel id"))?,
    );
    match payload[16] {
        4 if payload.len() >= 23 => {
            let ip = std::net::Ipv4Addr::new(
                payload[17],
                payload[18],
                payload[19],
                payload[20],
            );
            let port = u16::from_be_bytes([payload[21], payload[22]]);
            Ok((tunnel_id, SocketAddr::from((ip, port))))
        }
        6 if payload.len() >= 35 => {
            let ip = std::net::Ipv6Addr::from(
                <[u8; 16]>::try_from(&payload[17..33]).map_err(|_| {
                    io::Error::new(io::ErrorKind::InvalidData, "bad ipv6")
                })?,
            );
            let port = u16::from_be_bytes([payload[33], payload[34]]);
            Ok((tunnel_id, SocketAddr::from((ip, port))))
        }
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "bad stream header address",
        )),
    }
}

pub fn encode_stream_preamble(header: &StreamHeader) -> BytesMut {
    let payload = encode_open_payload(header.tunnel_id, header.src_addr);
    let mut out = BytesMut::with_capacity(2 + payload.len());
    out.extend_from_slice(&(payload.len() as u16).to_be_bytes());
    out.extend_from_slice(&payload);
    out
}

pub async fn write_stream_preamble<W: AsyncWrite + Unpin>(
    writer: &mut W,
    header: &StreamHeader,
) -> io::Result<()> {
    let frame = encode_stream_preamble(header);
    writer.write_all(&frame).await?;
    writer.flush().await
}

pub async fn read_stream_preamble<R: AsyncRead + Unpin>(reader: &mut R) -> io::Result<StreamHeader> {
    let len = tokio::time::timeout(STREAM_HEADER_TIMEOUT, reader.read_u16())
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "stream header timeout"))??;
    if len > STREAM_HEADER_LIMIT {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "stream header too long",
        ));
    }
    let mut payload = vec![0_u8; len as usize];
    tokio::time::timeout(STREAM_HEADER_TIMEOUT, reader.read_exact(&mut payload))
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "stream header body timeout"))??;
    let (tunnel_id, src_addr) = decode_open_payload(&payload)?;
    Ok(StreamHeader {
        tunnel_id,
        src_addr,
    })
}
