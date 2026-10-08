#!/usr/bin/env bash
set -euo pipefail

binary="${1:?compiled ArtCraftSuite binary is required}"
app_dir="${2:?app directory is required}"

mkdir -p "$app_dir/Contents/MacOS" "$app_dir/Contents/Resources"
cp "$binary" "$app_dir/Contents/MacOS/ArtCraftSuite"
chmod +x "$app_dir/Contents/MacOS/ArtCraftSuite"

cat > "$app_dir/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleName</key><string>ArtCraft Suite</string>
  <key>CFBundleDisplayName</key><string>ArtCraft Suite</string>
  <key>CFBundleIdentifier</key><string>community.artcraft.suite</string>
  <key>CFBundleVersion</key><string>0.2.0</string>
  <key>CFBundleShortVersionString</key><string>0.2.0</string>
  <key>CFBundleExecutable</key><string>ArtCraftSuite</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>LSMinimumSystemVersion</key><string>12.0</string>
  <key>NSHighResolutionCapable</key><true/>
</dict></plist>
PLIST
