"""Fetch one official FilmCraft AppImage for CI's extraction smoke test."""

import hashlib
import json
import pathlib
import sys
import urllib.request

arch = {"amd64": "x86_64", "arm64": "aarch64"}[sys.argv[1]]
destination = pathlib.Path(sys.argv[2])
headers = {"User-Agent": "ArtCraft-Suite-CI/0.1", "Accept": "application/vnd.github+json"}
request = urllib.request.Request(
    "https://api.github.com/repos/storytold/filmcraft/releases/latest", headers=headers
)
with urllib.request.urlopen(request, timeout=30) as response:
    release = json.load(response)
matches = [
    asset for asset in release["assets"]
    if asset["name"].endswith(f"linux-{arch}.AppImage")
]
if len(matches) != 1:
    raise SystemExit("Expected exactly one FilmCraft AppImage for this architecture")
asset = matches[0]
digest = asset.get("digest", "")
if not digest.startswith("sha256:") or len(digest) != 71:
    raise SystemExit("Upstream release has no usable SHA-256 digest")
download = urllib.request.Request(asset["browser_download_url"], headers=headers)
hasher = hashlib.sha256()
with urllib.request.urlopen(download, timeout=120) as response, destination.open("wb") as output:
    while chunk := response.read(128 * 1024):
        output.write(chunk)
        hasher.update(chunk)
if hasher.hexdigest() != digest[7:]:
    destination.unlink(missing_ok=True)
    raise SystemExit("Upstream AppImage failed SHA-256 verification")
print(f"Verified {asset['name']}")
