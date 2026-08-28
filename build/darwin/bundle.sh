#!/usr/bin/env sh
set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
RELEASE_DIR="$REPO_ROOT/desktop/target/release"
APP_DIR="$RELEASE_DIR/MultiDB.app"

echo "Creating MultiDB.app bundle..."

rm -rf "$APP_DIR"
mkdir -p "$APP_DIR/Contents/MacOS"
mkdir -p "$APP_DIR/Contents/Resources"

cp "$RELEASE_DIR/multidb"               "$APP_DIR/Contents/MacOS/multidb"
cp "$SCRIPT_DIR/Info.plist"             "$APP_DIR/Contents/Info.plist"
cp "$REPO_ROOT/build/appicon.icns"      "$APP_DIR/Contents/Resources/appicon.icns"
chmod +x "$APP_DIR/Contents/MacOS/multidb"

echo "Zipping MultiDB.app..."
cd "$RELEASE_DIR"
rm -f MultiDB-macos.zip
zip -qr --symlinks MultiDB-macos.zip MultiDB.app

echo "Done: $RELEASE_DIR/MultiDB-macos.zip"
