//! TCP carrier 在 bind 确认后的 ChaCha20-Poly1305 帧加密。
//! 帧格式：`u32 BE 密文长度` + `密文(含 16B tag)`；nonce = 每方向独立递增序号。

use chacha20poly1305::{
    aead::{Aead, KeyInit, Payload},
    ChaCha20Poly1305, Key,
};
use std::{
    io,
    pin::Pin,
    task::{Context, Poll},
};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;

const MAX_PLAINTEXT_FRAME: usize = 16 * 1024;
const TAG_LEN: usize = 16;
/// 待发送密文超过该值时不再接收新明文，形成背压。
const MAX_WRITE_BACKLOG: usize = 4 * (MAX_PLAINTEXT_FRAME + TAG_LEN + 4);

enum ReadStep {
    Len { got: usize },
    Body { len: usize, got: usize },
}

pub struct EncryptedStream {
    tcp: TcpStream,
    read_cipher: ChaCha20Poly1305,
    write_cipher: ChaCha20Poly1305,
    read_seq: u64,
    write_seq: u64,
    read_step: ReadStep,
    read_len: [u8; 4],
    read_body: Vec<u8>,
    read_plain: Vec<u8>,
    read_plain_off: usize,
    write_out: Vec<u8>,
    write_out_off: usize,
}

fn nonce(seq: u64) -> [u8; 12] {
    let mut nonce = [0_u8; 12];
    nonce[4..].copy_from_slice(&seq.to_be_bytes());
    nonce
}

impl EncryptedStream {
    pub fn new(tcp: TcpStream, read_key: [u8; 32], write_key: [u8; 32]) -> Self {
        Self {
            tcp,
            read_cipher: ChaCha20Poly1305::new(Key::from_slice(&read_key)),
            write_cipher: ChaCha20Poly1305::new(Key::from_slice(&write_key)),
            read_seq: 0,
            write_seq: 0,
            read_step: ReadStep::Len { got: 0 },
            read_len: [0_u8; 4],
            read_body: Vec::new(),
            read_plain: Vec::new(),
            read_plain_off: 0,
            write_out: Vec::new(),
            write_out_off: 0,
        }
    }

    fn queue_frame(&mut self, plaintext: &[u8]) -> io::Result<()> {
        let seq = self.write_seq;
        self.write_seq = seq
            .checked_add(1)
            .ok_or_else(|| io::Error::other("tcp carrier nonce exhausted"))?;
        let ct = self
            .write_cipher
            .encrypt(&nonce(seq).into(), Payload { msg: plaintext, aad: b"" })
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "tcp carrier encrypt failed"))?;
        let len = u32::try_from(ct.len())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "tcp carrier frame too large"))?;
        if self.write_out_off > 0 && self.write_out_off == self.write_out.len() {
            self.write_out.clear();
            self.write_out_off = 0;
        }
        self.write_out.extend_from_slice(&len.to_be_bytes());
        self.write_out.extend_from_slice(&ct);
        Ok(())
    }

    /// 尽量把已加密数据写入 socket；仅当底层 Pending 时返回 Pending。
    fn poll_drain(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        while self.write_out_off < self.write_out.len() {
            let slice = &self.write_out[self.write_out_off..];
            match Pin::new(&mut self.tcp).poll_write(cx, slice) {
                Poll::Ready(Ok(0)) => {
                    return Poll::Ready(Err(io::Error::new(
                        io::ErrorKind::WriteZero,
                        "tcp carrier write closed",
                    )));
                }
                Poll::Ready(Ok(n)) => self.write_out_off += n,
                Poll::Ready(Err(err)) => return Poll::Ready(Err(err)),
                Poll::Pending => return Poll::Pending,
            }
        }
        self.write_out.clear();
        self.write_out_off = 0;
        Poll::Ready(Ok(()))
    }

    fn pending_out(&self) -> usize {
        self.write_out.len() - self.write_out_off
    }
}

