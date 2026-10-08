# Security policy

## Supported versions

Security fixes are applied to the newest released version.

## Release trust model

The manager queries releases only from repositories listed in the checked-in
manifest. It downloads the exact matching release asset and verifies SHA-256
against the GitHub release API `digest` field, or the official release's
`SHA256SUMS.txt` when that field is unavailable. Installation fails closed if
neither source provides a matching 64-character SHA-256.

Windows archives are extracted into a fresh staging directory. Absolute paths
and paths escaping that directory are rejected. Updates are swapped only after
successful extraction and executable discovery. Removal is restricted to the
manager's per-user apps directory.

The Ubuntu manager verifies the official AppImage before extraction. It stages
the extracted application under the user's XDG data directory, validates its
`AppRun` entry point, and keeps a previous version until its replacement and
state file are committed. Removal is limited to its manager-owned app directory
and desktop launcher.

This verifies transport and publisher-provided integrity; it is not a malware
guarantee. Community release artifacts are unsigned unless a release explicitly
says otherwise.

## Reporting a vulnerability

Use GitHub's private security advisory feature for the repository rather than a
public issue. Include affected version, reproduction steps, and expected impact.
Do not include secrets or personal files.
