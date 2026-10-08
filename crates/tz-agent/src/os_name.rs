//! 与 komari-agent `monitoring/unit/os_*.go` 的 `OSName()` 一致。

pub fn os_name() -> String {
    #[cfg(target_os = "linux")]
    {
        return linux::os_name();
    }
    #[cfg(target_os = "macos")]
    {
        return darwin::os_name();
    }
    #[cfg(target_os = "freebsd")]
    {
        return freebsd::os_name();
    }
    #[cfg(target_os = "windows")]
    {
        return windows::os_name();
    }
    #[cfg(not(any(
        target_os = "linux",
        target_os = "macos",
        target_os = "freebsd",
        target_os = "windows"
    )))]
    {
        std::env::consts::OS.to_owned()
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use std::io::{BufRead, BufReader};
    use std::path::Path;
    use std::process::Command;

    pub fn os_name() -> String {
        if let Some(name) = detect_android() {
            return name;
        }
        if let Some(name) = detect_proxmox_ve() {
            return name;
        }
        if let Some(name) = detect_synology() {
            return name;
        }
        if let Some(name) = detect_fnos() {
            return name;
        }
        pretty_name_from_os_release().unwrap_or_else(|| "Linux".into())
    }

    fn pretty_name_from_os_release() -> Option<String> {
        let file = std::fs::File::open("/etc/os-release").ok()?;
        let reader = BufReader::new(file);
        for line in reader.lines().map_while(Result::ok) {
            if let Some(value) = line.strip_prefix("PRETTY_NAME=") {
                return Some(unquote_os_release_value(value));
            }
        }
        None
    }

    fn os_release_field(key: &str) -> Option<String> {
        let file = std::fs::File::open("/etc/os-release").ok()?;
        let prefix = format!("{key}=");
        for line in BufReader::new(file).lines().map_while(Result::ok) {
            if let Some(value) = line.strip_prefix(&prefix) {
                return Some(unquote_os_release_value(value));
            }
        }
        None
    }

    fn unquote_os_release_value(raw: &str) -> String {
        let trimmed = raw.trim();
        if trimmed.len() >= 2 {
            let bytes = trimmed.as_bytes();
            if (bytes[0] == b'"' && bytes[bytes.len() - 1] == b'"')
                || (bytes[0] == b'\'' && bytes[bytes.len() - 1] == b'\'')
            {
                return trimmed[1..trimmed.len() - 1].to_owned();
            }
        }
        trimmed.to_owned()
    }

    fn detect_fnos() -> Option<String> {
        let build_version = Path::new("/usr/trim/BUILD_VERSION");
        if build_version.is_file() {
            if let Ok(data) = std::fs::read_to_string(build_version) {
                let version = data.trim();
                if !version.is_empty() {
                    return Some(format!("fnOS {version}"));
                }
            }
        }
        if Path::new("/usr/trim").is_dir() {
            return Some("fnOS".into());
        }
        None
    }

    fn detect_synology() -> Option<String> {
        for file in ["/etc/synoinfo.conf", "/etc.defaults/synoinfo.conf"] {
            if Path::new(file).is_file() {
                if let Some(info) = read_synology_info(file) {
                    return Some(info);
                }
            }
        }
        if Path::new("/usr/syno").is_dir() {
            return Some("Synology DSM".into());
        }
        None
    }

    fn read_synology_info(filename: &str) -> Option<String> {
        let file = std::fs::File::open(filename).ok()?;
        let mut unique = String::new();
        let mut udc_check_state = String::new();
        for line in BufReader::new(file).lines().map_while(Result::ok) {
            let line = line.trim();
            if let Some(value) = line.strip_prefix("unique=") {
                unique = unquote_os_release_value(value);
            } else if let Some(value) = line.strip_prefix("udc_check_state=") {
                udc_check_state = unquote_os_release_value(value);
            }
        }
        if unique.contains("synology_") {
            let parts: Vec<_> = unique.split('_').collect();
            if parts.len() >= 3 {
                let model = parts[parts.len() - 1].to_ascii_uppercase();
                let mut result = format!("Synology {model}");
                if !udc_check_state.is_empty() {
                    result.push_str(&format!(" DSM {udc_check_state}"));
                } else {
                    result.push_str(" DSM");
                }
                return Some(result);
            }
        }
        None
    }

    fn detect_proxmox_ve() -> Option<String> {
        let output = Command::new("pveversion").output().ok()?;
        if !output.status.success() && output.stdout.is_empty() {
            return None;
        }
        let text = String::from_utf8_lossy(&output.stdout);
        let mut version = None::<String>;
        for line in text.lines() {
            let line = line.trim();
            if let Some(rest) = line.strip_prefix("pve-manager/") {
                let version_part = rest.split('~').next().unwrap_or(rest);
                version = Some(version_part.to_owned());
            }
        }
        let version = version?;
        let codename = os_release_field("VERSION_CODENAME").filter(|v| !v.is_empty());
        Some(match codename {
            Some(code) => format!("Proxmox VE {version} ({code})"),
            None => format!("Proxmox VE {version}"),
        })
    }

    fn detect_android() -> Option<String> {
        if let Ok(output) = Command::new("getprop").arg("ro.build.version.release").output() {
            let version_str = String::from_utf8_lossy(&output.stdout).trim().to_owned();
            if !version_str.is_empty() {
                let model = Command::new("getprop")
                    .arg("ro.product.model")
                    .output()
                    .ok()
                    .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
                    .unwrap_or_default();
                let brand = Command::new("getprop")
                    .arg("ro.product.brand")
                    .output()
                    .ok()
                    .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
                    .unwrap_or_default();
                let mut result = format!("Android {version_str}");
                if !model.is_empty() {
                    if !brand.is_empty() && brand != model {
                        result.push_str(&format!(" ({brand} {model})"));
                    } else {
                        result.push_str(&format!(" ({model})"));
                    }
                }
                return Some(result);
            }
        }
        if Path::new("/system/build.prop").exists() {
            return Some(read_android_build_prop().unwrap_or_else(|| "Android".into()));
        }
        if is_android_system() {
            return Some("Android".into());
        }
        None
    }

    fn read_android_build_prop() -> Option<String> {
        let file = std::fs::File::open("/system/build.prop").ok()?;
        let mut version = String::new();
        let mut model = String::new();
        let mut brand = String::new();
        for line in BufReader::new(file).lines().map_while(Result::ok) {
            let line = line.trim();
            if let Some(v) = line.strip_prefix("ro.build.version.release=") {
                version = v.to_owned();
            } else if let Some(v) = line.strip_prefix("ro.product.model=") {
                model = v.to_owned();
            } else if let Some(v) = line.strip_prefix("ro.product.brand=") {
                brand = v.to_owned();
            }
            if !version.is_empty() && !model.is_empty() && !brand.is_empty() {
                break;
            }
        }
        if version.is_empty() {
            return None;
        }
        let mut result = format!("Android {version}");
        if !model.is_empty() {
            if !brand.is_empty() && brand != model {
                result.push_str(&format!(" ({brand} {model})"));
            } else {
                result.push_str(&format!(" ({model})"));
            }
        }
        Some(result)
    }

    fn is_android_system() -> bool {
        let dirs = ["/system/app", "/system/priv-app", "/data/app", "/sdcard"];
        dirs.iter().filter(|path| Path::new(path).is_dir()).count() >= 2
    }
}

