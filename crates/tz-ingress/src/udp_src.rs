//! 多 IP 节点上 UDP 监听 `0.0.0.0` 时，回包源地址必须等于访客当初发往的本机 IP，
//! 否则访客侧已 `connect()` 的 UDP socket（如 iperf3、DNS 客户端）会丢弃回包。
//! Linux 用 `IP_PKTINFO` / `IPV6_PKTINFO` 收取目的地址并在回包时指定源地址；其他平台退化为普通收发。

use std::{
    io,
    net::{IpAddr, SocketAddr},
};

use tokio::net::UdpSocket;

#[cfg(target_os = "linux")]
mod imp {
    use super::*;
    use std::{
        mem,
        net::{Ipv4Addr, Ipv6Addr, SocketAddrV4, SocketAddrV6},
        os::fd::AsRawFd,
        ptr,
    };

    #[repr(align(8))]
    struct ControlBuffer([u8; 128]);

    fn set_flag(fd: libc::c_int, level: libc::c_int, name: libc::c_int) {
        let enabled: libc::c_int = 1;
        // SAFETY: fd 有效，选项值指向栈上的 c_int。
        unsafe {
            libc::setsockopt(
                fd,
                level,
                name,
                (&enabled as *const libc::c_int).cast(),
                mem::size_of_val(&enabled) as libc::socklen_t,
            );
        }
    }

    pub fn enable(socket: &UdpSocket) {
        let fd = socket.as_raw_fd();
        set_flag(fd, libc::IPPROTO_IP, libc::IP_PKTINFO);
        set_flag(fd, libc::IPPROTO_IPV6, libc::IPV6_RECVPKTINFO);
    }

    fn to_socket_addr(storage: &libc::sockaddr_storage) -> io::Result<SocketAddr> {
        match libc::c_int::from(storage.ss_family) {
            libc::AF_INET => {
                // SAFETY: family 为 AF_INET 时内核写入的是 sockaddr_in。
                let addr = unsafe { &*(storage as *const libc::sockaddr_storage).cast::<libc::sockaddr_in>() };
                Ok(SocketAddr::V4(SocketAddrV4::new(
                    Ipv4Addr::from(u32::from_be(addr.sin_addr.s_addr)),
                    u16::from_be(addr.sin_port),
                )))
            }
            libc::AF_INET6 => {
                // SAFETY: family 为 AF_INET6 时内核写入的是 sockaddr_in6。
                let addr = unsafe { &*(storage as *const libc::sockaddr_storage).cast::<libc::sockaddr_in6>() };
                Ok(SocketAddr::V6(SocketAddrV6::new(
                    Ipv6Addr::from(addr.sin6_addr.s6_addr),
                    u16::from_be(addr.sin6_port),
                    addr.sin6_flowinfo,
                    addr.sin6_scope_id,
                )))
            }
            _ => Err(io::Error::new(io::ErrorKind::InvalidData, "unsupported sockaddr family")),
        }
    }

    fn recv_once(socket: &UdpSocket, buf: &mut [u8]) -> io::Result<(usize, SocketAddr, Option<IpAddr>)> {
        let mut iov = libc::iovec {
            iov_base: buf.as_mut_ptr().cast(),
            iov_len: buf.len(),
        };
        // SAFETY: 全零的 sockaddr_storage / msghdr 是合法初值。
        let mut name: libc::sockaddr_storage = unsafe { mem::zeroed() };
        let mut control = ControlBuffer([0; 128]);
        let mut msg: libc::msghdr = unsafe { mem::zeroed() };
        msg.msg_name = (&mut name as *mut libc::sockaddr_storage).cast();
        msg.msg_namelen = mem::size_of::<libc::sockaddr_storage>() as libc::socklen_t;
        msg.msg_iov = &mut iov;
        msg.msg_iovlen = 1;
        msg.msg_control = control.0.as_mut_ptr().cast();
        msg.msg_controllen = control.0.len() as _;

        // SAFETY: msg 中的指针都指向本函数栈上存活的缓冲区。
        let len = unsafe { libc::recvmsg(socket.as_raw_fd(), &mut msg, 0) };
        if len < 0 {
            return Err(io::Error::last_os_error());
        }
        let peer = to_socket_addr(&name)?;
        let mut local_ip = None;
        // SAFETY: 按 CMSG_* 宏遍历内核填充的控制消息。
        unsafe {
            let mut cmsg = libc::CMSG_FIRSTHDR(&msg);
            while !cmsg.is_null() {
                let header = &*cmsg;
                if header.cmsg_level == libc::IPPROTO_IP && header.cmsg_type == libc::IP_PKTINFO {
                    let info = &*libc::CMSG_DATA(cmsg).cast::<libc::in_pktinfo>();
                    local_ip = Some(IpAddr::V4(Ipv4Addr::from(u32::from_be(info.ipi_addr.s_addr))));
                } else if header.cmsg_level == libc::IPPROTO_IPV6
                    && header.cmsg_type == libc::IPV6_PKTINFO
                {
                    let info = &*libc::CMSG_DATA(cmsg).cast::<libc::in6_pktinfo>();
                    local_ip = Some(IpAddr::V6(Ipv6Addr::from(info.ipi6_addr.s6_addr)));
                }
                cmsg = libc::CMSG_NXTHDR(&msg, cmsg);
            }
        }
        Ok((len as usize, peer, local_ip))
    }

