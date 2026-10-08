# Universal macOS manager

The Finder-launchable manager is written in Rust with `eframe`. The installer is
one universal app containing both Intel x86_64 and Apple Silicon arm64 code. It
uses the shared app manifest and installs official universal DMGs from the seven
upstream GitHub releases.

## Build on macOS

Install stable Rust, Xcode Command Line Tools, and both Rust targets, then run:

```sh
rustup target add aarch64-apple-darwin x86_64-apple-darwin
cargo test --locked --manifest-path src/rust-macos/Cargo.toml
cargo build --release --locked --manifest-path src/rust-macos/Cargo.toml --target aarch64-apple-darwin
cargo build --release --locked --manifest-path src/rust-macos/Cargo.toml --target x86_64-apple-darwin
bash scripts/package-rust-macos.sh \
  src/rust-macos/target/aarch64-apple-darwin/release/artcraft-suite-macos \
  src/rust-macos/target/x86_64-apple-darwin/release/artcraft-suite-macos \
  artifacts/macos-universal
```

The packaging script merges both manager binaries with `lipo` and verifies that
the result contains both architectures. Open
`artifacts/macos-universal/ArtCraftSuite-macos-universal.dmg`, drag the app to
Applications, and launch it from Finder. It installs ArtCraft apps in
`~/Applications/ArtCraft Suite` without administrator access. The manager picks
the matching upstream release asset for the running architecture and verifies
the installed app includes a compatible executable slice.

## Install behavior

1. Resolve the selected stable or latest release from the manifest repository.
2. Require a SHA-256 digest from the release asset or `SHA256SUMS.txt`.
3. Download and verify the complete DMG before mounting it read-only.
4. Discover exactly one `.app` bundle and verify its identifier, executable, and
   architecture compatibility.
5. Copy with `ditto` into a private staging directory, detach the image, and
   replace the managed app bundle with rollback if the swap or state write fails.
6. Launch with `open`; removal affects only the manager-owned app bundle. User
   documents and app settings are outside that directory.

An existing app at the managed path without manager state is never overwritten.

## Signing and distribution

Local builds are ad hoc signed. They are usable for local testing but are not
notarized; Gatekeeper may require the user to approve opening the app. Set
`APPLE_DEVELOPER_ID` to a Developer ID Application certificate name when running
the packaging script to sign with hardened runtime. Public distribution should
also notarize and staple the DMG using an Apple Developer account before release.
GitHub Actions artifacts are ad hoc signed and are not notarized.

The older Avalonia macOS scaffold remains in the repository for reference. The
Rust universal DMG is the supported macOS installer target.
