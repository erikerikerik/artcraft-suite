# Publishing a release

Releases are built by GitHub Actions. No binaries or upstream applications are
committed to the repository.

## Before tagging

1. Update `<Version>` in `src/ArtCraftSuite/ArtCraftSuite.csproj` and the two
   version strings in `scripts/package-macos.sh`.
2. Add an entry to [`CHANGELOG.md`](CHANGELOG.md).
3. Push to `main` and wait for the `CI` workflow to go green.

## Tag the release

Use semantic versioning: bump the patch number for fixes (`v0.1.0` → `v0.1.1`),
the minor number for new features (`v0.2.0`). A tag containing a hyphen, such as
`v0.2.0-rc.1`, is published as a prerelease.

From a clone:

```powershell
git tag -a v0.1.1 -m "ArtCraft Suite Manager 0.1.1"
git push origin v0.1.1
```

Or on GitHub: **Releases → Draft a new release**, type the new tag (for example
`v0.1.1`), choose **Create new tag on publish** targeting `main`, and publish.

Either way, the `Release` workflow builds the Windows x64 package and the clearly
labeled unsigned macOS ARM scaffold, writes SHA-256 files for both, and attaches
them to the GitHub Release. Review the release notes before announcing it.
