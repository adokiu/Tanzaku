use std::net::SocketAddr;

pub fn inject_forwarded_headers(buffer: &mut [u8], total: usize, peer: SocketAddr) -> usize {
    let Ok(text) = std::str::from_utf8(&buffer[..total]) else {
        return total;
    };
    let Some(header_end) = text.find("\r\n\r\n") else {
        return total;
    };
    let (headers, rest) = text.split_at(header_end);
    let mut lines: Vec<String> = headers.lines().map(str::to_string).collect();
    if lines.is_empty() {
        return total;
    }
    let peer_ip = peer.ip().to_string();
    if !lines
        .iter()
        .any(|line| line.to_ascii_lowercase().starts_with("x-forwarded-for:"))
    {
        lines.push(format!("X-Forwarded-For: {peer_ip}"));
    }
    if !lines
        .iter()
        .any(|line| line.to_ascii_lowercase().starts_with("x-forwarded-proto:"))
    {
        lines.push("X-Forwarded-Proto: http".into());
    }
    if !lines
        .iter()
        .any(|line| line.to_ascii_lowercase().starts_with("x-forwarded-host:"))
    {
        if let Some(host_line) = lines
            .iter()
            .find(|line| line.to_ascii_lowercase().starts_with("host:"))
        {
            lines.push(format!("X-Forwarded-Host: {}", host_line.splitn(2, ':').nth(1).unwrap_or("").trim()));
        }
    }
    let rebuilt = format!("{}\r\n{}", lines.join("\r\n"), rest);
    let bytes = rebuilt.as_bytes();
    if bytes.len() > buffer.len() {
        return total;
    }
    buffer[..bytes.len()].copy_from_slice(bytes);
    bytes.len()
}
