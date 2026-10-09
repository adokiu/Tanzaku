# Tanzaku

[简体中文](README.md) | English

Tanzaku is a multi-node, multi-tenant reverse tunneling / port forwarding system written in Rust. It consists of a central control plane (Board), public-facing nodes (Server), clients running inside private networks (Client), and two web UIs for administrators and end users.

## Features

- **Tunnel types**: TCP, UDP, HTTP (dedicated port or shared 80/443 ingress) and HTTPS (automatic HTTP → HTTPS redirect), with domain-based routing
- **Carriers**: QUIC (BBR congestion control) and multiplexed TCP, end-to-end ChaCha20-Poly1305 encryption, automatic reconnect and tunnel self-healing
- **Multi-node scheduling**: node groups, port ranges, traffic metering and rate limiting, tunnel quotas
- **Protection**: IP allow/deny lists, per-IP and per-tunnel concurrency and rate limits, HTTP / TLS protocol guards, UDP amplification protection, automatic suspend-and-cooldown under attack, auto-ban; policies configurable at global, node and tunnel level
- **Certificates**: upload your own certificates, automatic distribution to nodes by domain, expiry reminders; self-signed fallback when no domain is bound
- **Multi-tenancy and billing**: users, plans, orders, balance and payment channels, monthly traffic reset, SMTP notifications
- **UI**: the admin panel is embedded in the Board binary; the user panel is a replaceable theme package; both ship in Chinese and English
- **Deployment**: single static binaries for Linux (x86_64 / i386 / arm64 / mips / mipsel), macOS (x86_64 / arm64) and Windows (x86_64 / i386 / arm64); one-line installer registers a system service

## Architecture

```
                         ┌──────────────────────────────┐
                         │          tz-board            │
                         │  :9000 admin UI + node WS    │
                         │  :9001 user UI + client WS   │
                         │  PostgreSQL + Redis          │
                         └───────▲──────────────▲───────┘
                     control     │              │     control
                                 │              │
   visitors ──────► ┌────────────┴──────┐   ┌───┴────────────────┐
                    │  tanzaku-server   │◄══│   tanzaku-client   │ ──► local service
                    │  (public node)    │   │   (private host)   │
                    └───────────────────┘   └────────────────────┘
                              encrypted carrier (QUIC / TCP)
```

| Component | Binary | Role |
|-----------|--------|------|
| Board | `tz-board` | Control plane: admin API, user API, scheduling, billing, web UIs |
| Server | `tanzaku-server` | Public node: listens on ingress ports and forwards visitor traffic to clients over the encrypted carrier |
| Client | `tanzaku-client` | Private client: connects to nodes and forwards traffic to local services |

## Quick start

### 1. Deploy the Board

Requires PostgreSQL and Redis (provision them yourself). One-line install registered as a system service:

```bash
curl -fsSL https://raw.githubusercontent.com/adokiu/Tanzaku/main/install.sh | sudo bash -s -- -r board
```

Through a GitHub mirror (both the script and the binary are fetched via the mirror):

```bash
curl -fsSL https://ghfast.top/https://raw.githubusercontent.com/adokiu/Tanzaku/main/install.sh | sudo bash -s -- -r board --gh-proxy ghfast.top
```

Installs to `/opt/tanzaku-board` with `board.toml` and a `data/` directory; use `--admin-listen` / `--user-listen` to change the listen addresses. Re-running the script only upgrades the binary and keeps the existing config.

Open `http://<board-ip>:9000` and complete the setup wizard (database, Redis, admin account). Port `9000` serves the admin UI and node connections; port `9001` serves the user UI and client connections. In production put the Board behind Nginx / a CDN with HTTPS; it reads `CF-Connecting-IP` / `X-Real-IP` / `X-Forwarded-For` for the real visitor IP.

Config path precedence: `-c/--config` > `TANZAKU_CONFIG` env > `board.toml` next to the executable. Listen addresses can be overridden with `TANZAKU_LISTEN_ADMIN` / `TANZAKU_LISTEN_USER`.

### 2. Add a node (Server)

Create a node on the admin "Nodes" page, then click "Install" on its row. The dialog generates a one-line command with the token:

```bash
curl -fsSL https://raw.githubusercontent.com/adokiu/Tanzaku/main/install.sh | sudo bash -s -- -r server -e 'https://board.example.com:9000' -t 'NODE_TOKEN'
```

### 3. Add a client

Create a client on the user "Clients" page and click "Install":

```bash
curl -fsSL https://raw.githubusercontent.com/adokiu/Tanzaku/main/install.sh | sudo bash -s -- -e 'https://board.example.com:9001' -t 'CLIENT_TOKEN'
```

Windows (elevated PowerShell / CMD):

```powershell
powershell -ExecutionPolicy Bypass -Command "& ([scriptblock]::Create((irm 'https://raw.githubusercontent.com/adokiu/Tanzaku/main/install.ps1'))) -Role client -Endpoint 'https://board.example.com:9001' -Token 'CLIENT_TOKEN'"
```

### 4. Create a tunnel

On the user "Tunnels" page pick a client, a node and a tunnel type, then enter the local address and port.

## One-line installer

`install.sh` (bash; Linux / macOS) and `install.ps1` (Windows) detect the architecture, download the matching release and register a system service (systemd / OpenRC / launchd / Windows scheduled task).

| Option (bash) | Option (ps1) | Description |
|---------------|--------------|-------------|
| `-r, --role` | `-Role` | `client` (default), `server` or `board` (Linux only) |
| `-e, --endpoint` | `-Endpoint` | Board URL; servers use the admin port, clients the user port; not needed for board |
| `-t, --token` | `-Token` | Node / client token; not needed for board |
| `-v, --version` | `-Version` | Release tag to install, latest by default |
| `--gh-proxy` | `-GhProxy` | GitHub mirror, e.g. `ghfast.top` (`https://` optional) |
| `--install-dir` | `-InstallDir` | Install directory, default `/opt/tanzaku-<role>` / `%ProgramFiles%\Tanzaku\<role>` |
| `--service-name` | `-ServiceName` | Service name, default `tanzaku-<role>` |
| `--log-level` | `-LogLevel` | `error` / `warn` / `info` / `debug` / `trace` |
| `--admin-listen` / `--user-listen` | — | board only: admin / user listen address, default `0.0.0.0:9000` / `0.0.0.0:9001` |
| `--uninstall` | `-Uninstall` | Uninstall (board keeps its data directory and config) |

Tokens can be viewed or reset at any time from the admin / user panel; a reset revokes the old token immediately.

## Running Server / Client manually

```bash
./tanzaku-server --board https://board.example.com:9000 --token NODE_TOKEN
TANZAKU_BOARD=https://board.example.com:9001 TANZAKU_TOKEN=CLIENT_TOKEN ./tanzaku-client
```

Precedence: command line > `TANZAKU_BOARD` / `TANZAKU_TOKEN` > `server.toml` / `client.toml` next to the executable (examples in `deploy/config/`).

## Building from source

Requires Rust 1.90 (see `rust-toolchain.toml`) and Node.js 20.

```bash
# admin UI (embedded into tz-board)
cd web-admin && npm ci && npm run build && cd ..
# backend
cargo build --release -p tz-board -p tz-server -p tz-client
# user theme package (upload via admin "Themes")
cd web-user && npm ci && npm run build
```

`deploy/build-linux-amd64.sh` cross-compiles static Linux binaries from macOS; see [deploy/README.md](deploy/README.md).

Pushing a `v*` tag triggers GitHub Actions to build every platform and publish a Release.

## Repository layout

```
crates/
  tz-board     control plane (API, scheduling, billing, embedded admin UI)
  tz-server    public node
  tz-client    private client
  tz-agent     shared node/client control channel
  tz-carrier   encrypted carriers (QUIC, multiplexed TCP)
  tz-ingress   ingress listeners (TCP / UDP / HTTP / shared HTTP(S))
  tz-guard     protection pipeline
  tz-net       rate limiting, metering, admission
  tz-pki       certificates and keys
  tz-proto     control protocol
  tz-common    shared utilities
web-admin/     admin UI
web-user/      user theme
deploy/        build scripts and config examples
install.sh     Linux / macOS installer (bash)
install.ps1    Windows installer
```

## License

No open-source license has been declared yet; contact the author before use.
