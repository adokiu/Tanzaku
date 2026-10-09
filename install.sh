#!/usr/bin/env bash
# Tanzaku 一键安装脚本（bash）。支持 board / server / client，自动下载并注册为系统服务（systemd / OpenRC / launchd）。
#
#   # Board（控制面）
#   curl -fsSL https://raw.githubusercontent.com/adokiu/Tanzaku/main/install.sh | sudo bash -s -- -r board
#   # 节点
#   curl -fsSL https://raw.githubusercontent.com/adokiu/Tanzaku/main/install.sh | sudo bash -s -- -r server -e https://board.example.com:9000 -t TOKEN
#   # 客户端
#   wget -qO- https://raw.githubusercontent.com/adokiu/Tanzaku/main/install.sh | sudo bash -s -- -e https://board.example.com:9001 -t TOKEN
#   # 走 GitHub 加速站
#   curl -fsSL https://ghfast.top/https://raw.githubusercontent.com/adokiu/Tanzaku/main/install.sh | sudo bash -s -- -r board --gh-proxy ghfast.top
set -euo pipefail

REPO="${TANZAKU_REPO:-adokiu/Tanzaku}"
ROLE="client"
ENDPOINT=""
TOKEN=""
VERSION=""
GH_PROXY=""
INSTALL_DIR=""
SERVICE_NAME=""
LOG_LEVEL="info"
ADMIN_LISTEN="0.0.0.0:9000"
USER_LISTEN="0.0.0.0:9001"
UNINSTALL=0

usage() {
  cat <<'USAGE'
用法: install.sh [选项]

  -r, --role ROLE          client（默认）、server 或 board
  -e, --endpoint URL       Board 地址（server 连管理端口，client 连用户端口；board 不需要）
  -t, --token TOKEN        节点 / 客户端 token（board 不需要）
  -v, --version TAG        指定安装版本，如 v0.2.0；默认最新
      --gh-proxy URL       GitHub 加速站，如 ghfast.top 或 https://ghfast.top（--github-proxy 同义）
      --install-dir DIR    安装目录，默认 /opt/tanzaku-<role>
      --service-name NAME  服务名称，默认 tanzaku-<role>
      --log-level LEVEL    日志级别 error|warn|info|debug|trace，默认 info
      --admin-listen ADDR  仅 board：管理端监听地址，默认 0.0.0.0:9000
      --user-listen ADDR   仅 board：用户端监听地址，默认 0.0.0.0:9001
      --uninstall          卸载（需同时给出 -r / --service-name / --install-dir 以定位）
  -h, --help               显示帮助

board 首次安装后访问 http://<IP>:9000 完成数据库 / Redis / 管理员初始化；
再次执行脚本只升级二进制，不会覆盖已有 board.toml。
USAGE
}

info() { printf '\033[32m>>>\033[0m %s\n' "$*"; }
warn() { printf '\033[33m[警告]\033[0m %s\n' "$*" >&2; }
die() { printf '\033[31m[错误]\033[0m %s\n' "$*" >&2; exit 1; }

need_value() {
  [[ $# -ge 2 && -n "$2" ]] || die "选项 $1 需要参数"
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    -r|--role) need_value "$@"; ROLE="$2"; shift 2 ;;
    -e|--endpoint) need_value "$@"; ENDPOINT="$2"; shift 2 ;;
    -t|--token) need_value "$@"; TOKEN="$2"; shift 2 ;;
    -v|--version) need_value "$@"; VERSION="$2"; shift 2 ;;
    --gh-proxy|--github-proxy) need_value "$@"; GH_PROXY="$2"; shift 2 ;;
    --install-dir) need_value "$@"; INSTALL_DIR="$2"; shift 2 ;;
    --service-name) need_value "$@"; SERVICE_NAME="$2"; shift 2 ;;
    --log-level) need_value "$@"; LOG_LEVEL="$2"; shift 2 ;;
    --admin-listen) need_value "$@"; ADMIN_LISTEN="$2"; shift 2 ;;
    --user-listen) need_value "$@"; USER_LISTEN="$2"; shift 2 ;;
    --uninstall) UNINSTALL=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) usage; die "未知选项: $1" ;;
  esac
done

case "$ROLE" in
  client|server|board) ;;
  *) die "--role 只能是 client、server 或 board" ;;
esac
case "$LOG_LEVEL" in
  error|warn|info|debug|trace) ;;
  *) die "--log-level 只能是 error|warn|info|debug|trace" ;;
esac

if [[ "$ROLE" == "board" ]]; then
  BIN_NAME="tz-board"
else
  BIN_NAME="tanzaku-$ROLE"
fi
INSTALL_DIR="${INSTALL_DIR:-/opt/tanzaku-$ROLE}"
SERVICE_NAME="${SERVICE_NAME:-tanzaku-$ROLE}"
[[ "$SERVICE_NAME" =~ ^[A-Za-z0-9._-]+$ ]] || die "服务名称只能包含字母、数字、. _ -"
BIN_PATH="$INSTALL_DIR/$BIN_NAME"
CONFIG_PATH="$INSTALL_DIR/$ROLE.toml"
DATA_DIR="$INSTALL_DIR/data"

