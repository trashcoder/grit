#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ARCH="$(uname -m)"
if [[ "$ARCH" != "x86_64" ]]; then
  printf 'AppImage packaging currently supports x86_64; found %s\n' "$ARCH" >&2
  exit 1
fi

BIN="$ROOT/target/release/grit"
if [[ ! -x "$BIN" ]]; then
  printf 'Release binary not found. Run cargo build --release first.\n' >&2
  exit 1
fi

TOOL="${LINUXDEPLOY_APPIMAGE:-$ROOT/target/tools/linuxdeploy-x86_64.AppImage}"
if [[ ! -x "$TOOL" ]]; then
  mkdir -p "$(dirname "$TOOL")"
  curl --fail --location --retry 3 \
    https://github.com/linuxdeploy/linuxdeploy/releases/download/continuous/linuxdeploy-x86_64.AppImage \
    --output "$TOOL"
  chmod +x "$TOOL"
fi
RUNTIME="${APPIMAGETOOL_RUNTIME_FILE:-$ROOT/target/tools/runtime-x86_64}"
if [[ ! -f "$RUNTIME" ]]; then
  mkdir -p "$(dirname "$RUNTIME")"
  curl --fail --location --retry 3 \
    https://github.com/AppImage/type2-runtime/releases/download/continuous/runtime-x86_64 \
    --output "$RUNTIME"
fi

mkdir -p "$ROOT/dist"
OUTPUT_DIR="$ROOT/target/appimage-output"
mkdir -p "$OUTPUT_DIR"
mkdir -p "$ROOT/target/AppDir"
mkdir -p "$ROOT/target/AppDir/usr/share/licenses/grit"
cp "$ROOT/LICENSE" "$ROOT/target/AppDir/usr/share/licenses/grit/LICENSE"
rsvg-convert --width 256 --height 256 "$ROOT/packaging/grit.svg" > "$ROOT/target/AppDir/grit.png"
cd "$OUTPUT_DIR"
args=(
  --appdir "$ROOT/target/AppDir"
  --executable "$BIN"
  --desktop-file "$ROOT/packaging/io.github.grit.desktop"
  --icon-file "$ROOT/packaging/grit.svg"
)
# These are loaded by winit at runtime and may not appear in the executable's DT_NEEDED list.
for library in libwayland-client.so.0 libwayland-cursor.so.0 libwayland-egl.so.1 libxkbcommon.so.0 libxkbcommon-x11.so.0 libX11.so.6 libXcursor.so.1 libXrandr.so.2 libXi.so.6 libXfixes.so.3 libXext.so.6 libXrender.so.1 libxcb.so.1; do
  path="$(ldconfig -p 2>/dev/null | awk -v name="$library" '$1 == name {print $NF; exit}')"
  if [[ -n "$path" ]]; then args+=(--library "$path"); fi
done
APPIMAGETOOL_RUNTIME_FILE="$RUNTIME" ARCH=x86_64 "$TOOL" --appimage-extract-and-run "${args[@]}" --output appimage

shopt -s nullglob
images=("$OUTPUT_DIR"/grit*.AppImage)
if [[ ${#images[@]} -ne 1 ]]; then
  printf 'Expected one generated AppImage in %s, found %s\n' "$ROOT" "${#images[@]}" >&2
  exit 1
fi
mv "${images[0]}" "$ROOT/dist/grit-x86_64.AppImage"
printf 'Created %s\n' "$ROOT/dist/grit-x86_64.AppImage"
