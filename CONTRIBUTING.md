# Contributing

Thank you for helping improve ArtCraft Suite Manager.

By intentionally submitting a contribution for inclusion, you agree to the
contribution terms in [`LICENSE`](LICENSE), including the Project Author's right
to relicense that contribution as part of the Software. You retain copyright in
your contribution.

1. Open an issue before a large behavior or manifest change.
2. Create a focused branch and keep upstream application binaries out of Git.
3. Run `dotnet build -c Release` and `node scripts/validate-manifest.mjs`.
4. Explain user-facing and security implications in the pull request.

Asset patterns must be anchored, platform-specific, and narrow enough to avoid
CLI/web/source packages. Never weaken the SHA-256 requirement to work around an
upstream publishing problem; report that release as unavailable instead.
