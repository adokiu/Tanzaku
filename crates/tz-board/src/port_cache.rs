use redis::{Script, aio::ConnectionManager};
use thiserror::Error;
use tz_common::ports::PortRanges;
use uuid::Uuid;

const BITMAP_BYTES: usize = 1 << 13;

const RESERVE_CUSTOM: &str = r#"
if redis.call('GET', KEYS[3]) ~= '1' then return -1 end
if redis.call('GETBIT', KEYS[1], ARGV[1]) ~= 0 then return 0 end
redis.call('SETBIT', KEYS[1], ARGV[1], 1)
redis.call('HSET', KEYS[2], ARGV[1], ARGV[2])
return 1
"#;

const RESERVE_RANDOM: &str = r#"
if redis.call('GET', KEYS[3]) ~= '1' then return -1 end
local total = 0
for i = 4, #ARGV, 2 do
    total = total + tonumber(ARGV[i]) - tonumber(ARGV[i - 1]) + 1
end
if total == 0 then return 0 end
local start = tonumber(ARGV[1]) % total
local token = ARGV[2]
for offset = 0, total - 1 do
    local remaining = (start + offset) % total
    local port = 0
    for i = 4, #ARGV, 2 do
        local first = tonumber(ARGV[i - 1])
        local length = tonumber(ARGV[i]) - first + 1
        if remaining < length then
            port = first + remaining
            break
        end
        remaining = remaining - length
    end
    if redis.call('GETBIT', KEYS[1], port) == 0 then
        redis.call('SETBIT', KEYS[1], port, 1)
        redis.call('HSET', KEYS[2], port, token)
        return port
    end
end
return 0
"#;

const FINALIZE_RESERVATION: &str = r#"
if redis.call('HGET', KEYS[1], ARGV[1]) ~= ARGV[2] then return 0 end
redis.call('HDEL', KEYS[1], ARGV[1])
return 1
"#;

const ROLLBACK_RESERVATION: &str = r#"
if redis.call('HGET', KEYS[2], ARGV[1]) ~= ARGV[2] then return 0 end
redis.call('HDEL', KEYS[2], ARGV[1])
redis.call('SETBIT', KEYS[1], ARGV[1], 0)
return 1
"#;

const RELEASE_PORT: &str = r#"
redis.call('HDEL', KEYS[2], ARGV[1])
redis.call('SETBIT', KEYS[1], ARGV[1], 0)
return 1
"#;

const REBUILD_NODE: &str = r#"
redis.call('RENAME', KEYS[1], KEYS[2])
redis.call('DEL', KEYS[3])
redis.call('SET', KEYS[4], '1')
return 1
"#;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortProtocol {
    Tcp,
    Udp,
}

impl PortProtocol {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Tcp => "tcp",
            Self::Udp => "udp",
        }
    }
}

#[derive(Clone)]
pub struct RedisPortCache {
    connection: ConnectionManager,
}

