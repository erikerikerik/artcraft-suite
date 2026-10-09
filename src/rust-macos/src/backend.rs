use anyhow::{Context, Result, bail, ensure};
use regex::Regex;
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use tempfile::TempDir;

// The Mac edition ships its own app list (12 apps with signed universal DMGs). The shared
// manifest/apps.json stays at seven apps for the Windows edition and its validator.
const MANIFEST: &str = include_str!("../apps-macos.json");
const EXPECTED_TEAM_IDENTIFIER: &str = "DJ6XS33FX8";
const RELEASE_CACHE_TTL: Duration = Duration::from_secs(300);

type CachedReleases = HashMap<String, (Instant, Vec<ApiRelease>)>;
static RELEASE_CACHE: OnceLock<Mutex<CachedReleases>> = OnceLock::new();

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SuiteManifest {
    pub schema_version: u32,
    pub apps: Vec<AppManifest>,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppManifest {
    pub id: String,
    pub name: String,
    pub description: String,
    pub repository: String,
    pub accent: String,
    /// Upstream package name when it differs from `id` (PrintCraft ships as `pdfcraft-*.dmg`
    /// with bundle ID `ai.storyteller.pdfcraft`). Older `id`-named packages are still accepted.
    #[serde(default)]
    pub package_id: Option<String>,
    pub asset_patterns: BTreeMap<String, String>,
}

impl AppManifest {
    /// Every name this app's packages and bundle identifiers may use.
    pub fn package_names(&self) -> Vec<&str> {
        let mut names = vec![self.id.as_str()];
        if let Some(package) = self.package_id.as_deref()
            && package != self.id
        {
            names.push(package);
        }
        names
    }

    fn bundle_id_matches(&self, bundle_id: &str) -> bool {
        let bundle_id = bundle_id.to_ascii_lowercase();
        self.package_names()
            .iter()
            .any(|name| bundle_id == format!("ai.storyteller.{name}"))
    }

    fn asset_regex(&self, template: &str, version: &str) -> Result<Regex> {
        let names = self
            .package_names()
            .iter()
            .map(|name| regex::escape(name))
            .collect::<Vec<_>>()
            .join("|");
        let pattern = template
            .replace("{id}", &format!("(?:{names})"))
            .replace("{version}", &regex::escape(version));
        Ok(Regex::new(&format!("(?i:{pattern})"))?)
    }
}

#[derive(Clone, Debug)]
pub struct Release {
    pub version: String,
    pub asset_name: String,
    pub url: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct InstalledApp {
    pub version: String,
    pub source_asset: String,
    pub sha256: String,
    pub bundle_id: String,
}

#[derive(Default, Serialize, Deserialize)]
pub struct InstallState {
    pub apps: BTreeMap<String, InstalledApp>,
}

#[derive(Clone, Deserialize)]
struct ApiRelease {
    tag_name: String,
    draft: bool,
    prerelease: bool,
    assets: Vec<ApiAsset>,
}

#[derive(Clone, Deserialize)]
struct ApiAsset {
    name: String,
    browser_download_url: String,
    size: u64,
    digest: Option<String>,
}

#[derive(Deserialize)]
struct BundleInfo {
    #[serde(rename = "CFBundleIdentifier")]
    id: String,
    #[serde(rename = "CFBundleExecutable")]
    executable: String,
}

pub fn manifest() -> Result<SuiteManifest> {
    let manifest: SuiteManifest = serde_json::from_str(MANIFEST)?;
    ensure!(manifest.schema_version == 1, "Unsupported manifest schema");
    for app in &manifest.apps {
        ensure!(safe_id(&app.id), "Unsafe app ID in manifest");
        ensure!(
            app.package_id.as_deref().is_none_or(safe_id),
            "Unsafe package ID in manifest"
        );
        let expected_repo = if app.id == "printcraft" {
            "storytold/pdfcraft".to_string()
        } else {
            format!("storytold/{}", app.id)
        };
        ensure!(
            app.repository == expected_repo,
            "Unexpected repository in manifest"
        );
    }
    Ok(manifest)
}

fn safe_id(id: &str) -> bool {
    !id.is_empty() && id.bytes().all(|b| b.is_ascii_lowercase() || b == b'-')
}

fn client() -> Result<Client> {
    Ok(Client::builder()
        .user_agent(concat!(
            "ArtCraft-Suite-Mac/",
            env!("CARGO_PKG_VERSION"),
            " (+https://github.com/erikerikerik/artcraft-suite)"
        ))
        .timeout(std::time::Duration::from_secs(600))
        .build()?)
}

pub fn resolve_with_refresh(
    app: &AppManifest,
    stable: bool,
    force_refresh: bool,
) -> Result<Release> {
    let platform = match std::env::consts::ARCH {
        "x86_64" => "macos-x64",
        "aarch64" => "macos-arm64",
        arch => bail!("Unsupported macOS architecture: {arch}"),
    };
    let template = app
        .asset_patterns
        .get(platform)
        .context("No macOS package rule")?;
    let http = client()?;
    let releases = releases_for(&http, &app.repository, force_refresh)?;
    for release in releases
        .into_iter()
        .filter(|r| !r.draft && (!stable || !r.prerelease))
    {
        let version = release.tag_name.trim_start_matches(['v', 'V']).to_string();
        let pattern = app.asset_regex(template, &version)?;
        if let Some(asset) = release.assets.iter().find(|a| pattern.is_match(&a.name)) {
            let expected_prefix =
                format!("https://github.com/{}/releases/download/", app.repository);
            ensure!(
                asset.browser_download_url.starts_with(&expected_prefix),
                "Unexpected asset URL"
            );
            let sha256 = asset
                .digest
                .as_deref()
                .and_then(parse_digest)
                .or_else(|| {
                    checksum_asset(&http, &release.assets, &asset.name)
                        .ok()
                        .flatten()
                })
                .context("Upstream release has no usable SHA-256 digest")?;
            return Ok(Release {
                version,
                asset_name: asset.name.clone(),
                url: asset.browser_download_url.clone(),
                size: asset.size,
                sha256,
            });
        }
    }
    bail!("No matching macOS DMG in the selected release channel")
}

fn releases_for(http: &Client, repository: &str, force_refresh: bool) -> Result<Vec<ApiRelease>> {
    let cache = RELEASE_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if !force_refresh
        && let Some((fetched, releases)) = cache
            .lock()
            .map_err(|_| anyhow::anyhow!("Release cache is unavailable"))?
            .get(repository)
        && fetched.elapsed() < RELEASE_CACHE_TTL
    {
        return Ok(releases.clone());
    }
    let url = format!("https://api.github.com/repos/{repository}/releases?per_page=20");
    let response = checked_response(http.get(url).send()?, "GitHub releases API")?;
    let releases: Vec<ApiRelease> = response.json()?;
    cache
        .lock()
        .map_err(|_| anyhow::anyhow!("Release cache is unavailable"))?
        .insert(repository.to_string(), (Instant::now(), releases.clone()));
    Ok(releases)
}

fn checked_response(
    response: reqwest::blocking::Response,
    context: &str,
) -> Result<reqwest::blocking::Response> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS
        || (status == reqwest::StatusCode::FORBIDDEN
            && response
                .headers()
                .get("x-ratelimit-remaining")
                .and_then(|v| v.to_str().ok())
                == Some("0"))
    {
        bail!("GitHub's hourly release-check limit was reached. Try again in a few minutes.");
    }
    if status == reqwest::StatusCode::FORBIDDEN {
        bail!("GitHub denied the request for {context}. Try again shortly.");
    }
    bail!("{context} failed with HTTP {status}");
}

pub fn compare_versions(available: &str, installed: &str) -> Option<Ordering> {
    let parse = |value: &str| {
        let normalized = value.strip_prefix(['v', 'V']).unwrap_or(value);
        semver::Version::parse(normalized).ok()
    };
    Some(parse(available)?.cmp(&parse(installed)?))
}

fn parse_digest(value: &str) -> Option<String> {
    let hex = value.strip_prefix("sha256:")?;
    if hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        Some(hex.to_ascii_lowercase())
    } else {
        None
    }
}

