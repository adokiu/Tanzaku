use std::{
    num::{NonZeroU64, NonZeroUsize},
    time::Duration,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tz_net::{
    budget::Budget,
    meter::TrafficMeter,
    pool::BufferPool,
    pump::{PumpOptions, pump},
    rate::RateDriver,
};

fn pool() -> BufferPool {
    BufferPool::new(
        NonZeroUsize::new(32 * 1024).unwrap(),
        NonZeroUsize::new(2).unwrap(),
        &Budget::new(64 * 1024),
    )
    .unwrap()
}

#[tokio::test]
async fn pump_copies_both_directions_and_propagates_half_close() {
    let (left, left_peer) = tokio::io::duplex(128);
    let (mut left_peer_reader, mut left_peer_writer) = tokio::io::split(left_peer);
    let (right, mut right_peer) = tokio::io::duplex(128);
    let task = tokio::spawn(pump(
        left,
        right,
        pool(),
        PumpOptions {
            left_to_right: None,
            right_to_left: None,
            meter: TrafficMeter::default(),
        },
    ));

    let payload = vec![31; 1024 * 1024];
    let send_left = tokio::spawn(async move {
        left_peer_writer.write_all(&payload).await.unwrap();
        left_peer_writer.shutdown().await.unwrap();
    });
    let receive_reply = tokio::spawn(async move {
        let mut reply = Vec::new();
        left_peer_reader.read_to_end(&mut reply).await.unwrap();
        reply
    });
    let receive_right = tokio::spawn(async move {
        let mut received = Vec::new();
        right_peer.read_to_end(&mut received).await.unwrap();
        assert_eq!(received, vec![31; 1024 * 1024]);
        right_peer.write_all(b"reply").await.unwrap();
        right_peer.shutdown().await.unwrap();
    });
    send_left.await.unwrap();
    receive_right.await.unwrap();

    assert_eq!(receive_reply.await.unwrap(), b"reply");
    let result = task.await.unwrap().unwrap();
    assert_eq!(result.left_to_right, 1024 * 1024);
    assert_eq!(result.right_to_left, 5);
}

#[tokio::test]
async fn slow_consumer_cannot_grow_buffers_beyond_pool_capacity() {
    let pool = pool();
    let (left, mut left_peer) = tokio::io::duplex(64);
    let (right, _right_peer) = tokio::io::duplex(64);
    let task = tokio::spawn(pump(
        left,
        right,
        pool.clone(),
        PumpOptions {
            left_to_right: None,
            right_to_left: None,
            meter: TrafficMeter::default(),
        },
    ));
    let writer = tokio::spawn(async move {
        let _ = left_peer.write_all(&vec![1; 1024 * 1024]).await;
    });
    for _ in 0..100 {
        tokio::task::yield_now().await;
    }
    let snapshot = pool.snapshot();
    assert!(snapshot.allocated_bytes <= 64 * 1024);
    assert!(snapshot.in_use <= 2);
    task.abort();
    let _ = task.await;
    writer.abort();
    let _ = writer.await;
    assert_eq!(pool.snapshot().in_use, 0);
}

#[tokio::test]
async fn idle_connections_do_not_allocate_copy_buffers() {
    let pool = pool();
    let (left, _left_peer) = tokio::io::duplex(64);
    let (right, _right_peer) = tokio::io::duplex(64);
    let task = tokio::spawn(pump(
        left,
        right,
        pool.clone(),
        PumpOptions {
            left_to_right: None,
            right_to_left: None,
            meter: TrafficMeter::default(),
        },
    ));
    for _ in 0..100 {
        tokio::task::yield_now().await;
    }
    assert_eq!(pool.snapshot().allocated_bytes, 0);
    task.abort();
    let _ = task.await;
}

#[tokio::test(start_paused = true)]
async fn token_bucket_applies_backpressure_to_pump() {
    let (clock, driver) = RateDriver::new(Duration::from_millis(10)).unwrap();
    let driver_task = tokio::spawn(driver.run());
    let bucket = clock.bucket(
        NonZeroU64::new(1024).unwrap(),
        NonZeroUsize::new(1024).unwrap(),
    );
    let (left, mut left_peer) = tokio::io::duplex(128);
    let (right, mut right_peer) = tokio::io::duplex(128);
    let task = tokio::spawn(pump(
        left,
        right,
        pool(),
        PumpOptions {
            left_to_right: Some(bucket),
            right_to_left: None,
            meter: TrafficMeter::default(),
        },
    ));
    let writer = tokio::spawn(async move {
        left_peer.write_all(&vec![7; 2048]).await.unwrap();
        left_peer.shutdown().await.unwrap();
    });
    let receiver = tokio::spawn(async move {
        let mut received = Vec::new();
        right_peer.read_to_end(&mut received).await.unwrap();
        received.len()
    });
    writer.await.unwrap();
    assert_eq!(receiver.await.unwrap(), 2048);
    let result = task.await.unwrap().unwrap();
    assert_eq!(result.left_to_right, 2048);
    driver_task.abort();
    let _ = driver_task.await;
}
