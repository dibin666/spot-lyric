#!/usr/bin/env bash
set -Eeuo pipefail

ROOT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
GTK_DIR="$ROOT_DIR/spot-lyric-gtk"
DAEMON_DIR="$ROOT_DIR/spot-lyric-daemon"
APP_NAME="spot-lyric-gtk"
PROFILE="debug"
FORCE_BUILD=0
APP_ARGS=()

usage() {
  cat <<USAGE
用法: ./run.sh [--debug|--release] [--force] [-- 应用参数...]

一键编译并启动 $APP_NAME：
  - 已存在最新构建产物时，直接启动，跳过 cargo build。
  - 源码、资源、Cargo 配置或 path 依赖发生变化时，执行 cargo 增量编译后启动。

选项:
  --debug      使用 debug 构建产物（默认）
  --release    使用 release 构建产物
  --force,-f   强制重新调用 cargo build
  --help,-h    显示帮助
USAGE
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --debug)
      PROFILE="debug"
      shift
      ;;
    --release)
      PROFILE="release"
      shift
      ;;
    --force|-f)
      FORCE_BUILD=1
      shift
      ;;
    --help|-h)
      usage
      exit 0
      ;;
    --)
      shift
      APP_ARGS+=("$@")
      break
      ;;
    *)
      APP_ARGS+=("$1")
      shift
      ;;
  esac
done

TARGET_DIR="$GTK_DIR/target"
BIN_PATH="$TARGET_DIR/$PROFILE/$APP_NAME"
STAMP_PATH="$TARGET_DIR/.${APP_NAME}-${PROFILE}.inputs.sha256"
SCHEMA_SOURCE="$GTK_DIR/data/cn.spotlyric.Gtk.gschema.xml"
SCHEMA_TARGET_DIR="$TARGET_DIR/$PROFILE/gsettings-schemas"
SCHEMA_COMPILED="$SCHEMA_TARGET_DIR/gschemas.compiled"
ICON_SOURCE="$GTK_DIR/data/icons/scalable/apps/cn.spotlyric.Gtk.svg"
ICON_THEME_DIR="$TARGET_DIR/$PROFILE/icons"
ICON_THEME_INDEX="$ICON_THEME_DIR/hicolor/index.theme"
ICON_TARGET="$ICON_THEME_DIR/hicolor/scalable/apps/cn.spotlyric.Gtk.svg"

INPUT_PATHS=(
  "$GTK_DIR/Cargo.toml"
  "$GTK_DIR/Cargo.lock"
  "$GTK_DIR/build.rs"
  "$GTK_DIR/src"
  "$GTK_DIR/resources"
  "$GTK_DIR/data"
  "$DAEMON_DIR/Cargo.toml"
  "$DAEMON_DIR/Cargo.lock"
  "$DAEMON_DIR/build.rs"
  "$DAEMON_DIR/src"
  "$DAEMON_DIR/data"
)

list_input_files() {
  local path
  for path in "${INPUT_PATHS[@]}"; do
    [[ -e "$path" ]] || continue

    if [[ -f "$path" ]]; then
      printf '%s\0' "$path"
    elif [[ -d "$path" ]]; then
      find "$path" \
        \( -path "$GTK_DIR/data/gschemas.compiled" -o -path '*/target/*' \) -prune -o \
        -type f -print0
    fi
  done | LC_ALL=C sort -z
}

current_input_hash() {
  list_input_files | xargs -0 -r sha256sum | sha256sum | awk '{print $1}'
}

newer_input_than_binary() {
  local file
  while IFS= read -r -d '' file; do
    if [[ "$file" -nt "$BIN_PATH" ]]; then
      printf '%s\n' "${file#$ROOT_DIR/}"
      return 0
    fi
  done < <(list_input_files)

  return 1
}