[[ "$(id -u)" -eq 0 ]] || die "需要 root 权限运行（在管道后加 sudo，例如 | sudo bash -s -- ...）"

case "$(uname -s)" in
  Linux) OS_NAME="linux" ;;
  Darwin) OS_NAME="darwin" ;;
  *) die "不支持的系统: $(uname -s)（Windows 请使用 install.ps1）" ;;
esac
[[ "$ROLE" == "board" && "$OS_NAME" != "linux" ]] && die "board 只提供 Linux 版本"

# --gh-proxy 允许只写域名：ghfast.top -> https://ghfast.top
if [[ -n "$GH_PROXY" ]]; then
  [[ "$GH_PROXY" =~ ^https?:// ]] || GH_PROXY="https://$GH_PROXY"
  GH_PROXY="${GH_PROXY%/}"
fi

detect_arch() {
  local machine
  machine="$(uname -m)"
  case "$machine" in
    x86_64|amd64) echo "amd64" ;;
    i386|i486|i586|i686|x86) echo "386" ;;
    aarch64|arm64|armv8*) echo "arm64" ;;
    mips|mips64)
      # 小端机器上 'I' 的 od 输出第一个字节为 1。
      if [[ "$(printf 'I' | od -An -to2 | tr -d ' ' | cut -c6)" == "1" ]]; then echo "mipsle"; else echo "mips"; fi
      ;;
    mipsel|mipsle|mips64el) echo "mipsle" ;;
    *) die "不支持的 CPU 架构: $machine" ;;
  esac
}

service_manager() {
  if [[ "$OS_NAME" == "darwin" ]]; then
    echo "launchd"
  elif command -v systemctl >/dev/null 2>&1 && [[ -d /run/systemd/system ]]; then
    echo "systemd"
  elif command -v rc-service >/dev/null 2>&1 && [[ -d /etc/init.d ]]; then
    echo "openrc"
  else
    echo "none"
  fi
}

download() {
  local url="$1" out="$2"
  if command -v curl >/dev/null 2>&1; then
    curl -fL --retry 3 --connect-timeout 15 -o "$out" "$url"
  elif command -v wget >/dev/null 2>&1; then
    wget -q -O "$out" "$url"
  else
    die "需要 curl 或 wget"
  fi
}

toml_escape() {
  local v="$1"
  v="${v//\\/\\\\}"
  v="${v//\"/\\\"}"
  printf '%s' "$v"
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
    launchd)
      launchctl bootout "system/$SERVICE_NAME" 2>/dev/null \
        || launchctl unload "/Library/LaunchDaemons/$SERVICE_NAME.plist" 2>/dev/null || true
      ;;
    none)
      if [[ -f "$INSTALL_DIR/$SERVICE_NAME.pid" ]]; then
        kill "$(cat "$INSTALL_DIR/$SERVICE_NAME.pid")" 2>/dev/null || true
        rm -f "$INSTALL_DIR/$SERVICE_NAME.pid"
      fi
      ;;
  esac
}

install_systemd() {
  cat >"/etc/systemd/system/$SERVICE_NAME.service" <<UNIT
[Unit]
Description=Tanzaku $ROLE
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
WorkingDirectory=$INSTALL_DIR
Environment=RUST_LOG=$LOG_LEVEL
Environment=TANZAKU_DATA=$DATA_DIR
ExecStart=$BIN_PATH -c $CONFIG_PATH
Restart=always
RestartSec=5
LimitNOFILE=1048576

[Install]
WantedBy=multi-user.target
UNIT
  systemctl daemon-reload
  systemctl enable "$SERVICE_NAME" >/dev/null 2>&1
  systemctl restart "$SERVICE_NAME"
  info "已注册 systemd 服务 $SERVICE_NAME（日志: journalctl -u $SERVICE_NAME -f）"
}

install_openrc() {
  cat >"/etc/init.d/$SERVICE_NAME" <<RC
#!/sbin/openrc-run
name="$SERVICE_NAME"
description="Tanzaku $ROLE"
command="$BIN_PATH"
command_args="-c $CONFIG_PATH"
directory="$INSTALL_DIR"
pidfile="/run/$SERVICE_NAME.pid"
output_log="/var/log/$SERVICE_NAME.log"
error_log="/var/log/$SERVICE_NAME.log"
export RUST_LOG="$LOG_LEVEL"
export TANZAKU_DATA="$DATA_DIR"
supervisor=supervise-daemon
respawn_delay=5
respawn_max=0
rc_ulimit="-n 1048576"

depend() {
  need net
}
RC
  chmod +x "/etc/init.d/$SERVICE_NAME"
  rc-update add "$SERVICE_NAME" default >/dev/null 2>&1
  rc-service "$SERVICE_NAME" restart
  info "已注册 OpenRC 服务 $SERVICE_NAME（日志: /var/log/$SERVICE_NAME.log）"
}

