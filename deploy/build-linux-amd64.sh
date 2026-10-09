#!/usr/bin/env bash
# 交叉编译 Linux x86_64（静态 musl）产物。数字菜单交互，不靠命令行参数。
#
# 依赖（macOS）:
#   rustup target add x86_64-unknown-linux-musl
#   brew install FiloSottile/musl-cross/musl-cross
#
# 产物目录 dist/linux-amd64 仅保留：
#   tz-board  tanzaku-server  tanzaku-client  {short}-{version}.zip
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

TARGET="x86_64-unknown-linux-musl"
OUT_DIR="$ROOT/dist/linux-amd64"
LINKER_CANDIDATES=(
  "x86_64-linux-musl-gcc"
  "x86_64-unknown-linux-musl-gcc"
  "/opt/homebrew/bin/x86_64-linux-musl-gcc"
  "/opt/homebrew/opt/x86_64-unknown-linux-musl/bin/x86_64-linux-musl-gcc"
)

resolve_linker() {
  local candidate
  for candidate in "${LINKER_CANDIDATES[@]}"; do
    if command -v "$candidate" >/dev/null 2>&1; then
      command -v "$candidate"
      return 0
    fi
    if [[ -x "$candidate" ]]; then
      printf '%s\n' "$candidate"
      return 0
    fi
  done
  return 1
}

ensure_rust_target() {
  if ! rustup target list --installed | grep -qx "$TARGET"; then
    echo ">>> rustup target add $TARGET"
    rustup target add "$TARGET"
  fi
}

# 每次编译前清空产物目录，避免旧 web-admin / web-dist / themes 残留误导。
clean_out_dir() {
  echo ">>> 清空产物目录 $OUT_DIR"
  rm -rf "$OUT_DIR"
  mkdir -p "$OUT_DIR"
}

build_webadmin() {
  echo ">>> 构建 web-admin（嵌入 board，不复制到产物目录）"
  if [[ ! -d "$ROOT/web-admin/node_modules" ]]; then
    (cd "$ROOT/web-admin" && npm ci)
  fi
  (cd "$ROOT/web-admin" && npm run build)
  if [[ ! -f "$ROOT/web-admin/dist/index.html" ]]; then
    echo "错误: web-admin/dist/index.html 未生成" >&2
    exit 1
  fi
  # board 通过 rust-embed 嵌入 web-admin/dist；戳记强制 cargo 重嵌。
  date -u +%Y-%m-%dT%H:%M:%SZ >"$ROOT/crates/tz-board/.web-admin-embed"
  echo ">>> web-admin -> $ROOT/web-admin/dist （仅供 board 编译嵌入）"
}

# 构建 web-user，并打成可上传的主题 zip：
#   tanzaku-theme.json + dist/（+ 可选 preview.png）
# zip 内仍含 dist/（board 运行时要求）；产物目录只放这一份 zip，不再复制 themes/ 子目录。
build_usertheme_zip() {
  echo ">>> 构建 web-user 主题包"
  if ! command -v zip >/dev/null 2>&1; then
    echo "错误: 需要 zip 命令以打包主题" >&2
    exit 1
  fi
  if [[ ! -d "$ROOT/web-user/node_modules" ]]; then
    (cd "$ROOT/web-user" && npm ci)
  fi
  (cd "$ROOT/web-user" && npm run build)

  local manifest="$ROOT/web-user/tanzaku-theme.json"
  local dist_dir="$ROOT/web-user/dist"
  if [[ ! -f "$manifest" ]]; then
    echo "错误: 缺少 $manifest" >&2
    exit 1
  fi
  if [[ ! -f "$dist_dir/index.html" ]]; then
    echo "错误: 缺少 $dist_dir/index.html，前端构建失败？" >&2
    exit 1
  fi

  local short version
  short="$(
    node -e "const m=JSON.parse(require('fs').readFileSync(process.argv[1],'utf8')); process.stdout.write(String(m.short||''))" "$manifest"
  )"
  version="$(
    node -e "const m=JSON.parse(require('fs').readFileSync(process.argv[1],'utf8')); process.stdout.write(String(m.version||'0.0.0'))" "$manifest"
  )"
  if [[ -z "$short" ]]; then
    echo "错误: tanzaku-theme.json 缺少 short" >&2
    exit 1
  fi

  mkdir -p "$OUT_DIR"
  local staging="$OUT_DIR/.theme-staging-$$"
  local zip_name="${short}-${version}.zip"
  local zip_path="$OUT_DIR/$zip_name"
  rm -rf "$staging"
  mkdir -p "$staging/dist"
  cp "$manifest" "$staging/tanzaku-theme.json"
  cp -R "$dist_dir/." "$staging/dist/"
  if [[ -f "$ROOT/web-user/preview.png" ]]; then
    cp "$ROOT/web-user/preview.png" "$staging/preview.png"
  fi

  rm -f "$zip_path"
  (
    cd "$staging"
    zip -qr "$zip_path" tanzaku-theme.json dist
    if [[ -f preview.png ]]; then
      zip -q "$zip_path" preview.png
    fi
  )
  rm -rf "$staging"
  USERTHEME_ZIP_PATH="$zip_path"
  echo ">>> 用户主题 zip -> $zip_path ($(du -h "$zip_path" | awk '{print $1}'))"
  echo ">>> 管理端「用户主题」上传该 zip；解压路径 \$TANZAKU_DATA/theme/${short}/"
}

