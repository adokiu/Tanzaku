#!/bin/sh
# Tanzaku 一键安装（Linux / macOS / OpenWrt）。POSIX sh，不依赖 bash。
#
#   wget -qO- https://raw.githubusercontent.com/adokiu/Tanzaku/main/install.sh | sudo sh -s -- -e https://board.example.com:9001 -t TOKEN
#   curl -fsSL https://raw.githubusercontent.com/adokiu/Tanzaku/main/install.sh | sudo sh -s -- -r server -e https://board.example.com:9000 -t TOKEN
set -eu

REPO="${TANZAKU_REPO:-adokiu/Tanzaku}"
ROLE="client"
ENDPOINT=""
TOKEN=""
VERSION=""
GITHUB_PROXY=""
INSTALL_DIR=""
SERVICE_NAME=""
LOG_LEVEL="info"
UNINSTALL=0

usage() {
  cat <<'EOF'
用法: install.sh [选项]

  -e, --endpoint URL       Board 地址（server 连管理端口，client 连用户端口）
  -t, --token TOKEN        节点 / 客户端 token
  -r, --role ROLE          client（默认）或 server
  -v, --version TAG        指定安装版本，如 v0.2.0；默认最新
      --github-proxy URL   GitHub 下载代理前缀，如 https://ghfast.top
      --install-dir DIR    安装目录，默认 /opt/tanzaku-<role>
      --service-name NAME  服务名称，默认 tanzaku-<role>
      --log-level LEVEL    日志级别 error|warn|info|debug|trace，默认 info
      --uninstall          卸载（需同时给出 -r / --service-name / --install-dir 以定位）
  -h, --help               显示帮助
EOF
}

info() { printf '\033[32m>>>\033[0m %s\n' "$*"; }
warn() { printf '\033[33m[警告]\033[0m %s\n' "$*" >&2; }
die() { printf '\033[31m[错误]\033[0m %s\n' "$*" >&2; exit 1; }

need_value() {
  [ $# -ge 2 ] && [ -n "$2" ] || die "选项 $1 需要参数"
}

while [ $# -gt 0 ]; do
  case "$1" in
    -e|--endpoint) need_value "$@"; ENDPOINT="$2"; shift 2 ;;
    -t|--token) need_value "$@"; TOKEN="$2"; shift 2 ;;
    -r|--role) need_value "$@"; ROLE="$2"; shift 2 ;;
    -v|--version) need_value "$@"; VERSION="$2"; shift 2 ;;
    --github-proxy) need_value "$@"; GITHUB_PROXY="$2"; shift 2 ;;
    --install-dir) need_value "$@"; INSTALL_DIR="$2"; shift 2 ;;
    --service-name) need_value "$@"; SERVICE_NAME="$2"; shift 2 ;;
    --log-level) need_value "$@"; LOG_LEVEL="$2"; shift 2 ;;
    --uninstall) UNINSTALL=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) usage; die "未知选项: $1" ;;
  esac
done

case "$ROLE" in
  client|server) ;;
  *) die "--role 只能是 client 或 server" ;;
esac
case "$LOG_LEVEL" in
  error|warn|info|debug|trace) ;;
  *) die "--log-level 只能是 error|warn|info|debug|trace" ;;
esac

BIN_NAME="tanzaku-$ROLE"
[ -n "$INSTALL_DIR" ] || INSTALL_DIR="/opt/tanzaku-$ROLE"
[ -n "$SERVICE_NAME" ] || SERVICE_NAME="tanzaku-$ROLE"
case "$SERVICE_NAME" in
  *[!A-Za-z0-9._-]*) die "服务名称只能包含字母、数字、. _ -" ;;
esac
BIN_PATH="$INSTALL_DIR/$BIN_NAME"
CONFIG_PATH="$INSTALL_DIR/$ROLE.toml"

if [ "$(id -u)" -ne 0 ]; then
  die "需要 root 权限运行（在管道后加 sudo，例如 | sudo sh -s -- ...）"
fi

OS="$(uname -s)"
case "$OS" in
  Linux) OS_NAME="linux" ;;
  Darwin) OS_NAME="darwin" ;;
  *) die "不支持的系统: $OS（Windows 请使用 install.ps1）" ;;
esac

detect_arch() {
  machine="$(uname -m)"
  case "$machine" in
    x86_64|amd64) echo "amd64" ;;
    i386|i486|i586|i686|x86) echo "386" ;;
    aarch64|arm64|armv8*) echo "arm64" ;;
    mips|mips64)
      # od 输出第一个字节为 1 表示小端。
      if [ "$(printf 'I' | od -An -to2 | tr -d ' ' | cut -c6)" = "1" ]; then echo "mipsle"; else echo "mips"; fi
      ;;
    mipsel|mipsle|mips64el) echo "mipsle" ;;
    *) die "不支持的 CPU 架构: $machine" ;;
  esac
}

