//! Linux IP ACL → nftables/iptables；策略/端口变化或进程退出时清理。

use serde_json::Value;
use std::{
    process::Command,
    sync::atomic::{AtomicBool, Ordering},
};
use tracing::{debug, info, warn};

const NFT_TABLE: &str = "tanzaku_acl";
const IPT_CHAIN: &str = "TANZAKU_ACL";

static INSTALLED: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IpAclFirewall {
    pub enabled: bool,
    pub allow: Vec<String>,
    pub deny: Vec<String>,
}

pub fn parse_ip_acl(policy: &Value) -> IpAclFirewall {
    let Some(cfg) = policy.get("ip_acl") else {
        return IpAclFirewall::default();
    };
    if cfg.get("enabled").and_then(Value::as_bool) == Some(false) {
        return IpAclFirewall::default();
    }
    let parse = |key: &str| -> Vec<String> {
        cfg.get(key)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::trim)
            .filter(|cidr| !cidr.is_empty())
            .map(str::to_string)
            .collect()
    };
    IpAclFirewall {
        enabled: true,
        allow: parse("allow"),
        deny: parse("deny"),
    }
}

pub fn sync(acl: &IpAclFirewall, tcp_ports: &[u16], udp_ports: &[u16]) {
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (acl, tcp_ports, udp_ports);
        return;
    }
    #[cfg(target_os = "linux")]
    {
        let need = acl.enabled
            && (!acl.allow.is_empty() || !acl.deny.is_empty())
            && (!tcp_ports.is_empty() || !udp_ports.is_empty());
        if !need {
            if INSTALLED.swap(false, Ordering::SeqCst) {
                clear_all();
                info!("ip acl firewall cleared (policy or ports empty)");
            }
            return;
        }
        if sync_nft(acl, tcp_ports, udp_ports) {
            INSTALLED.store(true, Ordering::SeqCst);
            return;
        }
        if sync_iptables(acl, tcp_ports, udp_ports) {
            INSTALLED.store(true, Ordering::SeqCst);
            return;
        }
        warn!("ip acl firewall sync skipped: nft/iptables unavailable or permission denied");
    }
}

/// 进程退出时调用，移除本进程安装的规则。
pub fn teardown() {
    #[cfg(target_os = "linux")]
    {
        if INSTALLED.swap(false, Ordering::SeqCst) {
            clear_all();
            info!("ip acl firewall rules removed on shutdown");
        }
    }
}

#[cfg(target_os = "linux")]
pub fn clear_all() {
    let _ = run_ok("nft", &["delete", "table", "inet", NFT_TABLE]);
    clear_iptables_chain("iptables");
    clear_iptables_chain("ip6tables");
}

#[cfg(not(target_os = "linux"))]
pub fn clear_all() {}

#[cfg(target_os = "linux")]
fn sync_nft(acl: &IpAclFirewall, tcp_ports: &[u16], udp_ports: &[u16]) -> bool {
    if !command_exists("nft") {
        return false;
    }
    let _ = run_ok("nft", &["delete", "table", "inet", NFT_TABLE]);
    let mut rules = String::new();
    rules.push_str(&format!("table inet {NFT_TABLE} {{\n"));
    rules.push_str("  chain input {\n");
    rules.push_str("    type filter hook input priority -150; policy accept;\n");
    append_nft_family(&mut rules, "ip", &acl.deny, &acl.allow, tcp_ports, udp_ports);
    append_nft_family(&mut rules, "ip6", &acl.deny, &acl.allow, tcp_ports, udp_ports);
    rules.push_str("  }\n}\n");
    match run_stdin("nft", &["-f", "-"], &rules) {
        Ok(()) => {
            info!(
                tcp_ports = tcp_ports.len(),
                udp_ports = udp_ports.len(),
                "ip acl synced to nftables"
            );
            true
        }
        Err(err) => {
            debug!(%err, "nftables sync failed");
            false
        }
    }
}

