# Tanzaku

简体中文 | [English](README.en.md)

Tanzaku 是一套多节点、多租户的内网穿透 / 端口转发系统，使用 Rust 编写。由中心控制面 Board、部署在公网服务器上的节点 Server、运行在内网机器上的客户端 Client 三部分组成，配套管理端与用户端两套 Web 界面。

## 功能

- **隧道类型**：TCP、UDP、HTTP（独立端口或共享 80/443 入口）、HTTPS（自动 HTTP → HTTPS 重定向），支持按域名路由
- **传输载体**：QUIC（BBR 拥塞控制）与 TCP 多路复用两种载体，端到端 ChaCha20-Poly1305 加密，断线自动重连、异常自动重建
- **多节点调度**：节点分组、端口范围、流量计量与限速、隧道配额
- **安全防护**：IP 黑白名单、单 IP / 单隧道并发与速率限制、HTTP / TLS 协议守卫、UDP 放大防护、攻击自动暂停与冷却、自动封禁，防护策略可全局 / 节点 / 隧道三级配置
- **证书**：上传自有证书，按域名自动下发到节点，到期提醒；未绑定域名时自动使用自签证书
- **多租户与计费**：用户体系、套餐、订单、余额与支付渠道（易支付等）、流量月重置、SMTP 邮件通知
- **界面**：管理端嵌入 Board 二进制；用户端为可替换的主题包，支持中英文
- **部署**：单文件静态二进制，覆盖 Linux（x86_64 / i386 / arm64 / mips / mipsel）、macOS（x86_64 / arm64）、Windows（x86_64 / i386 / arm64），一键安装脚本自动注册系统服务

## 架构

```
                         ┌──────────────────────────────┐
                         │          tz-board            │
                         │  :9000 管理端 + 节点 WebSocket │
                         │  :9001 用户端 + 客户端 WebSocket│
                         │  PostgreSQL + Redis          │
                         └───────▲──────────────▲───────┘
                       控制通道  │              │  控制通道
                                 │              │
   公网访客 ──► ┌────────────────┴──┐       ┌───┴────────────────┐
                │  tanzaku-server   │◄══════│   tanzaku-client   │ ──► 内网服务
                │  (公网节点)       │ 加密载体│   (内网机器)        │
                └───────────────────┘ QUIC/TCP└────────────────────┘
```

| 组件 | 二进制 | 作用 |
|------|--------|------|
| Board | `tz-board` | 控制面：管理 API、用户 API、调度、计费、Web 界面 |
| Server | `tanzaku-server` | 公网节点：监听入口端口，接收访客流量并经加密载体转发给 Client |
| Client | `tanzaku-client` | 内网客户端：与节点建立载体连接，把流量转发到本地服务 |

## 快速开始

### 1. 部署 Board

依赖 PostgreSQL 与 Redis（需自行准备）。一键安装并注册为系统服务：

```bash
curl -fsSL https://raw.githubusercontent.com/adokiu/Tanzaku/main/install.sh | sudo bash -s -- -r board
```

国内服务器可走 GitHub 加速站（脚本与二进制都经加速站下载）：

```bash
curl -fsSL https://ghfast.top/https://raw.githubusercontent.com/adokiu/Tanzaku/main/install.sh | sudo bash -s -- -r board --gh-proxy ghfast.top
```

默认安装到 `/opt/tanzaku-board`，配置 `board.toml`，数据目录 `data/`；可用 `--admin-listen` / `--user-listen` 改监听地址。再次执行脚本只升级二进制，不覆盖已有配置。

浏览器打开 `http://<Board IP>:9000`，按安装向导填写数据库、Redis 与管理员账号。`9000` 为管理端口（管理界面 + 节点接入），`9001` 为用户端口（用户界面 + 客户端接入）。生产环境建议放在 Nginx / CDN 之后并开启 HTTPS，Board 会读取 `CF-Connecting-IP` / `X-Real-IP` / `X-Forwarded-For` 获取真实访客 IP。

配置文件路径优先级：`-c/--config` > 环境变量 `TANZAKU_CONFIG` > 可执行文件同级 `board.toml`；监听地址可用 `TANZAKU_LISTEN_ADMIN` / `TANZAKU_LISTEN_USER` 覆盖。

### 2. 添加节点（Server）

管理端「节点」页新建节点，点击该行的「安装」按钮，弹窗会生成带 token 的一键命令，形如：