#[derive(Debug, Error)]
pub enum PortCacheError {
    #[error("Redis port cache is not ready")]
    NotReady,
    #[error("Redis command failed")]
    Redis(#[from] redis::RedisError),
    #[error("random port selection failed")]
    Randomness(#[from] getrandom::Error),
}

impl RedisPortCache {
    pub fn new(connection: ConnectionManager) -> Self {
        Self { connection }
    }

    pub async fn is_available(
        &self,
        node_id: Uuid,
        protocol: PortProtocol,
        port: u16,
    ) -> Result<Option<bool>, PortCacheError> {
        let (bitmap, _, ready) = self.keys(node_id, protocol);
        let mut connection = self.connection.clone();
        let ready: Option<String> = redis::cmd("GET")
            .arg(ready)
            .query_async(&mut connection)
            .await?;
        if ready.as_deref() != Some("1") {
            return Ok(None);
        }
        let occupied: i64 = redis::cmd("GETBIT")
            .arg(bitmap)
            .arg(port)
            .query_async(&mut connection)
            .await?;
        Ok(Some(occupied == 0))
    }

    pub async fn reserve_custom(
        &self,
        node_id: Uuid,
        protocol: PortProtocol,
        port: u16,
        reservation: Uuid,
    ) -> Result<bool, PortCacheError> {
        let (bitmap, reservations, ready) = self.keys(node_id, protocol);
        let mut connection = self.connection.clone();
        let result: i64 = Script::new(RESERVE_CUSTOM)
            .key(bitmap)
            .key(reservations)
            .key(ready)
            .arg(port)
            .arg(reservation.to_string())
            .invoke_async(&mut connection)
            .await?;
        match result {
            1 => Ok(true),
            0 => Ok(false),
            _ => Err(PortCacheError::NotReady),
        }
    }

    pub async fn reserve_random(
        &self,
        node_id: Uuid,
        protocol: PortProtocol,
        ranges: &PortRanges,
        reservation: Uuid,
    ) -> Result<Option<u16>, PortCacheError> {
        let mut start = [0u8; 8];
        getrandom::fill(&mut start)?;
        let start = u64::from_ne_bytes(start);
        let (bitmap, reservations, ready) = self.keys(node_id, protocol);
        let script = Script::new(RESERVE_RANDOM);
        let mut invocation = script.prepare_invoke();
        invocation
            .key(bitmap)
            .key(reservations)
            .key(ready)
            .arg(start)
            .arg(reservation.to_string());
        for range in ranges.ranges() {
            invocation.arg(range.start).arg(range.end);
        }
        let mut connection = self.connection.clone();
        let result: i64 = invocation.invoke_async(&mut connection).await?;
        match result {
            -1 => Err(PortCacheError::NotReady),
            0 => Ok(None),
            port @ 1..=65_535 => Ok(Some(port as u16)),
            _ => Ok(None),
        }
    }

    pub async fn commit_reservation(
        &self,
        node_id: Uuid,
        protocol: PortProtocol,
        port: u16,
        reservation: Uuid,
    ) -> Result<bool, PortCacheError> {
        let (_, reservations, _) = self.keys(node_id, protocol);
        let mut connection = self.connection.clone();
        let result: i64 = Script::new(FINALIZE_RESERVATION)
            .key(reservations)
            .arg(port)
            .arg(reservation.to_string())
            .invoke_async(&mut connection)
            .await?;
        Ok(result == 1)
    }

    pub async fn rollback_reservation(
        &self,
        node_id: Uuid,
        protocol: PortProtocol,
        port: u16,
        reservation: Uuid,
    ) -> Result<bool, PortCacheError> {
        let (bitmap, reservations, _) = self.keys(node_id, protocol);
        let mut connection = self.connection.clone();
        let result: i64 = Script::new(ROLLBACK_RESERVATION)
            .key(bitmap)
            .key(reservations)
            .arg(port)
            .arg(reservation.to_string())
            .invoke_async(&mut connection)
            .await?;
        Ok(result == 1)
    }

    pub async fn invalidate(
        &self,
        node_id: Uuid,
        protocol: PortProtocol,
    ) -> Result<(), PortCacheError> {
        let (_, _, ready) = self.keys(node_id, protocol);
        let mut connection = self.connection.clone();
        let _: usize = redis::cmd("DEL")
            .arg(ready)
            .query_async(&mut connection)
            .await?;
        Ok(())
    }

    pub async fn release(
        &self,
        node_id: Uuid,
        protocol: PortProtocol,
        port: u16,
    ) -> Result<(), PortCacheError> {
        let (bitmap, reservations, _) = self.keys(node_id, protocol);
        let mut connection = self.connection.clone();
        let _: i64 = Script::new(RELEASE_PORT)
            .key(bitmap)
            .key(reservations)
            .arg(port)
            .invoke_async(&mut connection)
            .await?;
        Ok(())
    }

    pub async fn rebuild_node(
        &self,
        node_id: Uuid,
        tcp_ranges: &PortRanges,
        udp_ranges: &PortRanges,
        exclusions: &PortRanges,
        tcp_reserved: &[u16],
        udp_reserved: &[u16],
        tcp_tunnels: &[u16],
        udp_tunnels: &[u16],
    ) -> Result<(), PortCacheError> {
        let tcp = make_bitmap(tcp_ranges, exclusions, tcp_reserved, tcp_tunnels);
        let udp = make_bitmap(udp_ranges, exclusions, udp_reserved, udp_tunnels);
        self.rebuild_protocol(node_id, PortProtocol::Tcp, tcp)
            .await?;
        self.rebuild_protocol(node_id, PortProtocol::Udp, udp)
            .await?;
        Ok(())
    }

    fn keys(&self, node_id: Uuid, protocol: PortProtocol) -> (String, String, String) {
        let prefix = format!("ports:{{{node_id}}}:{}", protocol.name());
        (
            format!("{prefix}:bitmap"),
            format!("{prefix}:reservations"),
            format!("{prefix}:ready"),
        )
    }

    async fn rebuild_protocol(
        &self,
        node_id: Uuid,
        protocol: PortProtocol,
        bitmap: Vec<u8>,
    ) -> Result<(), PortCacheError> {
        let (target, reservations, ready) = self.keys(node_id, protocol);
        let staging = format!("{target}:rebuild:{}", Uuid::new_v4());
        let mut connection = self.connection.clone();
        let _: String = redis::cmd("SET")
            .arg(&staging)
            .arg(bitmap.as_slice())
            .query_async(&mut connection)
            .await?;
        let _: i64 = Script::new(REBUILD_NODE)
            .key(staging)
            .key(target)
            .key(reservations)
            .key(ready)
            .invoke_async(&mut connection)
            .await?;
        Ok(())
    }
}

fn make_bitmap(
    ranges: &PortRanges,
    exclusions: &PortRanges,
    fixed: &[u16],
    tunnels: &[u16],
) -> Vec<u8> {
    let mut bitmap = vec![u8::MAX; BITMAP_BYTES];
    set_port(&mut bitmap, 0, true);
    for range in ranges.ranges() {
        for port in range.start..=range.end {
            set_port(&mut bitmap, port, false);
        }
    }
    for range in exclusions.ranges() {
        for port in range.start..=range.end {
            set_port(&mut bitmap, port, true);
        }
    }
    for port in fixed.iter().chain(tunnels) {
        set_port(&mut bitmap, *port, true);
    }
    bitmap
}

fn set_port(bitmap: &mut [u8], port: u16, unavailable: bool) {
    let byte = &mut bitmap[usize::from(port) / 8];
    let mask = 1 << (7 - usize::from(port) % 8);
    if unavailable {
        *byte |= mask;
    } else {
        *byte &= !mask;
    }
}

pub fn reservation_token() -> Uuid {
    Uuid::new_v4()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn bit(bitmap: &[u8], port: u16) -> bool {
        bitmap[usize::from(port) / 8] & (1 << (7 - usize::from(port) % 8)) != 0
    }

    #[test]
    fn rebuild_bitmap_reserves_out_of_range_exclusions_and_active_ports() {
        let ranges = PortRanges::from_str("20000-20003,30000").unwrap();
        let excludes = PortRanges::from_str("20001").unwrap();
        let bitmap = make_bitmap(&ranges, &excludes, &[20002, 80], &[30000]);
        assert_eq!(bitmap.len(), 8192);
        assert!(bit(&bitmap, 0));
        assert!(!bit(&bitmap, 20000));
        assert!(bit(&bitmap, 20001));
        assert!(bit(&bitmap, 20002));
        assert!(!bit(&bitmap, 20003));
        assert!(bit(&bitmap, 80));
        assert!(bit(&bitmap, 30000));
        assert!(bit(&bitmap, 65535));
    }
}