impl AsyncRead for EncryptedStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = &mut *self;
        loop {
            if this.read_plain_off < this.read_plain.len() {
                let available = &this.read_plain[this.read_plain_off..];
                let take = available.len().min(buf.remaining());
                buf.put_slice(&available[..take]);
                this.read_plain_off += take;
                if this.read_plain_off == this.read_plain.len() {
                    this.read_plain.clear();
                    this.read_plain_off = 0;
                }
                return Poll::Ready(Ok(()));
            }
            match this.read_step {
                ReadStep::Len { got } => {
                    let mut tmp = ReadBuf::new(&mut this.read_len[got..]);
                    match Pin::new(&mut this.tcp).poll_read(cx, &mut tmp) {
                        Poll::Pending => return Poll::Pending,
                        Poll::Ready(Err(err)) => return Poll::Ready(Err(err)),
                        Poll::Ready(Ok(())) => {
                            let filled = tmp.filled().len();
                            if filled == 0 {
                                if got == 0 {
                                    return Poll::Ready(Ok(()));
                                }
                                return Poll::Ready(Err(io::Error::new(
                                    io::ErrorKind::UnexpectedEof,
                                    "tcp carrier truncated frame header",
                                )));
                            }
                            let got = got + filled;
                            if got < 4 {
                                this.read_step = ReadStep::Len { got };
                                continue;
                            }
                            let len = u32::from_be_bytes(this.read_len) as usize;
                            if len <= TAG_LEN || len > MAX_PLAINTEXT_FRAME + TAG_LEN {
                                return Poll::Ready(Err(io::Error::new(
                                    io::ErrorKind::InvalidData,
                                    "tcp carrier bad frame length",
                                )));
                            }
                            this.read_body.clear();
                            this.read_body.resize(len, 0);
                            this.read_step = ReadStep::Body { len, got: 0 };
                        }
                    }
                }
                ReadStep::Body { len, got } => {
                    let mut tmp = ReadBuf::new(&mut this.read_body[got..len]);
                    match Pin::new(&mut this.tcp).poll_read(cx, &mut tmp) {
                        Poll::Pending => return Poll::Pending,
                        Poll::Ready(Err(err)) => return Poll::Ready(Err(err)),
                        Poll::Ready(Ok(())) => {
                            let filled = tmp.filled().len();
                            if filled == 0 {
                                return Poll::Ready(Err(io::Error::new(
                                    io::ErrorKind::UnexpectedEof,
                                    "tcp carrier truncated frame body",
                                )));
                            }
                            let got = got + filled;
                            if got < len {
                                this.read_step = ReadStep::Body { len, got };
                                continue;
                            }
                            let seq = this.read_seq;
                            this.read_seq = seq.wrapping_add(1);
                            let plain = this
                                .read_cipher
                                .decrypt(
                                    &nonce(seq).into(),
                                    Payload { msg: &this.read_body, aad: b"" },
                                )
                                .map_err(|_| {
                                    io::Error::new(
                                        io::ErrorKind::InvalidData,
                                        "tcp carrier decrypt failed",
                                    )
                                })?;
                            this.read_plain = plain;
                            this.read_plain_off = 0;
                            this.read_step = ReadStep::Len { got: 0 };
                        }
                    }
                }
            }
        }
    }
}

impl AsyncWrite for EncryptedStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = &mut *self;
        if this.pending_out() >= MAX_WRITE_BACKLOG {
            match this.poll_drain(cx) {
                Poll::Ready(Ok(())) => {}
                Poll::Ready(Err(err)) => return Poll::Ready(Err(err)),
                Poll::Pending => return Poll::Pending,
            }
        }
        if buf.is_empty() {
            return Poll::Ready(Ok(0));
        }
        let take = buf.len().min(MAX_PLAINTEXT_FRAME);
        this.queue_frame(&buf[..take])?;
        if let Poll::Ready(Err(err)) = this.poll_drain(cx) {
            return Poll::Ready(Err(err));
        }
        Poll::Ready(Ok(take))
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = &mut *self;
        match this.poll_drain(cx) {
            Poll::Ready(Ok(())) => Pin::new(&mut this.tcp).poll_flush(cx),
            other => other,
        }
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = &mut *self;
        match this.poll_drain(cx) {
            Poll::Ready(Ok(())) => Pin::new(&mut this.tcp).poll_shutdown(cx),
            other => other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn roundtrip_small_and_large() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (a, b) = ([1_u8; 32], [2_u8; 32]);
        let server = tokio::spawn(async move {
            let (tcp, _) = listener.accept().await.unwrap();
            let mut s = EncryptedStream::new(tcp, a, b);
            let mut confirm = [0_u8; 4];
            s.read_exact(&mut confirm).await.unwrap();
            assert_eq!(&confirm, b"TZK1");
            s.write_all(b"TZK1").await.unwrap();
            s.flush().await.unwrap();
            let mut big = vec![0_u8; 3 * 1024 * 1024];
            s.read_exact(&mut big).await.unwrap();
            assert!(big.iter().enumerate().all(|(i, v)| *v == (i % 251) as u8));
            s.write_all(&big).await.unwrap();
            s.shutdown().await.unwrap();
        });
        let tcp = TcpStream::connect(addr).await.unwrap();
        let mut c = EncryptedStream::new(tcp, b, a);
        c.write_all(b"TZK1").await.unwrap();
        c.flush().await.unwrap();
        let mut confirm = [0_u8; 4];
        c.read_exact(&mut confirm).await.unwrap();
        assert_eq!(&confirm, b"TZK1");
        let big: Vec<u8> = (0..3 * 1024 * 1024).map(|i| (i % 251) as u8).collect();
        let (mut r, mut w) = tokio::io::split(c);
        let writer = tokio::spawn(async move {
            w.write_all(&big).await.unwrap();
            w.flush().await.unwrap();
            w
        });
        let mut echoed = Vec::new();
        r.read_to_end(&mut echoed).await.unwrap();
        assert_eq!(echoed.len(), 3 * 1024 * 1024);
        writer.await.unwrap();
        server.await.unwrap();
    }
}