```bash
curl -fsSL https://raw.githubusercontent.com/adokiu/Tanzaku/main/install.sh | sudo bash -s -- -r server -e 'wss://board.example.com:9000' -t 'NODE_TOKEN'
```

### 3. 添加客户端（Client）

用户端「客户端」页新建客户端，同样点击「安装」生成命令：

```bash
curl -fsSL https://raw.githubusercontent.com/adokiu/Tanzaku/main/install.sh | sudo bash -s -- -e 'wss://board.example.com:9001' -t 'CLIENT_TOKEN'
```

Windows（管理员 PowerShell / CMD）：

```powershell
powershell -ExecutionPolicy Bypass -Command "& ([scriptblock]::Create((irm 'https://raw.githubusercontent.com/adokiu/Tanzaku/main/install.ps1'))) -Role client -Endpoint 'wss://board.example.com:9001' -Token 'CLIENT_TOKEN'"
```

### 4. 创建隧道

用户端「隧道」页选择客户端、节点与类型，填写本地地址与端口即可。

## 一键安装脚本

`install.sh`（bash，Linux / macOS）与 `install.ps1`（Windows）自动识别架构、下载对应版本并注册为系统服务（systemd / OpenRC / launchd / Windows 计划任务）。

| 参数（bash） | 参数（ps1） | 说明 |
|-------------|-------------|------|
| `-r, --role` | `-Role` | `client`（默认）、`server` 或 `board`（仅 Linux） |
| `-e, --endpoint` | `-Endpoint` | Board 地址；server 连管理端口，client 连用户端口；board 不需要 |
| `-t, --token` | `-Token` | 节点 / 客户端 token；board 不需要 |
| `-v, --version` | `-Version` | 指定版本 tag，默认最新 |
| `--gh-proxy` | `-GhProxy` | GitHub 加速站，如 `ghfast.top`（可省略 `https://`） |
| `--install-dir` | `-InstallDir` | 安装目录，默认 `/opt/tanzaku-<role>` / `%ProgramFiles%\Tanzaku\<role>` |
| `--service-name` | `-ServiceName` | 服务名，默认 `tanzaku-<role>` |
| `--log-level` | `-LogLevel` | `error` / `warn` / `info` / `debug` / `trace` |
| `--admin-listen` / `--user-listen` | — | 仅 board：管理端 / 用户端监听地址，默认 `0.0.0.0:9000` / `0.0.0.0:9001` |
| `--uninstall` | `-Uninstall` | 卸载（board 保留数据目录与配置） |

Token 可在管理端 / 用户端随时查看或重置，重置后旧 token 立即失效。

## 手动运行 Server / Client

```bash
./tanzaku-server --board wss://board.example.com:9000 --token NODE_TOKEN
TANZAKU_BOARD=wss://board.example.com:9001 TANZAKU_TOKEN=CLIENT_TOKEN ./tanzaku-client
```

参数优先级：命令行 > `TANZAKU_BOARD` / `TANZAKU_TOKEN` > 同级 `server.toml` / `client.toml`（示例见 `deploy/config/`）。

## 从源码构建

要求 Rust 1.90（见 `rust-toolchain.toml`）与 Node.js 20。

```bash
# 管理端前端（嵌入 tz-board）
cd web-admin && npm ci && npm run build && cd ..
# 后端
cargo build --release -p tz-board -p tz-server -p tz-client
# 用户端主题包（上传到管理端「用户主题」）
cd web-user && npm ci && npm run build
```

macOS 上交叉编译 Linux 静态二进制可使用 `deploy/build-linux-amd64.sh`，详见 [deploy/README.md](deploy/README.md)。

推送 `v*` tag 会触发 GitHub Actions 自动构建全部平台并发布 Release。

## 目录结构

```
crates/
  tz-board     控制面（API、调度、计费、管理端静态资源）
  tz-server    公网节点
  tz-client    内网客户端
  tz-agent     节点 / 客户端共用的接入与控制通道
  tz-carrier   加密传输载体（QUIC、TCP 多路复用）
  tz-ingress   入口监听（TCP / UDP / HTTP / 共享 HTTP(S)）
  tz-guard     安全防护流水线
  tz-net       限速、计量、准入
  tz-pki       证书与密钥
  tz-proto     控制协议
  tz-common    公共工具
web-admin/     管理端前端
web-user/      用户端主题
deploy/        构建脚本与配置示例
install.sh     Linux / macOS 一键安装（bash）
install.ps1    Windows 一键安装
```

## 许可证

本项目尚未声明开源许可证，使用前请联系作者。