service_manager() {
  if [ "$OS_NAME" = "darwin" ]; then
    echo "launchd"
  elif command -v systemctl >/dev/null 2>&1 && [ -d /run/systemd/system ]; then
    echo "systemd"
  elif command -v rc-service >/dev/null 2>&1 && [ -d /etc/init.d ]; then
    echo "openrc"
  elif [ -f /etc/rc.common ] && { command -v procd >/dev/null 2>&1 || [ -x /sbin/procd ]; }; then
    echo "procd"
  else
    echo "none"
  fi
}

download() {
  url="$1"
  out="$2"
  if command -v curl >/dev/null 2>&1; then
    curl -fL --retry 3 --connect-timeout 15 -o "$out" "$url"
  elif command -v wget >/dev/null 2>&1; then
    wget -q -O "$out" "$url"
  else
    die "需要 curl 或 wget"
  fi
}

toml_escape() {
  printf '%s' "$1" | sed -e 's/\\/\\\\/g' -e 's/"/\\"/g'
}

stop_service() {
  case "$MANAGER" in
    systemd)
      systemctl stop "$SERVICE_NAME" 2>/dev/null || true
      systemctl disable "$SERVICE_NAME" 2>/dev/null || true
      ;;
    openrc)
      rc-service "$SERVICE_NAME" stop 2>/dev/null || true
      rc-update del "$SERVICE_NAME" default 2>/dev/null || true
      ;;
    procd)
      [ -x "/etc/init.d/$SERVICE_NAME" ] && "/etc/init.d/$SERVICE_NAME" stop 2>/dev/null || true
      [ -x "/etc/init.d/$SERVICE_NAME" ] && "/etc/init.d/$SERVICE_NAME" disable 2>/dev/null || true
      ;;
    launchd)
      launchctl bootout "system/$SERVICE_NAME" 2>/dev/null || launchctl unload "/Library/LaunchDaemons/$SERVICE_NAME.plist" 2>/dev/null || true
      ;;
    none)
      if [ -f "$INSTALL_DIR/$SERVICE_NAME.pid" ]; then
        kill "$(cat "$INSTALL_DIR/$SERVICE_NAME.pid")" 2>/dev/null || true
        rm -f "$INSTALL_DIR/$SERVICE_NAME.pid"
      fi
      ;;
  esac
}

install_systemd() {
  cat >"/etc/systemd/system/$SERVICE_NAME.service" <<EOF
[Unit]
Description=Tanzaku $ROLE
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
WorkingDirectory=$INSTALL_DIR
Environment=RUST_LOG=$LOG_LEVEL
ExecStart=$BIN_PATH -c $CONFIG_PATH
Restart=always
RestartSec=5
LimitNOFILE=1048576

[Install]
WantedBy=multi-user.target
EOF
  systemctl daemon-reload
  systemctl enable "$SERVICE_NAME" >/dev/null 2>&1
  systemctl restart "$SERVICE_NAME"
  info "已注册 systemd 服务 $SERVICE_NAME（日志: journalctl -u $SERVICE_NAME -f）"
}

install_openrc() {
  cat >"/etc/init.d/$SERVICE_NAME" <<EOF
#!/sbin/openrc-run
name="$SERVICE_NAME"
description="Tanzaku $ROLE"
command="$BIN_PATH"
command_args="-c $CONFIG_PATH"
command_background=true
directory="$INSTALL_DIR"
pidfile="/run/$SERVICE_NAME.pid"
output_log="/var/log/$SERVICE_NAME.log"
error_log="/var/log/$SERVICE_NAME.log"
export RUST_LOG="$LOG_LEVEL"
supervisor=supervise-daemon
respawn_delay=5

depend() {
  need net
}
EOF
  chmod +x "/etc/init.d/$SERVICE_NAME"
  rc-update add "$SERVICE_NAME" default >/dev/null 2>&1
  rc-service "$SERVICE_NAME" restart
  info "已注册 OpenRC 服务 $SERVICE_NAME（日志: /var/log/$SERVICE_NAME.log）"
}