fn checksum_asset(http: &Client, assets: &[ApiAsset], target: &str) -> Result<Option<String>> {
    let Some(checksums) = assets
        .iter()
        .find(|a| a.name.eq_ignore_ascii_case("SHA256SUMS.txt"))
    else {
        return Ok(None);
    };
    let text = http
        .get(&checksums.browser_download_url)
        .send()?
        .error_for_status()?
        .text()?;
    for line in text.lines() {
        let mut fields = line.split_whitespace();
        let Some(sha) = fields.next() else { continue };
        let Some(name) = fields.next() else { continue };
        if name.trim_start_matches('*') == target
            && sha.len() == 64
            && sha.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Ok(Some(sha.to_ascii_lowercase()));
        }
    }
    Ok(None)
}

pub fn apps_root() -> Result<PathBuf> {
    let home = std::env::var_os("HOME").context("HOME is unavailable")?;
    Ok(PathBuf::from(home)
        .join("Applications")
        .join("ArtCraft Suite"))
}

fn state_path() -> Result<PathBuf> {
    let home = std::env::var_os("HOME").context("HOME is unavailable")?;
    Ok(PathBuf::from(home).join("Library/Application Support/ArtCraftSuite/state.json"))
}

pub fn bundle_path(app: &AppManifest) -> Result<PathBuf> {
    Ok(apps_root()?.join(format!("{}.app", app.name)))
}

