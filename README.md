# ArtCraft Suite Manager

A lightweight, independent Rust installer and update manager for the seven
[ArtCraft creative apps](https://github.com/storytold). One codebase builds for
Windows x64 and macOS Apple Silicon.

> **Independent source-available project:** this repository is not maintained, sponsored, or endorsed
> by storytold or the ArtCraft team. It downloads unmodified packages from their
> official GitHub releases.

## What works

- Finds PhotoCraft, VectorCraft, DesignCraft, FilmCraft, EffectCraft, LightCraft,
  and PrintCraft from a readable JSON manifest.
- Shows each application's official upstream icon, with the original icon licence
  and provenance preserved under [`assets/icons`](assets/icons/README.md).
- Offers **Stable** (newest non-prerelease) and **Latest** (newest release,
  including prereleases) channels.
- Installs official Windows x64 portable packages per-user, with no administrator
  prompt, under `%LOCALAPPDATA%\ArtCraftSuite\apps`.
- Requires SHA-256 verification before extraction. It prefers GitHub's immutable
  release-asset digest and falls back to the upstream `SHA256SUMS.txt` file.
- Stages updates before swapping directories, defends against ZIP path traversal,
  launches installed apps, and removes only manager-owned app directories.
- Mounts verified macOS DMGs read-only, copies the `.app` into the manager-owned
  per-user app directory, and detaches the image.
- Builds native Windows x64 and macOS ARM64 release artifacts in GitHub Actions.

## Download and run

Download `ArtCraftSuite-windows-x64.zip` from this repository's Releases page,
extract it, and run `ArtCraftSuite.exe`. Windows may show a SmartScreen warning
for unsigned community builds; review the release checksum and source before
continuing.

The manager itself does not need administrator rights. Each creative application
keeps its own settings and documents outside the manager-owned installation
folder, so updating or removing an app does not intentionally remove user work.

## Build locally

Install the current stable [Rust toolchain](https://rustup.rs/), then:

```powershell
cargo test --locked
cargo run --release
```

Build the same native Windows executable produced by CI:

```powershell
cargo build --release --locked --target x86_64-pc-windows-msvc
```

## Manifest

[`manifest/apps.json`](manifest/apps.json) is intentionally data-driven. Each app
declares its upstream `owner/repository` and anchored asset-name patterns. Tokens
`{id}` and `{version}` are escaped before matching. A manifest change cannot bypass
the runtime hash requirement.

## macOS Apple Silicon status

The shared Rust code builds for `aarch64-apple-darwin`, selects the official
universal DMGs, verifies them, and installs their app bundle into the same
manager-owned data model used on Windows. The GitHub artifact remains unsigned;
code signing, notarization, and hands-on Apple Silicon testing are still required
before calling it a polished public macOS release.

See [`docs/MACOS.md`](docs/MACOS.md) for the completion plan.

## Security and privacy

The app talks only to `api.github.com` and the GitHub release download URLs selected
from the manifest. It has no telemetry and does not require a GitHub token. GitHub's
unauthenticated API rate limit applies. See [`SECURITY.md`](SECURITY.md) for the
verification and reporting model.

## Contributing

Issues and pull requests are welcome. Please read [`CONTRIBUTING.md`](CONTRIBUTING.md).
By submitting a contribution, you agree to the contribution terms in the project
license.

## License

ArtCraft Suite Manager may be used, modified, and redistributed for personal,
non-commercial purposes with prominent source credit. Commercial use is prohibited
unless separately approved and licensed in writing by the original author. See
[`LICENSE`](LICENSE) for the complete terms.

This restriction means the manager is **source-available, not OSI open source**.
The upstream ArtCraft applications and other dependencies keep their own licenses
and attributions, listed in [`THIRD_PARTY_NOTICES.md`](THIRD_PARTY_NOTICES.md).

Repository owners can follow the exact steps in [`PUBLISHING.md`](PUBLISHING.md)
to push the source and create the first automated release.
