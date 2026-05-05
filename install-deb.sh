#!/usr/bin/env bash
set -Eeuo pipefail

ROOT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
GTK_DIR="$ROOT_DIR/spot-lyric-gtk"
PACKAGE_NAME="spot-lyric-gtk"
APP_ID="cn.spotlyric.Gtk"
BIN_NAME="spot-lyric-gtk"
INSTALL_DEB=1

usage() {
  cat <<USAGE
用法: ./install-deb.sh [--no-install] [--help]

构建 release 版本，打包成 .deb，并默认安装到系统。

选项:
  --no-install  只生成 .deb，不执行安装
  --help,-h     显示帮助
USAGE
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --no-install)
      INSTALL_DEB=0
      shift
      ;;
    --help|-h)
      usage
      exit 0
      ;;
    *)
      echo "[deb] 未知参数: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

require_command() {
  local cmd="$1"
  if ! command -v "$cmd" >/dev/null 2>&1; then
    echo "[deb] 缺少命令: $cmd" >&2
    exit 1
  fi
}

run_as_root() {
  if [[ "${EUID:-$(id -u)}" -eq 0 ]]; then
    "$@"
  else
    require_command sudo
    sudo "$@"
  fi
}

require_command cargo
require_command dpkg-deb
require_command dpkg
require_command glib-compile-schemas

VERSION="$(awk -F '"' '/^version = / { print $2; exit }' "$GTK_DIR/Cargo.toml")"
ARCH="$(dpkg --print-architecture)"
TARGET_DIR="$GTK_DIR/target"
RELEASE_BIN="$TARGET_DIR/release/$BIN_NAME"
DEB_WORK_DIR="$TARGET_DIR/deb"
PKG_ROOT="$DEB_WORK_DIR/${PACKAGE_NAME}_${VERSION}_${ARCH}"
DEB_PATH="$DEB_WORK_DIR/${PACKAGE_NAME}_${VERSION}_${ARCH}.deb"

DESKTOP_SOURCE="$GTK_DIR/data/$APP_ID.desktop"
SCHEMA_SOURCE="$GTK_DIR/data/$APP_ID.gschema.xml"
ICON_SOURCE="$GTK_DIR/data/icons/scalable/apps/$APP_ID.svg"

DEPS=()

add_dep() {
  local dep="$1"
  local existing
  for existing in "${DEPS[@]}"; do
    [[ "$existing" == "$dep" ]] && return 0
  done
  DEPS+=("$dep")
}

add_runtime_dep_for_so() {
  local so_name="$1"
  local lib_path real_path owner package

  lib_path="$(ldd "$RELEASE_BIN" | awk -v so="$so_name" '$1 == so { print $3; exit }')"
  [[ -n "$lib_path" && -e "$lib_path" ]] || return 0

  real_path="$(readlink -f "$lib_path")"
  owner="$(dpkg-query -S "$real_path" 2>/dev/null | head -n 1 || true)"
  [[ -n "$owner" ]] || return 0

  package="${owner%%:*}"
  [[ -n "$package" ]] && add_dep "$package"
}

join_deps() {
  local joined=""
  local dep
  for dep in "${DEPS[@]}"; do
    if [[ -z "$joined" ]]; then
      joined="$dep"
    else
      joined="$joined, $dep"
    fi
  done
  printf '%s\n' "$joined"
}

echo "[deb] 构建 release: cargo build --release --bin $BIN_NAME"
(
  cd "$GTK_DIR"
  cargo build --release --bin "$BIN_NAME"
)

for file in "$RELEASE_BIN" "$DESKTOP_SOURCE" "$SCHEMA_SOURCE" "$ICON_SOURCE"; do
  if [[ ! -e "$file" ]]; then
    echo "[deb] 打包失败，缺少文件: $file" >&2
    exit 1
  fi
done

