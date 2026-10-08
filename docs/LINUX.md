# Ubuntu Linux manager

The Ubuntu manager is a windowed Rust app. A `.deb` installs its launcher in the
Ubuntu application menu. The manager then downloads official, SHA-256 verified
AppImages for x86-64 or ARM64 and extracts each app into the current user's XDG
data directory. This avoids FUSE runtime requirements and administrator prompts
when installing the creative apps.

## Build on Ubuntu 24.04

Install a current Rust toolchain and `dpkg-dev`, then:

```sh
cargo test --locked --manifest-path src/rust-linux/Cargo.toml
cargo build --release --locked --manifest-path src/rust-linux/Cargo.toml
bash scripts/package-linux.sh \
  src/rust-linux/target/release/artcraft-suite-linux artifacts/linux
```

Install `artcraft-suite_0.1.0_amd64.deb` or the ARM64 equivalent by opening it
in Ubuntu's package installer. Launch **ArtCraft Suite** from the app menu.

Installed apps live under `$XDG_DATA_HOME/artcraft-suite/apps` (normally
`~/.local/share/artcraft-suite/apps`). Their app menu launchers live under
`$XDG_DATA_HOME/applications`. The manager keeps only its own files there;
removal leaves user documents and application settings in place.

The updater stages a verified release, checks for an executable `AppRun`, then
replaces the managed app and launcher. It restores the prior installation if
the swap or state save fails. An app or launcher at the managed path without
manager state is not overwritten.

The Linux branch is separate from the macOS Rust branch and the original C#
Windows code on `main`.
