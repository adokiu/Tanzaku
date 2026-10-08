use std::time::Instant;
use sysinfo::{Networks, System};
use tz_proto::HostMetricsReport;

/// 与 Komari agent 一致：累计字节来自各网卡 rx/tx，速率由两次采样差分得到。
pub struct HostMetricsSampler {
    system: System,
    #[cfg(not(target_os = "linux"))]
    networks: Networks,
    last_net_rx: u64,
    last_net_tx: u64,
    last_sample: Instant,
    net_primed: bool,
}

impl HostMetricsSampler {
    pub fn new() -> Self {
        let mut system = System::new_all();
        system.refresh_cpu_usage();
        system.refresh_memory();
        #[cfg(not(target_os = "linux"))]
        let mut networks = Networks::new_with_refreshed_list();
        #[cfg(not(target_os = "linux"))]
        networks.refresh(false);
        let (rx, tx) = read_network_byte_totals();
        Self {
            system,
            #[cfg(not(target_os = "linux"))]
            networks,
            last_net_rx: rx,
            last_net_tx: tx,
            last_sample: Instant::now(),
            net_primed: false,
        }
    }

    pub fn sample(&mut self) -> HostMetricsReport {
        let elapsed = self.last_sample.elapsed();
        self.last_sample = Instant::now();

        self.system.refresh_cpu_usage();
        self.system.refresh_memory();
        #[cfg(not(target_os = "linux"))]
        {
            self.networks.refresh(false);
        }

        let cpu_usage_percent = self.system.global_cpu_usage().clamp(0.0, 100.0) as u8;
        let cpu_cores = u16::try_from(self.system.cpus().len().max(1)).unwrap_or(1);
        let memory_total_bytes = self.system.total_memory();
        let memory_used_bytes = self.system.used_memory();

        let (rx, tx) = read_network_byte_totals();
        let secs = elapsed.as_secs_f64().max(0.001);
        let (net_down_bps, net_up_bps) = if self.net_primed {
            (
                byte_delta_per_sec(rx, self.last_net_rx, secs),
                byte_delta_per_sec(tx, self.last_net_tx, secs),
            )
        } else {
            self.net_primed = true;
            (0, 0)
        };
        self.last_net_rx = rx;
        self.last_net_tx = tx;

        let (connections_tcp, connections_udp) = connections_now();

        HostMetricsReport {
            cpu_usage_percent,
            cpu_cores,
            memory_used_bytes,
            memory_total_bytes,
            net_up_bps,
            net_down_bps,
            net_rx_bytes_total: rx,
            net_tx_bytes_total: tx,
            connections_tcp,
            connections_udp,
        }
    }
}

/// 当前连接快照，每次采样覆盖，不累计。
/// TCP 只计 `st == 01`（ESTABLISHED）。把 TIME_WAIT/CLOSE_WAIT 算进去会只涨不掉。
fn connections_now() -> (u32, u32) {
    #[cfg(target_os = "linux")]
    {
        let tcp = count_tcp_established(&["tcp", "tcp6"]);
        let udp = count_udp_sockets(&["udp", "udp6"]);
        return (tcp, udp);
    }
    #[cfg(not(target_os = "linux"))]
    (0, 0)
}

#[cfg(target_os = "linux")]
fn count_tcp_established(names: &[&str]) -> u32 {
    let mut total = 0_u32;
    for name in names {
        let Ok(data) = std::fs::read_to_string(format!("/proc/net/{name}")) else {
            continue;
        };
        for line in data.lines().skip(1) {
            let mut fields = line.split_whitespace();
            let _index = fields.next();
            let _local = fields.next();
            let _remote = fields.next();
            // /proc/net/tcp 第 4 列：01 ESTABLISHED
            if fields.next().is_some_and(|state| state.eq_ignore_ascii_case("01")) {
                total = total.saturating_add(1);
            }
        }
    }
    total
}

#[cfg(target_os = "linux")]
fn count_udp_sockets(names: &[&str]) -> u32 {
    let mut total = 0_u32;
    for name in names {
        let Ok(data) = std::fs::read_to_string(format!("/proc/net/{name}")) else {
            continue;
        };
        for line in data.lines().skip(1) {
            if !line.trim().is_empty() {
                total = total.saturating_add(1);
            }
        }
    }
    total
}

fn byte_delta_per_sec(current: u64, previous: u64, secs: f64) -> u64 {
    let delta = current.saturating_sub(previous);
    (delta as f64 / secs).max(0.0) as u64
}

fn read_network_byte_totals() -> (u64, u64) {
    #[cfg(target_os = "linux")]
    {
        if let Some(totals) = linux_proc_net_dev_totals() {
            return totals;
        }
    }
    sysinfo_network_byte_totals()
}

#[cfg(target_os = "linux")]
fn linux_proc_net_dev_totals() -> Option<(u64, u64)> {
    use std::fs;

    let data = fs::read_to_string("/proc/net/dev").ok()?;
    let mut rx = 0_u64;
    let mut tx = 0_u64;
    for line in data.lines().skip(2) {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let (iface_part, rest) = line.split_once(':')?;
        let iface = iface_part.trim();
        if should_skip_interface(iface) {
            continue;
        }
        let fields: Vec<&str> = rest.split_whitespace().collect();
        if fields.len() < 9 {
            continue;
        }
        let rx_bytes: u64 = fields[0].parse().ok()?;
        let tx_bytes: u64 = fields[8].parse().ok()?;
        rx = rx.saturating_add(rx_bytes);
        tx = tx.saturating_add(tx_bytes);
    }
    Some((rx, tx))
}

#[cfg(not(target_os = "linux"))]
fn sysinfo_network_byte_totals() -> (u64, u64) {
    let mut networks = Networks::new_with_refreshed_list();
    networks.refresh(false);
    sum_network_interfaces(&networks)
}

#[cfg(target_os = "linux")]
fn sysinfo_network_byte_totals() -> (u64, u64) {
    let mut networks = Networks::new_with_refreshed_list();
    networks.refresh(false);
    sum_network_interfaces(&networks)
}

fn sum_network_interfaces(networks: &Networks) -> (u64, u64) {
    let mut rx = 0_u64;
    let mut tx = 0_u64;
    for (name, data) in networks {
        if should_skip_interface(name) {
            continue;
        }
        rx = rx.saturating_add(data.received());
        tx = tx.saturating_add(data.transmitted());
    }
    (rx, tx)
}

fn should_skip_interface(name: &str) -> bool {
    let lower = name.trim().to_ascii_lowercase();
    lower == "lo" || lower.starts_with("lo.")
}