#[cfg(target_os = "linux")]
fn append_nft_family(
    out: &mut String,
    family: &str,
    deny: &[String],
    allow: &[String],
    tcp_ports: &[u16],
    udp_ports: &[u16],
) {
    let deny_nets = filter_cidrs_for_family(deny, family == "ip6");
    let allow_nets = filter_cidrs_for_family(allow, family == "ip6");
    let saddr = if family == "ip6" { "ip6 saddr" } else { "ip saddr" };
    for proto in ["tcp", "udp"] {
        let ports = if proto == "tcp" { tcp_ports } else { udp_ports };
        if ports.is_empty() {
            continue;
        }
        let port_set = ports
            .iter()
            .map(|port| port.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        for cidr in &deny_nets {
            out.push_str(&format!("    {saddr} {cidr} {proto} dport {{ {port_set} }} drop\n"));
        }
        if !allow_nets.is_empty() {
            for cidr in &allow_nets {
                out.push_str(&format!(
                    "    {saddr} {cidr} {proto} dport {{ {port_set} }} accept\n"
                ));
            }
            out.push_str(&format!("    {proto} dport {{ {port_set} }} drop\n"));
        }
    }
}

#[cfg(target_os = "linux")]
fn filter_cidrs_for_family(cidrs: &[String], want_v6: bool) -> Vec<String> {
    cidrs
        .iter()
        .filter(|cidr| cidr.contains(':') == want_v6)
        .cloned()
        .collect()
}

#[cfg(target_os = "linux")]
fn sync_iptables(acl: &IpAclFirewall, tcp_ports: &[u16], udp_ports: &[u16]) -> bool {
    if !command_exists("iptables") {
        return false;
    }
    let ok4 = sync_iptables_bin("iptables", true, acl, tcp_ports, udp_ports);
    let ok6 = if command_exists("ip6tables") {
        sync_iptables_bin("ip6tables", true, acl, tcp_ports, udp_ports)
    } else {
        true
    };
    ok4 || ok6
}

#[cfg(target_os = "linux")]
fn sync_iptables_bin(
    bin: &str,
    v6: bool,
    acl: &IpAclFirewall,
    tcp_ports: &[u16],
    udp_ports: &[u16],
) -> bool {
    clear_iptables_chain(bin);
    if run_ok(bin, &["-N", IPT_CHAIN]).is_err() && run_ok(bin, &["-F", IPT_CHAIN]).is_err() {
        return false;
    }
    if run_ok(bin, &["-I", "INPUT", "1", "-j", IPT_CHAIN]).is_err() {
        return false;
    }
    let deny = filter_cidrs_for_family(&acl.deny, v6);
    let allow = filter_cidrs_for_family(&acl.allow, v6);
    for (proto, ports) in [("tcp", tcp_ports), ("udp", udp_ports)] {
        for &port in ports {
            for cidr in &deny {
                let _ = run_ok(
                    bin,
                    &[
                        "-A",
                        IPT_CHAIN,
                        "-s",
                        cidr,
                        "-p",
                        proto,
                        "--dport",
                        &port.to_string(),
                        "-j",
                        "DROP",
                    ],
                );
            }
            if !allow.is_empty() {
                for cidr in &allow {
                    let _ = run_ok(
                        bin,
                        &[
                            "-A",
                            IPT_CHAIN,
                            "-s",
                            cidr,
                            "-p",
                            proto,
                            "--dport",
                            &port.to_string(),
                            "-j",
                            "RETURN",
                        ],
                    );
                }
                let _ = run_ok(
                    bin,
                    &[
                        "-A",
                        IPT_CHAIN,
                        "-p",
                        proto,
                        "--dport",
                        &port.to_string(),
                        "-j",
                        "DROP",
                    ],
                );
            }
        }
    }
    true
}

#[cfg(target_os = "linux")]
fn clear_iptables_chain(bin: &str) {
    for _ in 0..8 {
        if run_ok(bin, &["-D", "INPUT", "-j", IPT_CHAIN]).is_err() {
            break;
        }
    }
    let _ = run_ok(bin, &["-F", IPT_CHAIN]);
    let _ = run_ok(bin, &["-X", IPT_CHAIN]);
}

#[cfg(target_os = "linux")]
fn command_exists(name: &str) -> bool {
    Command::new("sh")
        .args(["-c", &format!("command -v {name} >/dev/null 2>&1")])
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

#[cfg(target_os = "linux")]
fn run_ok(bin: &str, args: &[&str]) -> Result<(), String> {
    let output = Command::new(bin)
        .args(args)
        .output()
        .map_err(|err| err.to_string())?;
    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

#[cfg(target_os = "linux")]
fn run_stdin(bin: &str, args: &[&str], stdin: &str) -> Result<(), String> {
    use std::io::Write;
    let mut child = Command::new(bin)
        .args(args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|err| err.to_string())?;
    if let Some(mut pipe) = child.stdin.take() {
        pipe.write_all(stdin.as_bytes())
            .map_err(|err| err.to_string())?;
    }
    let output = child.wait_with_output().map_err(|err| err.to_string())?;
    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}
