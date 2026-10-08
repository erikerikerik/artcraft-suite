# macOS Apple Silicon support

The Rust application shares its UI, manifest, release resolver, verification,
state, and install model across Windows and macOS. The `aarch64-apple-darwin`
workflow emits an unsigned `.app` archive.

The manager already mounts verified DMGs read-only, discovers an `.app`, copies
it into its manager-owned per-user directory, detaches the image, launches it,
and removes only the corresponding manager-owned directory.

Before macOS support is promoted from preview to supported:

1. Test all seven current upstream DMGs on physical Apple Silicon hardware.
2. Validate each discovered bundle identifier against a manifest allow-list.
3. Add atomic bundle update rollback equivalent to the Windows directory swap.
4. Add cancellation around `hdiutil` and `ditto` while guaranteeing detach.
5. Sign and notarize the manager `.app`, then add staple and Gatekeeper checks to
   release automation. Signing secrets must remain in GitHub Actions secrets.
