#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
binary="${1:?path to the compiled Apple Silicon binary is required}"
output_dir="${2:?output directory is required}"
version="${3:-0.1.0}"
app="$output_dir/ArtCraft Suite.app"
image="$output_dir/ArtCraftSuite-macos-arm64.dmg"
staging="$output_dir/.dmg-staging"

[[ "$(uname -s)" == "Darwin" && "$(uname -m)" == "arm64" ]] || { echo "Build on an Apple Silicon Mac" >&2; exit 1; }
[[ -f "$binary" ]] || { echo "Binary not found: $binary" >&2; exit 1; }
mkdir -p "$output_dir"
rm -rf "$app" "$staging" "$image"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$binary" "$app/Contents/MacOS/ArtCraftSuite"
chmod 755 "$app/Contents/MacOS/ArtCraftSuite"
cp "$repo_root/LICENSE" "$repo_root/THIRD_PARTY_NOTICES.md" "$app/Contents/Resources/"
cp "$repo_root/src/rust-macos/Cargo.lock" "$app/Contents/Resources/Cargo.lock"
cat > "$app/Contents/Resources/MODIFICATIONS.txt" <<'NOTICE'
ArtCraft Suite Manager by erikerikerik
https://github.com/erikerikerik/artcraft-suite

This is a modified Apple Silicon edition. It adds a Rust windowed manager,
verified macOS DMG installation, updates, opening, removal, and DMG packaging.
See LICENSE for the terms that apply to this modified version.
NOTICE

cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleName</key><string>ArtCraft Suite</string>
  <key>CFBundleDisplayName</key><string>ArtCraft Suite</string>
  <key>CFBundleIdentifier</key><string>community.artcraft.suite.rust</string>
  <key>CFBundleVersion</key><string>$version</string>
  <key>CFBundleShortVersionString</key><string>$version</string>
  <key>CFBundleExecutable</key><string>ArtCraftSuite</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>LSMinimumSystemVersion</key><string>12.0</string>
  <key>NSHighResolutionCapable</key><true/>
</dict></plist>
PLIST
plutil -lint "$app/Contents/Info.plist"

if [[ -n "${APPLE_DEVELOPER_ID:-}" ]]; then
  codesign --force --options runtime --timestamp --sign "$APPLE_DEVELOPER_ID" "$app"
else
  codesign --force --sign - "$app"
fi
codesign --verify --deep --strict "$app"

mkdir -p "$staging"
cp -R "$app" "$staging/ArtCraft Suite.app"
ln -s /Applications "$staging/Applications"
hdiutil create -quiet -volname "ArtCraft Suite" -srcfolder "$staging" -ov -format UDZO "$image"
rm -rf "$staging"
(cd "$output_dir" && shasum -a 256 "$(basename "$image")" > "$(basename "$image").sha256")
echo "$image"
