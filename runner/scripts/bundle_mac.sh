#!/bin/bash
# Wraps the release binary as the prebuilt Runner.app that install copies.
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
R="$root/target/Runner.app/Contents"
rm -rf "$root/target/Runner.app"; mkdir -p "$R/MacOS" "$R/Resources"
cp "$root/target/release/runner" "$R/MacOS/Runner"
cat > "$R/Info.plist" <<P
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>Runner</string>
<key>CFBundleIdentifier</key><string>app.openas.runner</string>
<key>CFBundleName</key><string>Runner</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleShortVersionString</key><string>0.1</string>
<key>CFBundleVersion</key><string>1</string>
<key>CFBundleIconFile</key><string>AppIcon</string>
<key>LSMinimumSystemVersion</key><string>11.0</string>
<key>NSHighResolutionCapable</key><true/>
<key>OAAppFolder</key><string></string>
<key>NSCameraUsageDescription</key><string>This app uses the camera only when you ask it to.</string>
<key>NSMicrophoneUsageDescription</key><string>This app uses the microphone only when you ask it to.</string>
</dict></plist>
P
codesign --force --sign - "$root/target/Runner.app"
echo "$root/target/Runner.app"
