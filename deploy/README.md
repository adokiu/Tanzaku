# Linux amd64 交叉编译

在 macOS 上产出可在 Linux x86_64 运行的静态二进制（`x86_64-unknown-linux-musl`）。

## 依赖

```bash
rustup target add x86_64-unknown-linux-musl
brew install FiloSottile/musl-cross/musl-cross
```

## 用法

```bash
./deploy/build-linux-amd64.sh
```

启动后按数字选择：

| 选项 | 内容 |
|------|------|
| 1 | 全部（board + server + client + webadmin + 用户主题 zip） |
| 2 | board（会先编 webadmin） |
| 3 | server |
| 4 | client |
| 5 | webadmin |
| 6 | 用户主题 zip（构建 `web-user`，打成可上传包） |
| 0 | 退出 |

产物目录：`dist/linux-amd64/`（含二进制与 `*.toml.example`）

用户主题 zip 在产物根目录：`dist/linux-amd64/{short}-{version}.zip`（另有一份在 `themes/`），内含 `tanzaku-theme.json` + `dist/`。管理端「用户主题」上传后落到 `$TANZAKU_DATA/theme/{short}/`。

## 配置文件

示例源文件在 `deploy/config/`，运行时默认与可执行文件**同级**：

| 组件 | 默认路径 | 示例 |
|------|----------|------|
| board | `<exe>/board.toml` | `deploy/config/board.toml.example` |
| server | `<exe>/server.toml` | `deploy/config/server.toml.example` |
| client | `<exe>/client.toml` | `deploy/config/client.toml.example` |

路径优先级：`-c` / `--config` > 环境变量 `TANZAKU_CONFIG` > 默认同级 `*.toml`。

### board

```bash
cp deploy/config/board.toml.example /path/to/tz-board/board.toml
./tz-board
# 或
./tz-board -c /etc/tanzaku/board.toml
```

- `admin`（默认 `9000`）：管理前端 + server WebSocket `/ws`
- `user`（默认 `9001`）：用户前端 + client WebSocket `/ws`

监听地址可用 `TANZAKU_LISTEN_ADMIN` / `TANZAKU_LISTEN_USER` 覆盖。

### server / client

连接参数可写在配置文件，也可用命令行或环境变量覆盖：

```bash
cp deploy/config/server.toml.example /path/to/tanzaku-server/server.toml
# 编辑 board / token 后：
./tanzaku-server

# 或临时覆盖
./tanzaku-server --board https://board.example.com:9000 --token NODE_TOKEN
TANZAKU_BOARD=... TANZAKU_TOKEN=... ./tanzaku-client
```

字段优先级：`--board` / `--token` > `TANZAKU_BOARD` / `TANZAKU_TOKEN` > 配置文件。
