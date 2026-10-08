# ArtCraft Suite Manager

A lightweight, independent installer and update manager for the seven open-source
[ArtCraft crafting apps](https://github.com/storytold). Windows x64 and a separate
Rust universal macOS manager for Intel and Apple Silicon are available.

> **Independent open-source project:** this repository is not maintained, sponsored, or endorsed
> by storytold or the ArtCraft team. It downloads unmodified packages from their
> official GitHub releases.

## What works

- Finds PhotoCraft, VectorCraft, DesignCraft, FilmCraft, EffectCraft, LightCraft,
  and PrintCraft from a readable JSON manifest.
- Offers **Stable** (newest non-prerelease) and **Latest** (newest release,
  including prereleases) channels.
- Installs official Windows x64 portable packages per-user, with no administrator
  prompt, under `%LOCALAPPDATA%\ArtCraftSuite\apps`.
- Requires SHA-256 verification before extraction. It prefers GitHub's immutable
  release-asset digest and falls back to the upstream `SHA256SUMS.txt` file.
- Stages updates before swapping directories, defends against ZIP path traversal,
  launches installed apps, and removes only manager-owned app directories.
- Builds a self-contained Windows x64 release artifact in GitHub Actions.

## Download and run

Download `ArtCraftSuite-windows-x64.zip` from this repository's Releases page,
extract it, and run `ArtCraftSuite.exe`. Windows may show a SmartScreen warning
for unsigned community builds; review the release checksum and source before
continuing.

The manager itself does not need administrator rights. Each creative application
keeps its own settings and documents outside the manager-owned installation
folder, so updating or removing an app does not intentionally remove user work.

## Build locally

Install the [.NET 8 SDK](https://dotnet.microsoft.com/download/dotnet/8.0), then:

```powershell
dotnet restore
dotnet build -c Release
dotnet run --project src/ArtCraftSuite
```

Publish the same self-contained Windows build produced by CI:

```powershell
dotnet publish src/ArtCraftSuite/ArtCraftSuite.csproj -c Release -r win-x64 \
  --self-contained true -p:PublishSingleFile=true -p:IncludeNativeLibrariesForSelfExtract=true
```

## Manifest

[`manifest/apps.json`](manifest/apps.json) is intentionally data-driven. Each app
declares its upstream `owner/repository` and anchored asset-name patterns. Tokens
`{id}` and `{version}` are escaped before matching. A manifest change cannot bypass
the runtime hash requirement.

## macOS Intel and Apple Silicon

The macOS manager is a windowed Rust application in `src/rust-macos`. It installs
the official universal DMGs into `~/Applications/ArtCraft Suite`, verifies SHA-256
before mounting, and supports updates, opening, and removal. It requires no
administrator password. See [`docs/MACOS.md`](docs/MACOS.md) for build and
distribution instructions. The older Avalonia macOS scaffold remains in the
repository but is not the installer.

## Ubuntu Linux

The Ubuntu manager is a windowed Rust application in `src/rust-linux`. It installs
verified official AppImages into your user data folder, adds app launchers, and
supports updates and removal without administrator access. Debian packages are
built for Ubuntu on x86-64 and ARM64. See [`docs/LINUX.md`](docs/LINUX.md) for
build and packaging instructions.

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

ArtCraft Suite Manager is licensed under the Apache License, Version 2.0.
See [`LICENSE`](LICENSE) for the terms and [`NOTICE`](NOTICE) for attribution.
The upstream ArtCraft applications and other dependencies keep their own licenses
and attributions, listed in [`THIRD_PARTY_NOTICES.md`](THIRD_PARTY_NOTICES.md).

Repository owners can follow the exact steps in [`PUBLISHING.md`](PUBLISHING.md)
to push the source and create the first automated release.
