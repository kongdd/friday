#!/usr/bin/env bash
# Build a macOS app bundle without changing the installed app or login items.
set -euo pipefail

if [[ $(uname -s) != Darwin ]]; then
    echo "macOS is required" >&2
    exit 1
fi

cd "$(dirname "$0")/.."
target=${CARGO_TARGET_DIR:-"$PWD/target"}
mkdir -p "$target"
target=$(cd "$target" && pwd)
app="$target/Autossh.app"
staging=$(mktemp -d "$target/.autossh-app.XXXXXX")
trap 'rm -rf "$staging"' EXIT

cargo build --release --target-dir "$target" -p autossh-ui -p autossh-core
mkdir -p "$staging/Autossh.app/Contents/MacOS"
cp "$target/release/autossh-ui" "$target/release/autossh-core" \
    "$staging/Autossh.app/Contents/MacOS/"

cat > "$staging/Autossh.app/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleExecutable</key><string>autossh-ui</string>
    <key>CFBundleIdentifier</key><string>com.kongdd.autossh</string>
    <key>CFBundleName</key><string>Autossh</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleShortVersionString</key><string>0.1.0</string>
    <key>LSUIElement</key><true/>
    <key>NSMicrophoneUsageDescription</key><string>Friday 需要使用麦克风录制语音。</string>
</dict>
</plist>
PLIST

codesign --force --deep --sign "${CODESIGN_IDENTITY:--}" "$staging/Autossh.app"
rm -rf "$app"
mv "$staging/Autossh.app" "$app"
printf 'App: %s\n' "$app"
