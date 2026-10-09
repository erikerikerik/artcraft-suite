# Changelog

## 0.3.4 — 2026-10-09

### Fixed

- Updating PhotoCraft or PrintCraft (PdfCraft) reset its settings, presets and
  crash-recovery files. ArtCraft's Windows ZIPs include `portable.txt`, which makes
  these apps keep that data in a folder beside the program, and each update
  replaced that folder. New installs now drop the marker so the apps use the
  per-user settings folder. Apps that already have settings beside the program
  keep them: they are copied into every update.
- Removing an app no longer deletes settings kept beside the program. They are
  moved to `%LOCALAPPDATA%\ArtCraftSuite\saved-settings` and restored if you
  install the app again.
- Settings left behind in the previous version's folder by an earlier update are
  moved to the same place instead of being deleted on the next update or removal.

## 0.3.3 — 2026-10-08

### Fixed

- All seven application cards now fit on screen without scrolling in a maximized
  window on common displays, including 1920×1080 at 150% scaling, and in the
  restored 1280×720 window.
- On wide windows, cards no longer stretch to fill the leftover height.

### Changed

- Cards flow into two, three or four columns depending on the window width, and
  never get narrower than their buttons need.
- Cards are more compact. The status chip sits beside the app name, replacing the
  duplicate "Available" label, and the SHA-256 chip shares a row with the buttons.
  Its tooltip explains the checksum requirement.
- The restored window size is now 1280×720. The window still opens maximized.

## 0.3.2 — 2026-10-08

### Changed

- The main window now opens maximized so all seven application cards are visible
  immediately when the available desktop work area is tall enough. The existing
  scrollbar remains available on smaller displays and restored windows.

## 0.3.1 — 2026-10-08

### Fixed

- Prevented installed-app action buttons from crowding card titles and version
  details at normal window widths.
- Added bottom scroll clearance so the final application card is not obscured by
  the status and batch-action footer.
- Embedded application artwork directly in the Avalonia resource bundle so icons
  render instead of falling back to solid accent tiles.
- Migrates legacy Rust v0.2 `launchPath` and Unix timestamp install records to the
  v0.3 state format, preventing false **Repair needed** warnings after upgrading.

## 0.3.0 — 2026-10-08

### Added

- Restored the proven C# / Avalonia production application while preserving the
  native Rust v0.2 line on its own branch.
- Added official upstream icons with complete license and provenance records.
- Added an API-independent fallback for stable GitHub releases and automated
  tests for release parsing, package aliases, safe extraction, and installation.
- Added resumable downloads, ETag-backed offline release caching, structured JSON
  logs, and a one-click diagnostic report.
- Added transaction journals and retention of the immediately previous working
  version for crash-safe update recovery.

### Fixed

- Recognizes PrintCraft releases and executables published under the `pdfcraft`
  package name.
- Makes the two-column application card area vertically scrollable.
- Verifies both SHA-256 and expected download length, removes failed downloads,
  and retains transactional directory/state rollback during updates.
- Rejects ZIP path traversal, symbolic links, excessive entry counts, and archives
  that declare more than 16 GiB of extracted data.
- Detects any running executable inside an installation directory instead of
  assuming its process name matches the public application name.
- Validates manifest identifiers and repository/package rules before using them in
  filesystem paths or release requests.

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
