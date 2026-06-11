#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
target_dir="$repo_root/desktop/target/release"
binary="$target_dir/multidb"
app_dir="$target_dir/multidb.app"
contents_dir="$app_dir/Contents"
macos_dir="$contents_dir/MacOS"
resources_dir="$contents_dir/Resources"

if [[ "${1:-}" != "--no-build" ]]; then
  cargo build --release --manifest-path "$repo_root/desktop/Cargo.toml"
fi

if [[ ! -x "$binary" ]]; then
  echo "Expected release binary at $binary" >&2
  exit 1
fi

mkdir -p "$macos_dir" "$resources_dir"
cp "$binary" "$macos_dir/multidb"
cp "$repo_root/build/appicon.icns" "$resources_dir/appicon.icns"

cat > "$contents_dir/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleDevelopmentRegion</key>
    <string>en</string>
    <key>CFBundleExecutable</key>
    <string>multidb</string>
    <key>CFBundleIconFile</key>
    <string>appicon</string>
    <key>CFBundleIdentifier</key>
    <string>com.multidb.app</string>
    <key>CFBundleName</key>
    <string>multidb</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleShortVersionString</key>
    <string>0.3.0</string>
    <key>CFBundleVersion</key>
    <string>0.3.0</string>
    <key>LSMinimumSystemVersion</key>
    <string>10.13.0</string>
    <key>NSHighResolutionCapable</key>
    <true/>
</dict>
</plist>
PLIST

chmod +x "$macos_dir/multidb"
echo "Created $app_dir"