pub fn load_state() -> Result<InstallState> {
    let path = state_path()?;
    if !path.exists() {
        return Ok(InstallState::default());
    }
    let contents = fs::read(&path).with_context(|| format!("Could not read {}", path.display()))?;
    match serde_json::from_slice(&contents) {
        Ok(state) => Ok(state),
        Err(parse_error) => {
            let backup = path.with_file_name(format!(
                "state.json.corrupt-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos()
            ));
            fs::rename(&path, &backup).with_context(|| {
                format!(
                    "Settings are damaged and could not be preserved at {}",
                    backup.display()
                )
            })?;
            let state = rebuild_state()?;
            save_state(&state).with_context(|| {
                format!("Could not save recovered settings after {parse_error}")
            })?;
            Ok(state)
        }
    }
}

fn rebuild_state() -> Result<InstallState> {
    let mut state = InstallState::default();
    for app in manifest()?.apps {
        let bundle = bundle_path(&app)?;
        if !bundle.is_dir() || fs::symlink_metadata(&bundle)?.file_type().is_symlink() {
            continue;
        }
        let info: BundleInfo = match plist::from_file(bundle.join("Contents/Info.plist")) {
            Ok(info) => info,
            Err(_) => continue,
        };
        if !app.bundle_id_matches(&info.id) {
            continue;
        }
        state.apps.insert(
            app.id,
            InstalledApp {
                version: "unknown".into(),
                source_asset: "recovered-local-install".into(),
                sha256: String::new(),
                bundle_id: info.id,
            },
        );
    }
    Ok(state)
}

fn save_state(state: &InstallState) -> Result<()> {
    let path = state_path()?;
    fs::create_dir_all(path.parent().unwrap())?;
    let mut temp = tempfile::NamedTempFile::new_in(path.parent().unwrap())?;
    serde_json::to_writer_pretty(&mut temp, state)?;
    temp.flush()?;
    temp.persist(&path).map_err(|e| e.error)?;
    Ok(())
}

