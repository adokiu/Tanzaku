use tz_net::meter::{ByteCounter, TrafficMeter};

#[test]
fn counters_saturate_instead_of_wrapping() {
    let counter = ByteCounter::default();
    counter.add(u64::MAX as usize);
    counter.add(1);
    assert_eq!(counter.total(), u64::MAX);
}

#[test]
fn traffic_directions_are_counted_independently() {
    let meter = TrafficMeter::default();
    meter.left_to_right.add(1200);
    meter.right_to_left.add(300);
    assert_eq!(meter.snapshot().left_to_right, 1200);
    assert_eq!(meter.snapshot().right_to_left, 300);
}

#[test]
fn take_delta_resets_counters() {
    let meter = TrafficMeter::default();
    meter.left_to_right.add(500);
    meter.right_to_left.add(100);
    let delta = meter.take_delta();
    assert_eq!(delta.left_to_right, 500);
    assert_eq!(delta.right_to_left, 100);
    assert_eq!(meter.snapshot().left_to_right, 0);
    assert_eq!(meter.snapshot().right_to_left, 0);
}