needs_build_reason() {
  local current_hash stored_hash newer_file

  if [[ "$FORCE_BUILD" == "1" ]]; then
    printf '%s\n' "已指定 --force"
    return 0
  fi

  if [[ ! -x "$BIN_PATH" ]]; then
    printf '%s\n' "构建产物不存在: ${BIN_PATH#$ROOT_DIR/}"
    return 0
  fi

  current_hash="$(current_input_hash)"

  if [[ -f "$STAMP_PATH" ]]; then
    stored_hash="$(<"$STAMP_PATH")"
    if [[ "$stored_hash" == "$current_hash" ]]; then
      return 1
    fi

    printf '%s\n' "构建输入已变化"
    return 0
  fi

  if newer_file="$(newer_input_than_binary)"; then
    printf '%s\n' "构建输入比产物更新: $newer_file"
    return 0
  fi

  mkdir -p "$TARGET_DIR"
  printf '%s\n' "$current_hash" > "$STAMP_PATH"
  return 1
}

build_app() {
  local cargo_args=(build --bin "$APP_NAME")

  if [[ "$PROFILE" == "release" ]]; then
    cargo_args+=(--release)
  fi

  echo "[run] 执行增量编译: cargo ${cargo_args[*]}"
  (
    cd "$GTK_DIR"
    cargo "${cargo_args[@]}"
  )

  mkdir -p "$TARGET_DIR"
  current_input_hash > "$STAMP_PATH"
}

prepare_gsettings_schema() {
  if [[ ! -f "$SCHEMA_SOURCE" ]]; then
    echo "[run] 启动失败，未找到 GSettings schema: $SCHEMA_SOURCE" >&2
    exit 1
  fi

  mkdir -p "$SCHEMA_TARGET_DIR"

  if [[ ! -f "$SCHEMA_COMPILED" || "$SCHEMA_SOURCE" -nt "$SCHEMA_COMPILED" ]]; then
    echo "[run] 编译 GSettings schema: ${SCHEMA_TARGET_DIR#$ROOT_DIR/}"
    glib-compile-schemas --strict --targetdir "$SCHEMA_TARGET_DIR" "$GTK_DIR/data"
  fi

  export GSETTINGS_SCHEMA_DIR="$SCHEMA_TARGET_DIR"
}

prepare_tray_icon() {
  if [[ ! -f "$ICON_SOURCE" ]]; then
    echo "[run] 启动失败，未找到托盘图标: $ICON_SOURCE" >&2
    exit 1
  fi

  mkdir -p "$(dirname -- "$ICON_TARGET")"
  if [[ ! -f "$ICON_THEME_INDEX" ]]; then
    cat > "$ICON_THEME_INDEX" <<'EOF'
[Icon Theme]
Name=hicolor
Comment=Fallback icon theme for Spot-Lyric development runs
Directories=scalable/apps

[scalable/apps]
Size=128
Type=Scalable
Context=Applications
MinSize=16
MaxSize=512
EOF
  fi

  if [[ ! -f "$ICON_TARGET" || "$ICON_SOURCE" -nt "$ICON_TARGET" ]]; then
    echo "[run] 准备托盘图标: ${ICON_TARGET#$ROOT_DIR/}"
    cp "$ICON_SOURCE" "$ICON_TARGET"
  fi

  export SPOT_LYRIC_ICON_THEME_PATH="$ICON_THEME_DIR"
}

if reason="$(needs_build_reason)"; then
  echo "[run] 需要编译：$reason"
  build_app
else
  echo "[run] 构建产物已是最新，跳过编译: ${BIN_PATH#$ROOT_DIR/}"
fi

if [[ ! -x "$BIN_PATH" ]]; then
  echo "[run] 启动失败，未找到可执行文件: $BIN_PATH" >&2
  exit 1
fi

prepare_gsettings_schema
prepare_tray_icon

echo "[run] 启动: ${BIN_PATH#$ROOT_DIR/} ${APP_ARGS[*]}"
exec "$BIN_PATH" "${APP_ARGS[@]}"