pub fn install(
    app: &AppManifest,
    release: &Release,
    mut progress: impl FnMut(f32, &str),
) -> Result<()> {
    let root = apps_root()?;
    fs::create_dir_all(&root)?;
    let state = load_state()?;
    let target = bundle_path(app)?;
    ensure!(
        !target.exists() || state.apps.contains_key(&app.id),
        "An app already exists at {} but is not managed by ArtCraft Suite",
        target.display()
    );
    ensure_not_running(&target)?;
    let workspace = TempDir::new_in(&root)?;
    let archive = workspace.path().join("release.dmg");
    download(release, &archive, &mut progress)?;
    install_verified_archive(app, release, &archive, workspace.path(), &mut progress)
}

fn install_verified_archive(
    app: &AppManifest,
    release: &Release,
    archive: &Path,
    workspace: &Path,
    progress: &mut impl FnMut(f32, &str),
) -> Result<()> {
    let mut state = load_state()?;
    let target = bundle_path(app)?;
    ensure!(
        !target.exists() || state.apps.contains_key(&app.id),
        "An app already exists at {} but is not managed by ArtCraft Suite",
        target.display()
    );
    progress(1.0, "Opening disk image…");
    let mount = workspace.join("mount");
    fs::create_dir(&mount)?;
    let output = Command::new("hdiutil")
        .args(["attach", "-nobrowse", "-readonly", "-quiet", "-mountpoint"])
        .arg(&mount)
        .arg(archive)
        .output()?;
    ensure!(
        output.status.success(),
        "Could not mount DMG: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let guard = MountGuard(mount.clone());
    let source = discover_bundle(&mount)?;
    let staging = workspace.join("staged.app");
    progress(1.0, "Copying app…");
    let output = Command::new("ditto").arg(&source).arg(&staging).output()?;
    ensure!(
        output.status.success(),
        "Could not copy app bundle: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    progress(1.0, "Checking signature…");
    let bundle_id = validate_bundle(&staging, app)?;
    progress(1.0, "Finishing…");
    guard.detach()?;

    // Keep the previous bundle until the new bundle and state file are committed.
    let backup = workspace.join("previous.app");
    let had_old = target.exists();
    if had_old {
        fs::rename(&target, &backup)?;
    }
    if let Err(error) = fs::rename(&staging, &target) {
        if had_old {
            let _ = fs::rename(&backup, &target);
        }
        return Err(error.into());
    }
    state.apps.insert(
        app.id.clone(),
        InstalledApp {
            version: release.version.clone(),
            source_asset: release.asset_name.clone(),
            sha256: release.sha256.clone(),
            bundle_id,
        },
    );
    if let Err(error) = save_state(&state) {
        let _ = fs::remove_dir_all(&target);
        if had_old {
            let _ = fs::rename(&backup, &target);
        }
        return Err(error);
    }
    Ok(())
}

fn download(release: &Release, path: &Path, progress: &mut impl FnMut(f32, &str)) -> Result<()> {
    let mut response = client()?.get(&release.url).send()?.error_for_status()?;
    let total = response.content_length().unwrap_or(release.size);
    let mut file = File::create(path)?;
    let mut hash = Sha256::new();
    let mut done = 0_u64;
    let mut buf = [0_u8; 128 * 1024];
    loop {
        let n = response.read(&mut buf)?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])?;
        hash.update(&buf[..n]);
        done += n as u64;
        if total > 0 {
            progress((done as f32 / total as f32).clamp(0.0, 1.0), "Downloading…");
        }
    }
    file.flush()?;
    ensure!(
        release.size == 0 || done == release.size,
        "Downloaded size mismatch for {} (expected {} bytes, received {} bytes)",
        release.asset_name,
        release.size,
        done
    );
    let actual = hex::encode(hash.finalize());
    ensure!(
        actual == release.sha256,
        "SHA-256 mismatch for {}",
        release.asset_name
    );
    Ok(())
}

struct MountGuard(PathBuf);
impl MountGuard {
    fn detach(self) -> Result<()> {
        let status = Command::new("hdiutil")
            .args(["detach", "-quiet"])
            .arg(&self.0)
            .status()?;
        ensure!(status.success(), "Could not detach disk image");
        std::mem::forget(self);
        Ok(())
    }
}
impl Drop for MountGuard {
    fn drop(&mut self) {
        let _ = Command::new("hdiutil")
            .args(["detach", "-quiet"])
            .arg(&self.0)
            .status();
    }
}

