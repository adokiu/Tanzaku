use socket2::{SockRef, TcpKeepalive};
use std::{io, net::TcpStream, time::Duration};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OptionStatus {
    Applied,
    Unsupported,
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SocketReport {
    pub congestion: OptionStatus,
    pub keepalive: OptionStatus,
    pub nodelay: OptionStatus,
}

#[derive(Debug, Clone)]
pub enum CongestionControl {
    System,
    Bbr,
    Cubic,
    Named(String),
}

pub fn configure_tcp_stream(
    stream: &TcpStream,
    congestion: CongestionControl,
    keepalive: Option<Duration>,
    nodelay: bool,
) -> SocketReport {
    let socket = SockRef::from(stream);
    let congestion = match congestion {
        CongestionControl::System => OptionStatus::Applied,
        CongestionControl::Bbr => set_congestion(&socket, "bbr"),
        CongestionControl::Cubic => set_congestion(&socket, "cubic"),
        CongestionControl::Named(name) => set_congestion(&socket, &name),
    };
    let keepalive = match keepalive {
        Some(time) => match socket.set_tcp_keepalive(&TcpKeepalive::new().with_time(time)) {
            Ok(()) => OptionStatus::Applied,
            Err(error) => failure(error),
        },
        None => OptionStatus::Unsupported,
    };
    let nodelay = match socket.set_tcp_nodelay(nodelay) {
        Ok(()) => OptionStatus::Applied,
        Err(error) => failure(error),
    };
    SocketReport {
        congestion,
        keepalive,
        nodelay,
    }
}

#[cfg(target_os = "linux")]
fn set_congestion(socket: &SockRef<'_>, algorithm: &str) -> OptionStatus {
    match socket.set_tcp_congestion(algorithm.as_bytes()) {
        Ok(()) => OptionStatus::Applied,
        Err(error) => failure(error),
    }
}

#[cfg(not(target_os = "linux"))]
fn set_congestion(_socket: &SockRef<'_>, _algorithm: &str) -> OptionStatus {
    OptionStatus::Unsupported
}

#[cfg(target_os = "linux")]
pub fn set_tcp_fast_open_listener(stream: &TcpStream, queue_length: u32) -> io::Result<()> {
    use std::os::fd::AsRawFd;
    let socket = SockRef::from(stream);
    let queue_length = i32::try_from(queue_length).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "TCP Fast Open queue is too large",
        )
    })?;
    let result = unsafe {
        libc::setsockopt(
            socket.as_raw_fd(),
            libc::SOL_TCP,
            libc::TCP_FASTOPEN,
            (&queue_length as *const i32).cast(),
            std::mem::size_of_val(&queue_length) as libc::socklen_t,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(not(target_os = "linux"))]
pub fn set_tcp_fast_open_listener(_stream: &TcpStream, _queue_length: u32) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "TCP Fast Open is only available on Linux",
    ))
}

#[cfg(target_os = "linux")]
pub fn set_tcp_fast_open_connect(stream: &TcpStream) -> io::Result<()> {
    use std::os::fd::AsRawFd;
    let socket = SockRef::from(stream);
    let enabled: libc::c_int = 1;
    let result = unsafe {
        libc::setsockopt(
            socket.as_raw_fd(),
            libc::IPPROTO_TCP,
            libc::TCP_FASTOPEN_CONNECT,
            (&enabled as *const libc::c_int).cast(),
            std::mem::size_of_val(&enabled) as libc::socklen_t,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(not(target_os = "linux"))]
pub fn set_tcp_fast_open_connect(_stream: &TcpStream) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "TCP Fast Open is only available on Linux",
    ))
}

/// 高包速 UDP（QUIC carrier、UDP 入口）的内核收发缓冲。默认约 208KB，1Gbps 下不到 2ms 即溢出丢包。
/// 内核缓冲只在积压时占用。优先 `SO_*BUFFORCE`（root 可越过 `net.core.*mem_max`），否则退回普通设置。
#[cfg(target_os = "linux")]
pub fn set_udp_buffers<S: std::os::fd::AsFd>(socket: &S, bytes: usize) {
    use std::os::fd::AsRawFd;
    let socket = SockRef::from(socket);
    let size = libc::c_int::try_from(bytes).unwrap_or(libc::c_int::MAX);
    for (force, fallback) in [
        (libc::SO_RCVBUFFORCE, libc::SO_RCVBUF),
        (libc::SO_SNDBUFFORCE, libc::SO_SNDBUF),
    ] {
        for name in [force, fallback] {
            // SAFETY: fd 有效，选项值指向栈上的 c_int。
            let result = unsafe {
                libc::setsockopt(
                    socket.as_raw_fd(),
                    libc::SOL_SOCKET,
                    name,
                    (&size as *const libc::c_int).cast(),
                    std::mem::size_of_val(&size) as libc::socklen_t,
                )
            };
            if result == 0 {
                break;
            }
        }
    }
}

#[cfg(all(unix, not(target_os = "linux")))]
pub fn set_udp_buffers<S: std::os::fd::AsFd>(socket: &S, bytes: usize) {
    let socket = SockRef::from(socket);
    let _ = socket.set_recv_buffer_size(bytes);
    let _ = socket.set_send_buffer_size(bytes);
}

#[cfg(not(unix))]
pub fn set_udp_buffers<S>(_socket: &S, _bytes: usize) {}

fn failure(error: io::Error) -> OptionStatus {
    if error.kind() == io::ErrorKind::Unsupported {
        OptionStatus::Unsupported
    } else {
        OptionStatus::Failed(error.to_string())
    }
}