    pub async fn recv(socket: &UdpSocket, buf: &mut [u8]) -> io::Result<(usize, SocketAddr, Option<IpAddr>)> {
        socket
            .async_io(tokio::io::Interest::READABLE, || loop {
                match recv_once(socket, buf) {
                    Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
                    other => break other,
                }
            })
            .await
    }

    fn send_once(socket: &UdpSocket, buf: &[u8], peer: SocketAddr, local_ip: IpAddr) -> io::Result<usize> {
        let peer_addr = socket2::SockAddr::from(peer);
        let mut iov = libc::iovec {
            iov_base: buf.as_ptr() as *mut libc::c_void,
            iov_len: buf.len(),
        };
        let mut control = ControlBuffer([0; 128]);
        // SAFETY: 全零 msghdr 是合法初值。
        let mut msg: libc::msghdr = unsafe { mem::zeroed() };
        msg.msg_name = peer_addr.as_ptr() as *mut libc::c_void;
        msg.msg_namelen = peer_addr.len();
        msg.msg_iov = &mut iov;
        msg.msg_iovlen = 1;
        msg.msg_control = control.0.as_mut_ptr().cast();

        let source = match local_ip {
            IpAddr::V4(ip) => IpAddr::V4(ip),
            IpAddr::V6(ip) => ip.to_ipv4_mapped().map_or(IpAddr::V6(ip), IpAddr::V4),
        };
        // SAFETY: 控制缓冲区 128 字节足够容纳单个 in_pktinfo / in6_pktinfo 消息。
        unsafe {
            match source {
                IpAddr::V4(src) => {
                    let space = libc::CMSG_SPACE(mem::size_of::<libc::in_pktinfo>() as u32) as usize;
                    msg.msg_controllen = space as _;
                    let cmsg = libc::CMSG_FIRSTHDR(&msg);
                    (*cmsg).cmsg_level = libc::IPPROTO_IP;
                    (*cmsg).cmsg_type = libc::IP_PKTINFO;
                    (*cmsg).cmsg_len = libc::CMSG_LEN(mem::size_of::<libc::in_pktinfo>() as u32) as _;
                    ptr::write(
                        libc::CMSG_DATA(cmsg).cast::<libc::in_pktinfo>(),
                        libc::in_pktinfo {
                            ipi_ifindex: 0,
                            ipi_spec_dst: libc::in_addr { s_addr: u32::from(src).to_be() },
                            ipi_addr: libc::in_addr { s_addr: 0 },
                        },
                    );
                }
                IpAddr::V6(src) => {
                    let space = libc::CMSG_SPACE(mem::size_of::<libc::in6_pktinfo>() as u32) as usize;
                    msg.msg_controllen = space as _;
                    let cmsg = libc::CMSG_FIRSTHDR(&msg);
                    (*cmsg).cmsg_level = libc::IPPROTO_IPV6;
                    (*cmsg).cmsg_type = libc::IPV6_PKTINFO;
                    (*cmsg).cmsg_len = libc::CMSG_LEN(mem::size_of::<libc::in6_pktinfo>() as u32) as _;
                    ptr::write(
                        libc::CMSG_DATA(cmsg).cast::<libc::in6_pktinfo>(),
                        libc::in6_pktinfo {
                            ipi6_addr: libc::in6_addr { s6_addr: src.octets() },
                            ipi6_ifindex: 0,
                        },
                    );
                }
            }
            let sent = libc::sendmsg(socket.as_raw_fd(), &msg, 0);
            if sent < 0 {
                Err(io::Error::last_os_error())
            } else {
                Ok(sent as usize)
            }
        }
    }

    pub async fn send(socket: &UdpSocket, buf: &[u8], peer: SocketAddr, local_ip: Option<IpAddr>) -> io::Result<usize> {
        let Some(local_ip) = local_ip.filter(|ip| !ip.is_unspecified()) else {
            return socket.send_to(buf, peer).await;
        };
        socket
            .async_io(tokio::io::Interest::WRITABLE, || send_once(socket, buf, peer, local_ip))
            .await
    }
}

#[cfg(not(target_os = "linux"))]
mod imp {
    use super::*;

    pub fn enable(_socket: &UdpSocket) {}

    pub async fn recv(socket: &UdpSocket, buf: &mut [u8]) -> io::Result<(usize, SocketAddr, Option<IpAddr>)> {
        let (len, peer) = socket.recv_from(buf).await?;
        Ok((len, peer, None))
    }

    pub async fn send(socket: &UdpSocket, buf: &[u8], peer: SocketAddr, _local_ip: Option<IpAddr>) -> io::Result<usize> {
        socket.send_to(buf, peer).await
    }
}

/// 开启目的地址回传；绑定具体 IP 时无副作用。
pub fn enable_local_ip_info(socket: &UdpSocket) {
    imp::enable(socket);
}

/// 返回 (长度, 访客地址, 访客发往的本机 IP)。
pub async fn recv_with_local_ip(
    socket: &UdpSocket,
    buf: &mut [u8],
) -> io::Result<(usize, SocketAddr, Option<IpAddr>)> {
    imp::recv(socket, buf).await
}

/// 以 `local_ip` 为源地址回包；`None` 时由内核选路。
pub async fn send_from_local_ip(
    socket: &UdpSocket,
    buf: &[u8],
    peer: SocketAddr,
    local_ip: Option<IpAddr>,
) -> io::Result<usize> {
    imp::send(socket, buf, peer, local_ip).await
}
