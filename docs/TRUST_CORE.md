# v0.3 Trust Core

ArtCraft Suite v0.3 competes on dependable installation rather than feature count.

## Guarantees

- Packages come only from the repository and asset rules in the validated manifest.
- Every package must have an upstream SHA-256 and pass verification before extraction.
- Partial downloads resume when the upstream server supports HTTP ranges.
- ZIP traversal, symbolic links, excessive entry counts, and excessive declared
  expansion are rejected before activation.
- An update is staged separately and journaled before the existing installation moves.
- A failed or interrupted update restores the prior directory and state record.
- The immediately previous working version remains manager-owned for recovery.
- Release responses are cached with ETags and may be used as last-known-good data
  when GitHub is temporarily offline or rate limited.
- Errors remain visible in plain language and are recorded in structured local logs.

## Deliberate boundaries

v0.3 does not silently install updates, accept unverified packages, delete application
profiles, build applications from source, or place advanced lifecycle controls on the
main screen. Catalog expansion belongs to v0.4. Signing, self-update, user-selectable
backups, and notifications belong to v0.5.
