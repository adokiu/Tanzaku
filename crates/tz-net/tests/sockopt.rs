use std::{
    net::{TcpListener, TcpStream},
    thread,
    time::Duration,
};
use tz_net::sockopt::{CongestionControl, OptionStatus, configure_tcp_stream};

#[test]
fn tcp_socket_configuration_reports_platform_capabilities_without_panicking() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let accept = thread::spawn(move || listener.accept().unwrap().0);
    let stream = TcpStream::connect(address).unwrap();
    let _peer = accept.join().unwrap();
    let report = configure_tcp_stream(
        &stream,
        CongestionControl::Bbr,
        Some(Duration::from_secs(30)),
        true,
    );
    assert!(matches!(
        report.congestion,
        OptionStatus::Applied | OptionStatus::Unsupported | OptionStatus::Failed(_)
    ));
    assert_eq!(report.nodelay, OptionStatus::Applied);
}
