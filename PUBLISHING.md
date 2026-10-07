# Publishing to GitHub

No generated binaries or upstream applications need to be committed. Start from
the source archive, then publish the repository and let GitHub Actions build the
artifacts.

## GitHub CLI

Replace `YOURNAME` and choose `--public` or `--private`:

```powershell
Expand-Archive .\ArtCraftSuite-source.zip .\artcraft-suite
Set-Location .\artcraft-suite
git init
git add .
git commit -m "Initial ArtCraft Suite Manager"
git branch -M main
gh auth login
gh repo create YOURNAME/artcraft-suite --public --source . --remote origin --push
```

The CI workflow will build Windows x64 and the unsigned macOS ARM scaffold.

## GitHub website plus Git

1. Create an empty repository named `artcraft-suite` on GitHub. Do not add a
   README, license, or `.gitignore` there because this project already has them.
2. Run:

```powershell
Expand-Archive .\ArtCraftSuite-source.zip .\artcraft-suite
Set-Location .\artcraft-suite
git init
git add .
git commit -m "Initial ArtCraft Suite Manager"
git branch -M main
git remote add origin https://github.com/YOURNAME/artcraft-suite.git
git push -u origin main
```

## Create the first release

After the `CI` workflow is green:

```powershell
git tag -a v0.1.0 -m "ArtCraft Suite Manager 0.1.0"
git push origin v0.1.0
```

The `Release` workflow builds, checksums, and attaches the Windows x64 package
and the clearly labeled unsigned macOS ARM scaffold to a GitHub Release. Review
the generated release notes before announcing it.
