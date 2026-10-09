# Universal macOS manager

The Finder-launchable manager is written in Rust with `eframe`. The installer is
one universal app containing both Intel x86_64 and Apple Silicon arm64 code. It
installs official universal DMGs from twelve upstream GitHub releases, listed in its
own app list, [`src/rust-macos/apps-macos.json`](../src/rust-macos/apps-macos.json).
The shared `manifest/apps.json` stays at the seven Windows apps.

An app is added to the Mac list only when its upstream release publishes a Developer
ID–signed universal DMG with a SHA-256 digest. `packageId` covers an app whose packages
use a different name: PrintCraft is published as `pdfcraft-<version>-macos-universal.dmg`
with bundle ID `ai.storyteller.pdfcraft`, and older `printcraft-*` packages still match.

## Using the manager

- Apps appear in one list, grouped as **Updates Available**, **Installed** and
  **Not Installed**, with each app's official icon (see `assets/icons/README.md`).
- **Get** installs, **Update** updates, **Open** launches. **Update All** queues every
  update. Requests made while another app is installing wait their turn; click a
  waiting app's ring to cancel it.
- The **⋯** button on an installed app offers Open, Show in Finder, Reinstall or
  Downgrade (when the selected channel offers the same or an older version), and
  Remove. Removal asks for confirmation first.
- **Stable** shows tested releases; **Latest** includes pre-releases. The refresh
  button (or ⌘R) checks GitHub again. Both are unavailable while apps are installing.
- The window follows the system light or dark appearance and uses the macOS system
  font when it can be read.

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
the matching upstream release asset for the running architecture, verifies
Apple signature integrity and Gatekeeper acceptance, and checks that the signing
team is Learning Machines LLC (Team ID `DJ6XS33FX8`). It also verifies that the
installed app includes a compatible executable slice. Updates and removal are
blocked while the app is running.

## Install behavior

1. Resolve the selected stable or latest release from the manifest repository.
2. Require a SHA-256 digest from the release asset or `SHA256SUMS.txt`.
3. Download and verify the complete DMG before mounting it read-only.
4. Discover exactly one `.app` bundle, remove disallowed FinderInfo attributes,
   verify its Developer ID signature, require Team ID `DJ6XS33FX8`, assess it with
   Gatekeeper, and verify its architecture compatibility.
5. Copy with `ditto` into a private staging directory, detach the image, and
   replace the managed app bundle with rollback if the swap or state write fails.
6. Refuse updates or removal while the app executable is open.
7. Launch with `open`; removal affects only the manager-owned app bundle. User
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