install_procd() {
  cat >"/etc/init.d/$SERVICE_NAME" <<EOF
#!/bin/sh /etc/rc.common
START=99
USE_PROCD=1

start_service() {
  procd_open_instance
  procd_set_param command $BIN_PATH -c $CONFIG_PATH
  procd_set_param env RUST_LOG=$LOG_LEVEL
  procd_set_param respawn 3600 5 0
  procd_set_param stdout 1
  procd_set_param stderr 1
  procd_close_instance
}
EOF
  chmod +x "/etc/init.d/$SERVICE_NAME"
  "/etc/init.d/$SERVICE_NAME" enable
  "/etc/init.d/$SERVICE_NAME" restart
  info "已注册 procd 服务 $SERVICE_NAME（日志: logread -f）"
}

install_launchd() {
  plist="/Library/LaunchDaemons/$SERVICE_NAME.plist"
  cat >"$plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>$SERVICE_NAME</string>
  <key>ProgramArguments</key>
  <array>
    <string>$BIN_PATH</string>
    <string>-c</string>
    <string>$CONFIG_PATH</string>
  </array>
  <key>WorkingDirectory</key><string>$INSTALL_DIR</string>
  <key>EnvironmentVariables</key>
  <dict><key>RUST_LOG</key><string>$LOG_LEVEL</string></dict>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>StandardOutPath</key><string>$INSTALL_DIR/$SERVICE_NAME.log</string>
  <key>StandardErrorPath</key><string>$INSTALL_DIR/$SERVICE_NAME.log</string>
</dict>
</plist>
EOF
  chmod 644 "$plist"
  launchctl bootstrap system "$plist" 2>/dev/null || launchctl load -w "$plist"
  info "已注册 launchd 服务 $SERVICE_NAME（日志: $INSTALL_DIR/$SERVICE_NAME.log）"
}

install_nohup() {
  warn "未检测到 systemd / OpenRC / procd，使用 nohup 后台运行，重启后需手动启动"
  (cd "$INSTALL_DIR" && RUST_LOG="$LOG_LEVEL" nohup "$BIN_PATH" -c "$CONFIG_PATH" >>"$INSTALL_DIR/$SERVICE_NAME.log" 2>&1 &
    echo $! >"$INSTALL_DIR/$SERVICE_NAME.pid")
  info "已后台启动（日志: $INSTALL_DIR/$SERVICE_NAME.log）"
}

MANAGER="$(service_manager)"

if [ "$UNINSTALL" -eq 1 ]; then
  info "卸载 $SERVICE_NAME"
  stop_service
  rm -f "/etc/systemd/system/$SERVICE_NAME.service" "/etc/init.d/$SERVICE_NAME" "/Library/LaunchDaemons/$SERVICE_NAME.plist"
  [ "$MANAGER" = "systemd" ] && systemctl daemon-reload || true
  rm -rf "$INSTALL_DIR"
  info "已卸载"
  exit 0
fi

[ -n "$ENDPOINT" ] || { usage; die "缺少 -e / --endpoint"; }
[ -n "$TOKEN" ] || { usage; die "缺少 -t / --token"; }

ARCH="$(detect_arch)"
ASSET="$BIN_NAME-$OS_NAME-$ARCH"
if [ -n "$VERSION" ]; then
  URL="https://github.com/$REPO/releases/download/$VERSION/$ASSET"
else
  URL="https://github.com/$REPO/releases/latest/download/$ASSET"
fi
if [ -n "$GITHUB_PROXY" ]; then
  URL="${GITHUB_PROXY%/}/$URL"
fi

info "系统: $OS_NAME/$ARCH  角色: $ROLE  服务管理: $MANAGER"
info "下载 $URL"
mkdir -p "$INSTALL_DIR"
TMP_BIN="$INSTALL_DIR/.$BIN_NAME.download"
download "$URL" "$TMP_BIN" || die "下载失败，可尝试 --github-proxy 或 --version"
chmod +x "$TMP_BIN"

stop_service
mv -f "$TMP_BIN" "$BIN_PATH"
if [ "$OS_NAME" = "darwin" ]; then
  xattr -d com.apple.quarantine "$BIN_PATH" 2>/dev/null || true
fi

umask 077
cat >"$CONFIG_PATH" <<EOF
# 由 install.sh 生成
board = "$(toml_escape "$ENDPOINT")"
token = "$(toml_escape "$TOKEN")"
EOF
umask 022

case "$MANAGER" in
  systemd) install_systemd ;;
  openrc) install_openrc ;;
  procd) install_procd ;;
  launchd) install_launchd ;;
  none) install_nohup ;;
esac

info "安装完成: $BIN_PATH"
info "配置文件: $CONFIG_PATH"
info "卸载: 重新执行本脚本并加 --uninstall -r $ROLE --service-name $SERVICE_NAME --install-dir $INSTALL_DIR"