#[cfg(target_os = "macos")]
mod darwin {
    use std::process::Command;

    pub fn os_name() -> String {
        Command::new("sw_vers")
            .arg("-productName")
            .output()
            .ok()
            .and_then(|output| {
                let name = String::from_utf8_lossy(&output.stdout).trim().to_owned();
                (!name.is_empty()).then_some(name)
            })
            .unwrap_or_else(|| "macOS".into())
    }
}

#[cfg(target_os = "freebsd")]
mod freebsd {
    use std::process::Command;

    pub fn os_name() -> String {
        Command::new("uname")
            .args(["-sr"])
            .output()
            .ok()
            .and_then(|output| {
                let name = String::from_utf8_lossy(&output.stdout).trim().to_owned();
                (!name.is_empty()).then_some(name)
            })
            .unwrap_or_else(|| "FreeBSD".into())
    }
}

#[cfg(target_os = "windows")]
mod windows {
    use std::process::Command;

    pub fn os_name() -> String {
        let script = r#"
$k = 'HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion'
$p = (Get-ItemProperty -Path $k -Name ProductName).ProductName
$b = (Get-ItemProperty -Path $k -Name CurrentBuild).CurrentBuild
if ($p -match 'Server') { $p; exit }
if ($p -match 'Windows 11') { $p; exit }
if ([int]$b -ge 22000) {
  if ($p -like 'Windows 10 *') { 'Windows 11 ' + $p.Substring(10); exit }
  if ($p -eq 'Windows 10') { 'Windows 11'; exit }
  if ($p -notmatch 'Windows 11') { ($p -replace 'Windows 10','Windows 11'); exit }
}
$p
"#;
        Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", script])
            .output()
            .ok()
            .and_then(|output| {
                let name = String::from_utf8_lossy(&output.stdout).trim().to_owned();
                (!name.is_empty()).then_some(name)
            })
            .unwrap_or_else(|| "Microsoft Windows".into())
    }
}
