use anyhow::{Context, Result, bail, ensure};
use regex::Regex;
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::TempDir;

const MANIFEST: &str = include_str!("../../../manifest/apps.json");

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
    pub asset_patterns: BTreeMap<String, String>,
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

#[derive(Deserialize)]
struct ApiRelease {
    tag_name: String,
    draft: bool,
    prerelease: bool,
    assets: Vec<ApiAsset>,
}

#[derive(Deserialize)]
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
        .user_agent("ArtCraft-Suite-Rust/0.1 (+https://github.com/erikerikerik/artcraft-suite)")
        .timeout(std::time::Duration::from_secs(600))
        .build()?)
}

pub fn resolve(app: &AppManifest, stable: bool) -> Result<Release> {
    let template = app
        .asset_patterns
        .get("macos-arm64")
        .context("No macOS package rule")?;
    let url = format!(
        "https://api.github.com/repos/{}/releases?per_page=20",
        app.repository
    );
    let http = client()?;
    let releases: Vec<ApiRelease> = http.get(url).send()?.error_for_status()?.json()?;
    for release in releases
        .into_iter()
        .filter(|r| !r.draft && (!stable || !r.prerelease))
    {
        let version = release.tag_name.trim_start_matches(['v', 'V']).to_string();
        let pattern = template
            .replace("{id}", &regex::escape(&app.id))
            .replace("{version}", &regex::escape(&version));
        let pattern = Regex::new(&format!("(?i:{pattern})"))?;
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
    Ok(serde_json::from_reader(File::open(&path).with_context(
        || format!("Could not read {}", path.display()),
    )?)?)
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
    progress(1.0, "Mounting verified disk image…");
    let mount = workspace.join("mount");
    fs::create_dir(&mount)?;
    let output = Command::new("hdiutil")
        .args(["attach", "-nobrowse", "-readonly", "-quiet", "-mountpoint"])
        .arg(&mount)
        .arg(&archive)
        .output()?;
    ensure!(
        output.status.success(),
        "Could not mount DMG: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let guard = MountGuard(mount.clone());
    let source = discover_bundle(&mount, app)?;
    let staging = workspace.join("staged.app");
    progress(1.0, "Copying app bundle…");
    let output = Command::new("ditto").arg(&source).arg(&staging).output()?;
    ensure!(
        output.status.success(),
        "Could not copy app bundle: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let bundle_id = validate_bundle(&staging, app)?;
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
            progress(
                (done as f32 / total as f32).clamp(0.0, 1.0),
                "Downloading and verifying…",
            );
        }
    }
    file.flush()?;
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

fn discover_bundle(mount: &Path, app: &AppManifest) -> Result<PathBuf> {
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
    validate_bundle(&bundles[0], app)?;
    Ok(bundles.remove(0))
}

fn validate_bundle(bundle: &Path, app: &AppManifest) -> Result<String> {
    ensure!(
        !fs::symlink_metadata(bundle)?.file_type().is_symlink(),
        "App bundle is a symlink"
    );
    let info: BundleInfo = plist::from_file(bundle.join("Contents/Info.plist"))?;
    ensure!(
        info.id.to_ascii_lowercase().contains(&app.id),
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
    ensure!(
        bundle
            .join("Contents/MacOS")
            .join(&info.executable)
            .is_file(),
        "App executable is missing"
    );
    Ok(info.id)
}

pub fn remove(app: &AppManifest) -> Result<()> {
    let mut state = load_state()?;
    ensure!(
        state.apps.contains_key(&app.id),
        "App is not managed by ArtCraft Suite"
    );
    let target = bundle_path(app)?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_is_safe_and_complete() {
        let data = manifest().unwrap();
        assert_eq!(data.apps.len(), 7);
        for app in data.apps {
            assert!(app.asset_patterns.contains_key("macos-arm64"));
        }
    }

    #[test]
    fn digest_requires_full_sha256() {
        assert!(parse_digest("sha256:1234").is_none());
        assert!(parse_digest(&format!("sha256:{}", "a".repeat(64))).is_some());
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
