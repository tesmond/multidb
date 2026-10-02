#!/bin/sh
# Wrap the release binary in a macOS .app bundle and zip it.
# Icon: icons/icon_*.png (iconset sizes). Version: Cargo.toml.
# Run after `cargo build --release` (see `make build`).
# Output: target/release/MultiDB.app and target/release/MultiDB-<version>-macos-<arch>.zip
set -eu

ROOT=$(cd "$(dirname "$0")/../.." && pwd)
VERSION=${VERSION:-$(sed -n 's/^version *= *"\(.*\)"/\1/p' "$ROOT/Cargo.toml" | head -n 1)}
TARGET_DIR=${CARGO_TARGET_DIR:-$ROOT/target}
BIN="$TARGET_DIR/release/multidb"
APP="$TARGET_DIR/release/MultiDB.app"
ARCH=$(uname -m)
[ "$ARCH" = "x86_64" ] || ARCH=arm64
ZIP="$TARGET_DIR/release/MultiDB-$VERSION-macos-$ARCH.zip"

[ -x "$BIN" ] || { echo "bundle.sh: $BIN not found - run cargo build --release first" >&2; exit 1; }

rm -rf "$APP" "$ZIP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN" "$APP/Contents/MacOS/multidb"

# iconutil only accepts a directory named *.iconset
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT
mkdir "$WORK/multidb.iconset"
cp "$ROOT"/icons/icon_*.png "$WORK/multidb.iconset/"
iconutil -c icns "$WORK/multidb.iconset" -o "$APP/Contents/Resources/multidb.icns"

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key><string>MultiDB</string>
    <key>CFBundleDisplayName</key><string>MultiDB</string>
    <key>CFBundleExecutable</key><string>multidb</string>
    <key>CFBundleIdentifier</key><string>com.multidb.app</string>
    <key>CFBundleIconFile</key><string>multidb</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleShortVersionString</key><string>${VERSION}</string>
    <key>CFBundleVersion</key><string>${VERSION}</string>
    <key>LSMinimumSystemVersion</key><string>11.0</string>
    <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST

# Ad-hoc sign so the bundle launches on Apple Silicon (not notarised).
codesign --force --deep --sign - "$APP"
ditto -c -k --keepParent "$APP" "$ZIP"
echo "Created $APP and $ZIP"
