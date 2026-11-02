#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN="$ROOT/target/release/grit"
if [[ ! -x "$BIN" ]]; then
  printf 'Release binary not found. Run cargo build --release first.\n' >&2
  exit 1
fi

ARCH="$(uname -m)"
case "$ARCH" in
  arm64|x86_64) ;;
  *) printf 'Unsupported macOS architecture: %s\n' "$ARCH" >&2; exit 1 ;;
esac

VERSION="$(awk -F '"' '/^version = / { print $2; exit }' "$ROOT/Cargo.toml")"
STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT
APP="$STAGE/grit.app"
CONTENTS="$APP/Contents"
mkdir -p "$CONTENTS/MacOS" "$CONTENTS/Resources"
cp "$BIN" "$CONTENTS/MacOS/grit"
cp "$ROOT/README.md" "$CONTENTS/Resources/README.md"
cp "$ROOT/LICENSE" "$CONTENTS/Resources/LICENSE"
cat > "$CONTENTS/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>grit</string>
  <key>CFBundleDisplayName</key><string>grit</string>
  <key>CFBundleIdentifier</key><string>io.github.grit</string>
  <key>CFBundleVersion</key><string>$VERSION</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleExecutable</key><string>grit</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.developer-tools</string>
  <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST
printf 'APPL????' > "$CONTENTS/PkgInfo"

mkdir -p "$ROOT/dist"
ditto -c -k --sequesterRsrc --keepParent \
  "$APP" "$ROOT/dist/grit-macos-$ARCH.zip"
printf 'Created %s\n' "$ROOT/dist/grit-macos-$ARCH.zip"
