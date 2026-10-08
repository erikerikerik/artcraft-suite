#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
binary="${1:?path to Linux Rust binary is required}"
output_dir="${2:?output directory is required}"
version="${3:-$(sed -n 's/^version = "\(.*\)"/\1/p' "$repo_root/src/rust-linux/Cargo.toml" | head -1)}"
arch="$(dpkg --print-architecture)"

[[ "$arch" == "amd64" || "$arch" == "arm64" ]] || { echo "Unsupported architecture: $arch" >&2; exit 1; }
[[ -f "$binary" ]] || { echo "Binary not found: $binary" >&2; exit 1; }
mkdir -p "$output_dir"
output_dir="$(cd "$output_dir" && pwd)"
stage="$(mktemp -d "$output_dir/.deb-stage.XXXXXX")"
metadata="$(mktemp -d "$output_dir/.shlibdeps.XXXXXX")"
trap 'rm -rf "$stage" "$metadata"' EXIT
mkdir -p "$stage/DEBIAN" "$stage/usr/bin" "$stage/usr/share/applications" \
  "$stage/usr/share/icons/hicolor/scalable/apps" "$stage/usr/share/doc/artcraft-suite"
install -m 755 "$binary" "$stage/usr/bin/artcraft-suite"
install -m 644 "$repo_root/assets/artcraft-suite.svg" "$stage/usr/share/icons/hicolor/scalable/apps/artcraft-suite.svg"
install -m 644 "$repo_root/LICENSE" "$repo_root/NOTICE" "$repo_root/THIRD_PARTY_NOTICES.md" "$stage/usr/share/doc/artcraft-suite/"
install -m 644 "$repo_root/src/rust-linux/Cargo.lock" "$stage/usr/share/doc/artcraft-suite/Cargo.lock"
cat > "$stage/usr/share/doc/artcraft-suite/MODIFICATIONS" <<'NOTICE'
ArtCraft Suite Manager by erikerikerik
https://github.com/erikerikerik/artcraft-suite

This modified Ubuntu edition adds a Rust windowed manager, verified AppImage
installation, desktop launchers, updates, and removal.
NOTICE
cat > "$stage/usr/share/applications/artcraft-suite.desktop" <<'DESKTOP'
[Desktop Entry]
Type=Application
Name=ArtCraft Suite
Comment=Install and update ArtCraft creative applications
Exec=/usr/bin/artcraft-suite
TryExec=/usr/bin/artcraft-suite
Icon=artcraft-suite
Categories=Graphics;
Terminal=false
DESKTOP
if command -v desktop-file-validate >/dev/null; then
  desktop-file-validate "$stage/usr/share/applications/artcraft-suite.desktop"
fi

mkdir -p "$metadata/debian"
cat > "$metadata/debian/control" <<'BUILD_CONTROL'
Source: artcraft-suite
Section: graphics
Priority: optional
Maintainer: erikerikerik <erikerikerik@users.noreply.github.com>
Standards-Version: 4.7.0

Package: artcraft-suite
Architecture: any
Description: Windowed installer and update manager for ArtCraft creative apps
BUILD_CONTROL
dependencies="$(cd "$metadata" && dpkg-shlibdeps -O -e"$stage/usr/bin/artcraft-suite" | sed -n 's/^shlibs:Depends=//p')"
[[ -n "$dependencies" ]] || { echo "Could not determine shared library dependencies" >&2; exit 1; }
cat > "$stage/DEBIAN/control" <<CONTROL
Package: artcraft-suite
Version: $version
Section: graphics
Priority: optional
Architecture: $arch
Maintainer: erikerikerik <erikerikerik@users.noreply.github.com>
Depends: $dependencies, libgl1, libxkbcommon0, libxkbcommon-x11-0
Description: Windowed installer and update manager for ArtCraft creative apps
 Download verified ArtCraft AppImages, install them for the current user,
 and manage updates, launching, and removal without administrator access.
CONTROL

package="$output_dir/artcraft-suite_${version}_${arch}.deb"
dpkg-deb --build --root-owner-group "$stage" "$package"
(cd "$output_dir" && sha256sum "$(basename "$package")" > "$(basename "$package").sha256")
echo "$package"
