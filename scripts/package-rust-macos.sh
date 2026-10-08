#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
arm64_binary="${1:?path to the Apple Silicon binary is required}"
x64_binary="${2:?path to the Intel binary is required}"
output_dir="${3:?output directory is required}"
version="${4:-0.1.1}"
app="$output_dir/ArtCraft Suite.app"
image="$output_dir/ArtCraftSuite-macos-universal.dmg"
staging="$output_dir/.dmg-staging"

[[ "$(uname -s)" == "Darwin" ]] || { echo "Build on macOS" >&2; exit 1; }
[[ -f "$arm64_binary" && -f "$x64_binary" ]] || { echo "Both architecture binaries must exist" >&2; exit 1; }
mkdir -p "$output_dir"
rm -rf "$app" "$staging" "$image"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
lipo -create "$arm64_binary" "$x64_binary" -output "$app/Contents/MacOS/ArtCraftSuite"
chmod 755 "$app/Contents/MacOS/ArtCraftSuite"
archs="$(lipo -archs "$app/Contents/MacOS/ArtCraftSuite")"
[[ " $archs " == *" arm64 "* && " $archs " == *" x86_64 "* ]] || { echo "Expected universal binary, got: $archs" >&2; exit 1; }
cp "$repo_root/LICENSE" "$repo_root/NOTICE" "$repo_root/THIRD_PARTY_NOTICES.md" "$app/Contents/Resources/"
cp "$repo_root/src/rust-macos/Cargo.lock" "$app/Contents/Resources/Cargo.lock"
cat > "$app/Contents/Resources/MODIFICATIONS.txt" <<'NOTICE'
ArtCraft Suite Manager by erikerikerik
https://github.com/erikerikerik/artcraft-suite

This is a modified universal macOS edition for Intel and Apple Silicon. It adds
a Rust windowed manager, verified macOS DMG installation, updates, opening,
removal, and DMG packaging. See LICENSE for the terms that apply to this version.
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
