# macOS Apple Silicon manager

The Finder-launchable macOS manager is written in Rust with `eframe` and uses the
same checked-in app manifest as the Windows manager. It downloads official
universal DMGs from the seven upstream GitHub releases.

## Build on an Apple Silicon Mac

Install a current stable Rust toolchain and Xcode Command Line Tools, then run:

```sh
cargo test --locked --manifest-path src/rust-macos/Cargo.toml
cargo build --release --locked --manifest-path src/rust-macos/Cargo.toml
bash scripts/package-rust-macos.sh \
  src/rust-macos/target/release/artcraft-suite-macos artifacts/macos-arm64
```

Open `artifacts/macos-arm64/ArtCraftSuite-macos-arm64.dmg`, drag the app to
Applications, and launch it from Finder. The manager installs ArtCraft apps in
`~/Applications/ArtCraft Suite`. It does not require administrator access.

## Install behavior

1. Resolve the selected stable or latest release from the manifest repository.
2. Require a SHA-256 digest from the release asset or `SHA256SUMS.txt`.
3. Download and verify the complete DMG before mounting it read-only.
4. Discover exactly one `.app` bundle and verify its identifier and executable.
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
The current GitHub Actions artifact is ad hoc signed and is not notarized.

The older Avalonia `osx-arm64` scaffold remains in the repository for reference.
The Rust DMG is the supported macOS installer target.