fn discover_bundle(mount: &Path) -> Result<PathBuf> {
    let mut bundles = Vec::new();
    for entry in fs::read_dir(mount)? {
        let path = entry?.path();
        if path.extension().is_some_and(|x| x == "app") {
            bundles.push(path);
        }
    }
    ensure!(
        bundles.len() == 1,
        "Expected exactly one .app bundle in the DMG; found {}",
        bundles.len()
    );
    Ok(bundles.remove(0))
}

fn validate_bundle(bundle: &Path, app: &AppManifest) -> Result<String> {
    ensure!(
        !fs::symlink_metadata(bundle)?.file_type().is_symlink(),
        "App bundle is a symlink"
    );
    let info: BundleInfo = plist::from_file(bundle.join("Contents/Info.plist"))?;
    ensure!(
        app.bundle_id_matches(&info.id),
        "Unexpected bundle identifier: {}",
        info.id
    );
    ensure!(
        !info.executable.is_empty()
            && info
                .executable
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
        "Unsafe bundle executable name"
    );
    let executable = bundle.join("Contents/MacOS").join(&info.executable);
    ensure!(executable.is_file(), "App executable is missing");
    let cleanup = Command::new("xattr")
        .args(["-rd", "com.apple.FinderInfo"])
        .arg(bundle)
        .output()?;
    if !cleanup.status.success()
        && !String::from_utf8_lossy(&cleanup.stderr).contains("No such xattr")
    {
        bail!(
            "Could not normalize app bundle metadata: {}",
            String::from_utf8_lossy(&cleanup.stderr).trim()
        );
    }
    let signature = Command::new("codesign")
        .args(["--verify", "--deep", "--strict"])
        .arg(bundle)
        .output()?;
    ensure!(
        signature.status.success(),
        "App signature is invalid: {}",
        String::from_utf8_lossy(&signature.stderr).trim()
    );
    let details = Command::new("codesign")
        .args(["-dv", "--verbose=4"])
        .arg(bundle)
        .output()?;
    ensure!(
        details.status.success(),
        "Could not read app signing identity"
    );
    let details = format!(
        "{}{}",
        String::from_utf8_lossy(&details.stdout),
        String::from_utf8_lossy(&details.stderr)
    );
    let team = details
        .lines()
        .find_map(|line| line.strip_prefix("TeamIdentifier="));
    ensure!(
        team == Some(EXPECTED_TEAM_IDENTIFIER),
        "Unexpected app signing team: {}",
        team.unwrap_or("missing")
    );
    let assessment = Command::new("spctl")
        .args(["--assess", "--type", "execute"])
        .arg(bundle)
        .output()?;
    ensure!(
        assessment.status.success(),
        "Gatekeeper rejected the app: {}",
        String::from_utf8_lossy(&assessment.stderr).trim()
    );
    let output = Command::new("lipo")
        .arg("-archs")
        .arg(&executable)
        .output()?;
    ensure!(
        output.status.success(),
        "Could not inspect app executable architecture"
    );
    let architectures = String::from_utf8_lossy(&output.stdout);
    let expected = match std::env::consts::ARCH {
        "x86_64" => "x86_64",
        "aarch64" => "arm64",
        arch => bail!("Unsupported macOS architecture: {arch}"),
    };
    ensure!(
        architectures
            .split_whitespace()
            .any(|arch| arch == expected),
        "App does not contain a {expected} executable slice"
    );
    Ok(info.id)
}