build_rust() {
  local packages=("$@")
  if [[ ${#packages[@]} -eq 0 ]]; then
    return 0
  fi
  ensure_rust_target
  local linker
  if ! linker="$(resolve_linker)"; then
    echo "错误: 找不到 musl 交叉编译器 x86_64-linux-musl-gcc" >&2
    echo "请安装: brew install FiloSottile/musl-cross/musl-cross" >&2
    exit 1
  fi
  export CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER="$linker"
  export CC_x86_64_unknown_linux_musl="$linker"
  export CMAKE_C_COMPILER="$linker"
  local cxx="${linker%-gcc}-g++"
  if [[ -x "$cxx" ]] || command -v "$cxx" >/dev/null 2>&1; then
    export CMAKE_CXX_COMPILER="$cxx"
  fi

  local -a cargo_args=()
  local pkg
  for pkg in "${packages[@]}"; do
    cargo_args+=(-p "$pkg")
  done

  echo ">>> 交叉编译 target=$TARGET linker=$linker packages=${packages[*]}"
  cargo build --release --target "$TARGET" "${cargo_args[@]}"
}

copy_bin() {
  local name="$1"
  local src="$ROOT/target/$TARGET/release/$name"
  if [[ ! -f "$src" ]]; then
    echo "错误: 未找到产物 $src" >&2
    exit 1
  fi
  mkdir -p "$OUT_DIR"
  cp "$src" "$OUT_DIR/$name"
  chmod +x "$OUT_DIR/$name"
  echo ">>> $name -> $OUT_DIR/$name ($(du -h "$OUT_DIR/$name" | awk '{print $1}'))"
}

pick_menu() {
  cat <<'EOF'

Tanzaku Linux amd64 交叉编译
----------------------------
  1) 全部（board + server + client + webadmin + 用户主题 zip）
  2) board（含 webadmin，因嵌入 dist）
  3) server
  4) client
  5) webadmin（仅构建嵌入用前端，不写产物目录）
  6) 用户主题 zip
  0) 退出

产物目录仅含：tz-board / tanzaku-server / tanzaku-client / 主题.zip
EOF
  local choice
  while true; do
    printf '请选择 [0-6]: '
    read -r choice
    case "$choice" in
      0) echo "已取消"; exit 0 ;;
      1|2|3|4|5|6) CHOICE="$choice"; return 0 ;;
      *) echo "无效输入，请重新选择" ;;
    esac
  done
}

WANT_BOARD=0
WANT_SERVER=0
WANT_CLIENT=0
WANT_WEBADMIN=0
WANT_USERTHEME=0
USERTHEME_ZIP_PATH=""

pick_menu
case "$CHOICE" in
  1)
    WANT_BOARD=1
    WANT_SERVER=1
    WANT_CLIENT=1
    WANT_WEBADMIN=1
    WANT_USERTHEME=1
    ;;
  2)
    WANT_BOARD=1
    WANT_WEBADMIN=1
    ;;
  3)
    WANT_SERVER=1
    ;;
  4)
    WANT_CLIENT=1
    ;;
  5)
    WANT_WEBADMIN=1
    ;;
  6)
    WANT_USERTHEME=1
    ;;
esac

clean_out_dir

if [[ "$WANT_WEBADMIN" -eq 1 ]]; then
  build_webadmin
elif [[ "$WANT_BOARD" -eq 1 ]]; then
  echo ">>> board 依赖嵌入的 web-admin，重新构建前端以保证一致"
  build_webadmin
fi

if [[ "$WANT_USERTHEME" -eq 1 ]]; then
  build_usertheme_zip
fi

RUST_PKGS=()
if [[ "$WANT_BOARD" -eq 1 ]]; then
  RUST_PKGS+=("tz-board")
fi
if [[ "$WANT_SERVER" -eq 1 ]]; then
  RUST_PKGS+=("tz-server")
fi
if [[ "$WANT_CLIENT" -eq 1 ]]; then
  RUST_PKGS+=("tz-client")
fi

if [[ ${#RUST_PKGS[@]} -gt 0 ]]; then
  build_rust "${RUST_PKGS[@]}"
fi

if [[ "$WANT_BOARD" -eq 1 ]]; then
  copy_bin tz-board
fi
if [[ "$WANT_SERVER" -eq 1 ]]; then
  copy_bin tanzaku-server
fi
if [[ "$WANT_CLIENT" -eq 1 ]]; then
  copy_bin tanzaku-client
fi

echo ">>> 完成: $OUT_DIR"
if [[ -n "$USERTHEME_ZIP_PATH" && -f "$USERTHEME_ZIP_PATH" ]]; then
  echo ">>> 主题 zip（上传用）: $USERTHEME_ZIP_PATH"
fi
echo ">>> 产物列表:"
ls -lh "$OUT_DIR"
