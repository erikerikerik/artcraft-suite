# macOS Apple Silicon scaffold

The project already contains the cross-platform UI, `osx-arm64` publish target,
manifest rules for official `macos-universal.dmg` assets, and a GitHub Actions job
that emits an unsigned `.app` archive.

Before macOS support is promoted from scaffold to supported, implement and test:

1. Mount the verified DMG with `hdiutil attach -nobrowse -readonly`.
2. Discover exactly one expected `.app` bundle and validate its identifier.
3. Copy it atomically into a manager-owned location or `/Applications`, with an
   explicit permission choice in the UI.
4. Detach the image on success, failure, and cancellation.
5. Add launch, version discovery, update rollback, and uninstall behavior for
   bundles without deleting user data.
6. Sign and notarize the manager `.app`, then add staple and Gatekeeper checks to
   release automation. Signing secrets must remain in GitHub Actions secrets.

The current runtime deliberately refuses installation on macOS rather than
performing an incomplete or unsafe DMG operation.