add_dep "ca-certificates"
add_dep "hicolor-icon-theme"
add_dep "libglib2.0-bin"
for so_name in \
  libc.so.6 \
  libgtk-4.so.1 \
  libadwaita-1.so.0 \
  libglib-2.0.so.0 \
  libgio-2.0.so.0 \
  libgobject-2.0.so.0 \
  libdbus-1.so.3 \
  libssl.so.3 \
  libcrypto.so.3 \
  libX11.so.6 \
  libXext.so.6 \
  libXrandr.so.2
do
  add_runtime_dep_for_so "$so_name"
done
DEPENDS="$(join_deps)"

rm -rf "$PKG_ROOT"
mkdir -p \
  "$PKG_ROOT/DEBIAN" \
  "$PKG_ROOT/usr/bin" \
  "$PKG_ROOT/usr/share/applications" \
  "$PKG_ROOT/usr/share/glib-2.0/schemas" \
  "$PKG_ROOT/usr/share/icons/hicolor/scalable/apps"

install -Dm755 "$RELEASE_BIN" "$PKG_ROOT/usr/bin/$BIN_NAME"
install -Dm644 "$DESKTOP_SOURCE" "$PKG_ROOT/usr/share/applications/$APP_ID.desktop"
install -Dm644 "$SCHEMA_SOURCE" "$PKG_ROOT/usr/share/glib-2.0/schemas/$APP_ID.gschema.xml"
install -Dm644 "$ICON_SOURCE" "$PKG_ROOT/usr/share/icons/hicolor/scalable/apps/$APP_ID.svg"

INSTALLED_SIZE="$(du -sk "$PKG_ROOT/usr" | awk '{print $1}')"
cat > "$PKG_ROOT/DEBIAN/control" <<EOF
Package: $PACKAGE_NAME
Version: $VERSION
Section: sound
Priority: optional
Architecture: $ARCH
Maintainer: Spot-Lyric <noreply@localhost>
Installed-Size: $INSTALLED_SIZE
Depends: $DEPENDS
Description: Desktop lyrics overlay for Spotify
 Spot-Lyric GTK provides synchronized desktop lyrics for Spotify and
 a StatusNotifier tray menu.
EOF

cat > "$PKG_ROOT/DEBIAN/postinst" <<'EOF'
#!/bin/sh
set -e

if command -v glib-compile-schemas >/dev/null 2>&1; then
  glib-compile-schemas /usr/share/glib-2.0/schemas
fi

if command -v gtk-update-icon-cache >/dev/null 2>&1; then
  gtk-update-icon-cache -q -t -f /usr/share/icons/hicolor || true
fi

if command -v update-desktop-database >/dev/null 2>&1; then
  update-desktop-database -q /usr/share/applications || true
fi

exit 0
EOF

cat > "$PKG_ROOT/DEBIAN/postrm" <<'EOF'
#!/bin/sh
set -e

if command -v glib-compile-schemas >/dev/null 2>&1; then
  glib-compile-schemas /usr/share/glib-2.0/schemas || true
fi

if command -v gtk-update-icon-cache >/dev/null 2>&1; then
  gtk-update-icon-cache -q -t -f /usr/share/icons/hicolor || true
fi

if command -v update-desktop-database >/dev/null 2>&1; then
  update-desktop-database -q /usr/share/applications || true
fi

exit 0
EOF

chmod 0755 "$PKG_ROOT/DEBIAN/postinst" "$PKG_ROOT/DEBIAN/postrm"
find "$PKG_ROOT" -type d -exec chmod 0755 {} +

mkdir -p "$DEB_WORK_DIR"
dpkg-deb --build --root-owner-group "$PKG_ROOT" "$DEB_PATH"
echo "[deb] 已生成: ${DEB_PATH#$ROOT_DIR/}"

if [[ "$INSTALL_DEB" == "1" ]]; then
  if command -v apt-get >/dev/null 2>&1; then
    echo "[deb] 安装: apt-get install ${DEB_PATH#$ROOT_DIR/}"
    run_as_root apt-get install -y "$DEB_PATH"
  else
    echo "[deb] 安装: dpkg -i ${DEB_PATH#$ROOT_DIR/}"
    run_as_root dpkg -i "$DEB_PATH"
  fi
fi
