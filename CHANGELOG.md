# Changelog

## 0.2.0 — 2026-10-08

Rust rewrite release.

### Added

- Rebuilt the manager as a native Rust application using `eframe`/`egui`, with
  one shared codebase for Windows x64 and macOS Apple Silicon.
- Added the seven official upstream application icons to the existing two-column
  interface, with icon provenance and upstream license copies under `assets/icons`.
- Added macOS ARM64 install, update, remove, and launch support for verified DMGs.

### Changed

- Preserved the existing compact two-column UI while replacing the C#/Avalonia
  implementation and .NET build pipeline.
- GitHub Actions now builds and packages native Rust artifacts for Windows x64
  and macOS ARM64.
- Changed the manager license from Apache-2.0 to the ArtCraft Suite Manager
  Personal Use License 1.0. Upstream apps, icons, and dependencies retain their
  own licenses.

## 0.1.1 — 2026-10-07

Hotfix release.

### Fixed

- Changing the channel while a check or install was running could leave the cards
  showing the previous channel's versions, and Install could then install a build
  from the wrong channel. The channel picker is now locked while busy, and every
  install re-resolves against the channel currently shown.
- Per-app Install, Remove and Open buttons stayed enabled during other operations.
  Install silently did nothing, and Remove could delete an app while it was being
  installed. All actions are now disabled while the manager is busy.
- Switching channels no longer re-downloads release lists from GitHub. Lists are
  cached until **Refresh** is pressed, so checks use far fewer of GitHub's 60
  anonymous requests per hour (each Refresh uses one per app). When the limit is
  reached, the manager now says so and shows when to try again instead of a bare
  "403" error.
- If the newest release has no package for this platform, the newest release that
  does is offered instead of marking the app unavailable.
- Updating or removing an app that is still open now stops with a "close it and try
  again" message. Removal moves the folder aside before deleting it and reports
  any files that could not be cleaned up.
- **Install selected** now runs as one operation. The buttons no longer re-enable
  between apps, Cancel stops the whole batch, and the status line reports apps that
  failed instead of showing only the last result.
- A damaged `state.json` no longer silently erases the record of every installed
  app. The damaged file is kept aside and the list is rebuilt from the installed
  app folders. A file that is only briefly locked is retried, not treated as damaged.
  Installation restores the previous app if saving its new state fails.
- The action button now says **Downgrade** instead of **Update** when the selected
  channel offers an older version than the one installed.

### Added

- A **Cancel** button for checks and downloads. Cancelling never leaves an app
  half-installed.

### Changed

- Changed the manager's project license to Apache-2.0. Third-party applications
  and dependencies retain their own licenses.
- The User-Agent sent to GitHub now identifies this project and its version.
- Removed a leftover local NuGet package-folder setting and replaced the stale
  first-publish instructions in `PUBLISHING.md` with release steps.

## 0.1.0 — 2026-10-07

Initial release.