install_launchd() {
  local plist="/Library/LaunchDaemons/$SERVICE_NAME.plist"
  cat >"$plist" <<PLIST
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
  <dict>
    <key>RUST_LOG</key><string>$LOG_LEVEL</string>
    <key>TANZAKU_DATA</key><string>$DATA_DIR</string>
  </dict>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>StandardOutPath</key><string>$INSTALL_DIR/$SERVICE_NAME.log</string>
  <key>StandardErrorPath</key><string>$INSTALL_DIR/$SERVICE_NAME.log</string>
</dict>
</plist>
PLIST
  chmod 644 "$plist"
  launchctl bootstrap system "$plist" 2>/dev/null || launchctl load -w "$plist"
  info "已注册 launchd 服务 $SERVICE_NAME（日志: $INSTALL_DIR/$SERVICE_NAME.log）"
}

install_nohup() {
  warn "未检测到 systemd / OpenRC，使用 nohup 后台运行，重启后需手动启动"
  (
    cd "$INSTALL_DIR"
    RUST_LOG="$LOG_LEVEL" TANZAKU_DATA="$DATA_DIR" nohup "$BIN_PATH" -c "$CONFIG_PATH" >>"$INSTALL_DIR/$SERVICE_NAME.log" 2>&1 &
    echo $! >"$INSTALL_DIR/$SERVICE_NAME.pid"
  )
  info "已后台启动（日志: $INSTALL_DIR/$SERVICE_NAME.log）"
}

MANAGER="$(service_manager)"

if [[ "$UNINSTALL" -eq 1 ]]; then
  info "卸载 $SERVICE_NAME"
  stop_service
  rm -f "/etc/systemd/system/$SERVICE_NAME.service" "/etc/init.d/$SERVICE_NAME" "/Library/LaunchDaemons/$SERVICE_NAME.plist"
  [[ "$MANAGER" == "systemd" ]] && systemctl daemon-reload
  if [[ "$ROLE" == "board" && -d "$DATA_DIR" ]]; then
    warn "保留 board 数据目录 $DATA_DIR 与配置 $CONFIG_PATH，如需彻底删除请手动执行 rm -rf $INSTALL_DIR"
    rm -f "$BIN_PATH"
  else
    rm -rf "$INSTALL_DIR"
  fi
  info "已卸载"
  exit 0
fi

if [[ "$ROLE" != "board" ]]; then
  [[ -n "$ENDPOINT" ]] || { usage; die "缺少 -e / --endpoint"; }
  [[ -n "$TOKEN" ]] || { usage; die "缺少 -t / --token"; }
fi

ARCH="$(detect_arch)"
if [[ "$ROLE" == "board" && "$ARCH" != "amd64" && "$ARCH" != "arm64" ]]; then
  die "board 只提供 linux amd64 / arm64 版本，当前为 $ARCH"
fi

ASSET="$BIN_NAME-$OS_NAME-$ARCH"
if [[ -n "$VERSION" ]]; then
  URL="https://github.com/$REPO/releases/download/$VERSION/$ASSET"
else
  URL="https://github.com/$REPO/releases/latest/download/$ASSET"
fi
[[ -n "$GH_PROXY" ]] && URL="$GH_PROXY/$URL"

info "系统: $OS_NAME/$ARCH  角色: $ROLE  服务管理: $MANAGER"
info "下载 $URL"
mkdir -p "$INSTALL_DIR"
TMP_BIN="$INSTALL_DIR/.$BIN_NAME.download"
download "$URL" "$TMP_BIN" || die "下载失败，可尝试 --gh-proxy 或 --version"
chmod +x "$TMP_BIN"

stop_service
mv -f "$TMP_BIN" "$BIN_PATH"
[[ "$OS_NAME" == "darwin" ]] && xattr -d com.apple.quarantine "$BIN_PATH" 2>/dev/null || true

umask 077
if [[ "$ROLE" == "board" ]]; then
  mkdir -p "$DATA_DIR"
  if [[ -f "$CONFIG_PATH" ]]; then
    info "保留已有配置 $CONFIG_PATH"
  else
    cat >"$CONFIG_PATH" <<CFG
# 由 install.sh 生成。PostgreSQL / Redis 由管理端安装向导写入。
[listen]
admin = "$(toml_escape "$ADMIN_LISTEN")"
user = "$(toml_escape "$USER_LISTEN")"
CFG
  fi
else
  cat >"$CONFIG_PATH" <<CFG
# 由 install.sh 生成
board = "$(toml_escape "$ENDPOINT")"
token = "$(toml_escape "$TOKEN")"
CFG
fi
umask 022

case "$MANAGER" in
  systemd) install_systemd ;;
  openrc) install_openrc ;;
  launchd) install_launchd ;;
  none) install_nohup ;;
esac

info "安装完成: $BIN_PATH"
info "配置文件: $CONFIG_PATH"
if [[ "$ROLE" == "board" ]]; then
  info "管理端: http://<本机IP>:${ADMIN_LISTEN##*:}   用户端: http://<本机IP>:${USER_LISTEN##*:}"
  info "首次访问管理端完成 PostgreSQL / Redis / 管理员初始化"
fi
info "卸载: 重新执行本脚本并加 --uninstall -r $ROLE --service-name $SERVICE_NAME --install-dir $INSTALL_DIR"