fn ensure_not_running(bundle: &Path) -> Result<()> {
    if !bundle.exists() {
        return Ok(());
    }
    let info: BundleInfo = plist::from_file(bundle.join("Contents/Info.plist"))?;
    let executable = bundle.join("Contents/MacOS").join(info.executable);
    let output = Command::new("lsof")
        .args(["-n", "-t"])
        .arg(&executable)
        .output()?;
    if output.status.success() && !output.stdout.is_empty() {
        bail!(
            "{} is open. Quit it before updating or removing it.",
            bundle
                .file_stem()
                .and_then(|v| v.to_str())
                .unwrap_or("The app")
        );
    }
    ensure!(
        output.status.code() == Some(1),
        "Could not check whether the app is open: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(())
}

pub fn remove(app: &AppManifest) -> Result<()> {
    let mut state = load_state()?;
    ensure!(
        state.apps.contains_key(&app.id),
        "App is not managed by ArtCraft Suite"
    );
    let target = bundle_path(app)?;
    ensure_not_running(&target)?;
    // Rename into a private holding area, commit state, then delete the old bundle.
    let root = apps_root()?;
    fs::create_dir_all(&root)?;
    let holding = TempDir::new_in(&root)?;
    let moved = holding.path().join("removed.app");
    if target.exists() {
        fs::rename(&target, &moved)?;
    }
    state.apps.remove(&app.id);
    if let Err(error) = save_state(&state) {
        if moved.exists() {
            let _ = fs::rename(&moved, &target);
        }
        return Err(error);
    }
    Ok(())
}

pub fn launch(app: &AppManifest) -> Result<()> {
    let state = load_state()?;
    ensure!(state.apps.contains_key(&app.id), "App is not installed");
    let path = bundle_path(app)?;
    ensure!(path.is_dir(), "App bundle is missing");
    let status = Command::new("open").arg(&path).status()?;
    ensure!(status.success(), "macOS could not open the app");
    Ok(())
}

/// Shows the installed app selected in a Finder window.
pub fn reveal(app: &AppManifest) -> Result<()> {
    let path = bundle_path(app)?;
    ensure!(path.is_dir(), "App bundle is missing");
    let status = Command::new("open").arg("-R").arg(&path).status()?;
    ensure!(status.success(), "Finder could not show the app");
    Ok(())
}

/// Opens ~/Applications/ArtCraft Suite in Finder, creating it first if needed.
pub fn open_apps_folder() -> Result<()> {
    let root = apps_root()?;
    fs::create_dir_all(&root)?;
    let status = Command::new("open").arg(&root).status()?;
    ensure!(status.success(), "Finder could not open the apps folder");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_is_safe_and_complete() {
        let data = manifest().unwrap();
        assert_eq!(data.apps.len(), 12);
        let mut ids: Vec<_> = data.apps.iter().map(|app| app.id.as_str()).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), 12, "app IDs must be unique");
        for app in data.apps {
            assert!(app.asset_patterns.contains_key("macos-arm64"));
            assert!(app.asset_patterns.contains_key("macos-x64"));
        }
    }

    fn app(id: &str) -> AppManifest {
        manifest()
            .unwrap()
            .apps
            .into_iter()
            .find(|app| app.id == id)
            .unwrap()
    }

    #[test]
    fn printcraft_matches_renamed_pdfcraft_packages() {
        let printcraft = app("printcraft");
        let template = &printcraft.asset_patterns["macos-arm64"];
        let current = printcraft.asset_regex(template, "0.4.0").unwrap();
        assert!(current.is_match("pdfcraft-0.4.0-macos-universal.dmg"));
        assert!(current.is_match("printcraft-0.4.0-macos-universal.dmg"));
        assert!(!current.is_match("pdfcraft-0.4.0-windows-x64-portable.zip"));
        assert!(!current.is_match("xpdfcraft-0.4.0-macos-universal.dmg"));
        assert!(!current.is_match("pdfcraft-0.4.0-macos-universal.dmg.sig"));
        assert!(printcraft.bundle_id_matches("ai.storyteller.pdfcraft"));
        assert!(printcraft.bundle_id_matches("ai.storyteller.printcraft"));
        assert!(!printcraft.bundle_id_matches("ai.storyteller.wordcraft"));
        assert!(!printcraft.bundle_id_matches("ai.fake.pdfcraft"));
        assert!(!printcraft.bundle_id_matches("ai.storyteller.pdfcraft.helper"));
    }

    #[test]
    fn new_apps_match_their_own_packages_only() {
        for id in [
            "wordcraft",
            "gridcraft",
            "deckcraft",
            "soundcraft",
            "cadcraft",
        ] {
            let item = app(id);
            assert_eq!(item.repository, format!("storytold/{id}"));
            let pattern = item
                .asset_regex(&item.asset_patterns["macos-x64"], "0.3.0")
                .unwrap();
            assert!(pattern.is_match(&format!("{id}-0.3.0-macos-universal.dmg")));
            assert!(!pattern.is_match(&format!("{id}-0.3.1-macos-universal.dmg")));
            assert!(!pattern.is_match("photocraft-0.3.0-macos-universal.dmg"));
            assert!(item.bundle_id_matches(&format!("ai.storyteller.{id}")));
        }
    }

    #[test]
    fn digest_requires_full_sha256() {
        assert!(parse_digest("sha256:1234").is_none());
        assert!(parse_digest(&format!("sha256:{}", "a".repeat(64))).is_some());
    }

    #[test]
    fn available_versions_are_compared_semantically() {
        assert_eq!(compare_versions("1.10.0", "1.9.0"), Some(Ordering::Greater));
        assert_eq!(
            compare_versions("1.0.0-beta.2", "1.0.0"),
            Some(Ordering::Less)
        );
        assert_eq!(compare_versions("v1.2.3", "1.2.3"), Some(Ordering::Equal));
    }

    #[test]
    fn damaged_state_is_preserved_and_rebuilt() {
        let home = TempDir::new().unwrap();
        let state = home
            .path()
            .join("Library/Application Support/ArtCraftSuite/state.json");
        fs::create_dir_all(state.parent().unwrap()).unwrap();
        fs::write(&state, b"{broken json").unwrap();
        let previous_home = std::env::var_os("HOME");
        unsafe {
            std::env::set_var("HOME", home.path());
        }
        let result = load_state().unwrap();
        if let Some(previous_home) = previous_home {
            unsafe {
                std::env::set_var("HOME", previous_home);
            }
        } else {
            unsafe {
                std::env::remove_var("HOME");
            }
        }
        assert!(result.apps.is_empty());
        assert!(state.exists());
        assert!(fs::read_dir(state.parent().unwrap()).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("state.json.corrupt-")
        }));
    }

    #[test]
    #[ignore = "requires ARTCRAFT_TEST_DMG with a verified FilmCraft release DMG"]
    fn installs_and_removes_real_dmg_in_isolated_home() {
        let archive = PathBuf::from(std::env::var("ARTCRAFT_TEST_DMG").unwrap());
        let home = TempDir::new().unwrap();
        // Run this ignored test alone: HOME is process-wide.
        unsafe {
            std::env::set_var("HOME", home.path());
        }
        let app = manifest()
            .unwrap()
            .apps
            .into_iter()
            .find(|a| a.id == "filmcraft")
            .unwrap();
        let release = Release {
            version: "0.2.1".into(),
            asset_name: "filmcraft-0.2.1-macos-universal.dmg".into(),
            url: String::new(),
            size: 0,
            sha256: "4deff5924e8e4040e60c73db2e668812a5967c973790be7565f8e8d9c69344fd".into(),
        };
        let root = apps_root().unwrap();
        fs::create_dir_all(&root).unwrap();
        let workspace = TempDir::new_in(&root).unwrap();
        install_verified_archive(&app, &release, &archive, workspace.path(), &mut |_, _| {})
            .unwrap();
        assert!(
            bundle_path(&app)
                .unwrap()
                .join("Contents/MacOS/FilmCraft")
                .is_file()
        );
        assert_eq!(
            load_state().unwrap().apps.get(&app.id).unwrap().version,
            "0.2.1"
        );
        remove(&app).unwrap();
        assert!(!bundle_path(&app).unwrap().exists());
        assert!(load_state().unwrap().apps.is_empty());
    }
}
