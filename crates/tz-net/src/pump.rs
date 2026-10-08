use crate::{
    meter::{TrafficMeter, TrafficSnapshot},
    pool::BufferPool,
    rate::TokenBucket,
};
use std::{io, pin::Pin, task::Poll};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};

const PROBE_SIZE: usize = 64;

pub struct PumpOptions {
    pub left_to_right: Option<TokenBucket>,
    pub right_to_left: Option<TokenBucket>,
    pub meter: TrafficMeter,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PumpResult {
    pub left_to_right: u64,
    pub right_to_left: u64,
}

pub async fn pump<A, B>(
    left: A,
    right: B,
    pool: BufferPool,
    options: PumpOptions,
) -> io::Result<PumpResult>
where
    A: AsyncRead + AsyncWrite + Unpin,
    B: AsyncRead + AsyncWrite + Unpin,
{
    let (left_reader, left_writer) = tokio::io::split(left);
    let (right_reader, right_writer) = tokio::io::split(right);
    let left_to_right = copy_direction(
        left_reader,
        right_writer,
        pool.clone(),
        options.left_to_right,
        &options.meter.left_to_right,
    );
    let right_to_left = copy_direction(
        right_reader,
        left_writer,
        pool,
        options.right_to_left,
        &options.meter.right_to_left,
    );
    tokio::try_join!(left_to_right, right_to_left)?;
    let TrafficSnapshot {
        left_to_right,
        right_to_left,
    } = options.meter.snapshot();
    Ok(PumpResult {
        left_to_right,
        right_to_left,
    })
}

async fn copy_direction<R, W>(
    mut reader: R,
    mut writer: W,
    pool: BufferPool,
    limiter: Option<TokenBucket>,
    counter: &crate::meter::ByteCounter,
) -> io::Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut probe = [0u8; PROBE_SIZE];
    loop {
        let read = reader.read(&mut probe).await?;
        if read == 0 {
            writer.shutdown().await?;
            return Ok(());
        }
        let mut buffer = pool.acquire().await?;
        buffer.buffer_mut()[..read].copy_from_slice(&probe[..read]);
        buffer.mark_used(read);
        let mut length = read;
        let extra = poll_read_available(&mut reader, &mut buffer.buffer_mut()[length..]).await?;
        length += extra;
        buffer.mark_used(length);
        let mut offset = 0;
        while offset < length {
            let grant = match &limiter {
                Some(limiter) => Some(
                    limiter
                        .acquire(length - offset)
                        .await
                        .map_err(rate_io_error)?,
                ),
                None => None,
            };
            let allowed = grant
                .as_ref()
                .map_or(length - offset, |grant| grant.remaining());
            let written = writer
                .write(&buffer.as_ref()[offset..offset + allowed])
                .await?;
            if written == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "relay writer made no progress",
                ));
            }
            if let Some(mut grant) = grant {
                grant.consume(written).map_err(rate_io_error)?;
            }
            counter.add(written);
            offset += written;
        }
        writer.flush().await?;
    }
}

async fn poll_read_available<R: AsyncRead + Unpin>(
    reader: &mut R,
    dst: &mut [u8],
) -> io::Result<usize> {
    std::future::poll_fn(|cx| {
        let mut buffer = ReadBuf::new(dst);
        match Pin::new(&mut *reader).poll_read(cx, &mut buffer) {
            Poll::Ready(Ok(())) => Poll::Ready(Ok(buffer.filled().len())),
            Poll::Ready(Err(error)) => Poll::Ready(Err(error)),
            Poll::Pending => Poll::Ready(Ok(0)),
        }
    })
    .await
}

fn rate_io_error(error: crate::rate::RateError) -> io::Error {
    let kind = match error {
        crate::rate::RateError::Closed => io::ErrorKind::BrokenPipe,
        crate::rate::RateError::ZeroRequest => io::ErrorKind::InvalidInput,
        crate::rate::RateError::Poisoned => io::ErrorKind::Other,
        crate::rate::RateError::InvalidPeriod | crate::rate::RateError::Overconsume => {
            io::ErrorKind::InvalidInput
        }
    };
    io::Error::new(kind, error)
}
