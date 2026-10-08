use anyhow::{Context, Result, bail, ensure};
use regex::Regex;
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use tempfile::{NamedTempFile, TempDir};

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

pub fn manifest() -> Result<SuiteManifest> {
    let manifest: SuiteManifest = serde_json::from_str(MANIFEST)?;
    ensure!(manifest.schema_version == 1, "Unsupported manifest schema");
    ensure!(manifest.apps.len() == 7, "Expected seven ArtCraft apps");
    for app in &manifest.apps {
        ensure!(safe_id(&app.id), "Unsafe app ID in manifest");
        ensure!(
            !app.name.contains(['\n', '\r']),
            "Unsafe app name in manifest"
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

fn platform_key() -> Result<&'static str> {
    ensure!(cfg!(target_os = "linux"), "This installer runs on Linux");
    match std::env::consts::ARCH {
        "x86_64" => Ok("linux-x64"),
        "aarch64" => Ok("linux-arm64"),
        other => bail!("Unsupported Linux architecture: {other}"),
    }
}

fn client() -> Result<Client> {
    Ok(Client::builder()
        .user_agent("ArtCraft-Suite-Linux/0.1 (+https://github.com/erikerikerik/artcraft-suite)")
        .timeout(std::time::Duration::from_secs(600))
        .build()?)
}

pub fn resolve(app: &AppManifest, stable: bool) -> Result<Release> {
    let platform = platform_key()?;
    let template = app
        .asset_patterns
        .get(platform)
        .context("No package rule for this architecture")?;
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
    bail!("No matching AppImage in the selected release channel")
}

fn parse_digest(value: &str) -> Option<String> {
    let hex = value.strip_prefix("sha256:")?;
    (hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit()))
        .then(|| hex.to_ascii_lowercase())
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
        let (Some(sha), Some(name)) = (fields.next(), fields.next()) else {
            continue;
        };
        if name.trim_start_matches('*') == target
            && sha.len() == 64
            && sha.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Ok(Some(sha.to_ascii_lowercase()));
        }
    }
    Ok(None)
}

fn data_home() -> Result<PathBuf> {
    if let Some(xdg) = std::env::var_os("XDG_DATA_HOME") {
        let path = PathBuf::from(xdg);
        if path.is_absolute() {
            return Ok(path);
        }
    }
    let home = std::env::var_os("HOME").context("HOME is unavailable")?;
    Ok(PathBuf::from(home).join(".local/share"))
}

pub fn apps_root() -> Result<PathBuf> {
    Ok(data_home()?.join("artcraft-suite/apps"))
}
fn state_path() -> Result<PathBuf> {
    Ok(data_home()?.join("artcraft-suite/state.json"))
}
fn desktop_path(app: &AppManifest) -> Result<PathBuf> {
    Ok(data_home()?
        .join("applications")
        .join(format!("artcraft-suite-{}.desktop", app.id)))
}
fn installed_path(app: &AppManifest) -> Result<PathBuf> {
    Ok(apps_root()?.join(&app.id))
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
    let mut temp = NamedTempFile::new_in(path.parent().unwrap())?;
    serde_json::to_writer_pretty(&mut temp, state)?;
    temp.flush()?;
    temp.persist(&path).map_err(|e| e.error)?;
    Ok(())
}

fn write_desktop(app: &AppManifest) -> Result<()> {
    let path = desktop_path(app)?;
    fs::create_dir_all(path.parent().unwrap())?;
    let mut temp = NamedTempFile::new_in(path.parent().unwrap())?;
    write!(
        temp,
        "[Desktop Entry]\nType=Application\nName={}\nComment={}\nExec=/usr/bin/artcraft-suite --launch {}\nIcon=applications-graphics\nCategories=Graphics;\nTerminal=false\n",
        app.name,
        app.description.replace(['\n', '\r'], " "),
        app.id
    )?;
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
    let workspace = TempDir::new_in(&root)?;
    let archive = workspace.path().join("release.AppImage");
    download(release, &archive, &mut progress)?;
    install_verified_archive(app, release, &archive, workspace.path(), &mut progress)
}

fn download(release: &Release, path: &Path, progress: &mut impl FnMut(f32, &str)) -> Result<()> {
    let mut response = client()?.get(&release.url).send()?.error_for_status()?;
    let total = response.content_length().unwrap_or(release.size);
    let mut file = File::create(path)?;
    let mut hash = Sha256::new();
    let mut done = 0_u64;
    let mut buffer = [0_u8; 128 * 1024];
    loop {
        let count = response.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        file.write_all(&buffer[..count])?;
        hash.update(&buffer[..count]);
        done += count as u64;
        if total > 0 {
            progress(
                (done as f32 / total as f32).clamp(0.0, 1.0),
                "Downloading and verifying…",
            );
        }
    }
    file.flush()?;
    ensure!(
        hex::encode(hash.finalize()) == release.sha256,
        "SHA-256 mismatch for {}",
        release.asset_name
    );
    Ok(())
}

fn install_verified_archive(
    app: &AppManifest,
    release: &Release,
    archive: &Path,
    workspace: &Path,
    progress: &mut impl FnMut(f32, &str),
) -> Result<()> {
    let mut state = load_state()?;
    let target = installed_path(app)?;
    let desktop = desktop_path(app)?;
    ensure!(
        !target.exists() || state.apps.contains_key(&app.id),
        "An unmanaged app already exists at {}",
        target.display()
    );
    ensure!(
        !desktop.exists() || state.apps.contains_key(&app.id),
        "An unmanaged launcher already exists at {}",
        desktop.display()
    );
    let mut perms = fs::metadata(archive)?.permissions();
    perms.set_mode(perms.mode() | 0o700);
    fs::set_permissions(archive, perms)?;
    progress(1.0, "Extracting verified AppImage…");
    let output = Command::new(archive)
        .arg("--appimage-extract")
        .current_dir(workspace)
        .stdout(Stdio::null())
        .output()?;
    ensure!(
        output.status.success(),
        "Could not extract AppImage: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let staging = workspace.join("squashfs-root");
    validate_install(&staging)?;

    let backup_app = workspace.join("previous-app");
    let backup_desktop = workspace.join("previous.desktop");
    let had_app = target.exists();
    let had_desktop = desktop.exists();
    if had_app {
        fs::rename(&target, &backup_app)?;
    }
    if had_desktop {
        if let Err(error) = fs::rename(&desktop, &backup_desktop) {
            if had_app {
                let _ = fs::rename(&backup_app, &target);
            }
            return Err(error.into());
        }
    }
    let result = (|| -> Result<()> {
        fs::rename(&staging, &target)?;
        write_desktop(app)?;
        state.apps.insert(
            app.id.clone(),
            InstalledApp {
                version: release.version.clone(),
                source_asset: release.asset_name.clone(),
                sha256: release.sha256.clone(),
            },
        );
        save_state(&state)
    })();
    if let Err(error) = result {
        if target.exists() {
            let _ = fs::remove_dir_all(&target);
        }
        if had_app {
            let _ = fs::rename(&backup_app, &target);
        }
        if desktop.exists() {
            let _ = fs::remove_file(&desktop);
        }
        if had_desktop {
            let _ = fs::rename(&backup_desktop, &desktop);
        }
        return Err(error);
    }
    Ok(())
}

fn validate_install(root: &Path) -> Result<()> {
    ensure!(
        root.is_dir() && !fs::symlink_metadata(root)?.file_type().is_symlink(),
        "No extracted app directory"
    );
    let entrypoint = root.join("AppRun");
    let resolved = fs::canonicalize(&entrypoint).context("AppImage has no AppRun")?;
    ensure!(
        resolved.starts_with(fs::canonicalize(root)?),
        "AppRun escapes the app directory"
    );
    ensure!(resolved.is_file(), "AppRun is not a file");
    ensure!(
        fs::metadata(&resolved)?.permissions().mode() & 0o111 != 0,
        "AppRun is not executable"
    );
    Ok(())
}

pub fn remove(app: &AppManifest) -> Result<()> {
    let mut state = load_state()?;
    ensure!(
        state.apps.contains_key(&app.id),
        "App is not managed by ArtCraft Suite"
    );
    let target = installed_path(app)?;
    let desktop = desktop_path(app)?;
    let root = apps_root()?;
    fs::create_dir_all(&root)?;
    let holding = TempDir::new_in(&root)?;
    let moved_app = holding.path().join("removed-app");
    let moved_desktop = holding.path().join("removed.desktop");
    if target.exists() {
        fs::rename(&target, &moved_app)?;
    }
    if desktop.exists() {
        if let Err(error) = fs::rename(&desktop, &moved_desktop) {
            if moved_app.exists() {
                let _ = fs::rename(&moved_app, &target);
            }
            return Err(error.into());
        }
    }
    state.apps.remove(&app.id);
    if let Err(error) = save_state(&state) {
        if moved_app.exists() {
            let _ = fs::rename(&moved_app, &target);
        }
        if moved_desktop.exists() {
            let _ = fs::rename(&moved_desktop, &desktop);
        }
        return Err(error);
    }
    Ok(())
}

pub fn launch(app: &AppManifest) -> Result<()> {
    ensure!(
        load_state()?.apps.contains_key(&app.id),
        "App is not installed"
    );
    let target = installed_path(app)?;
    validate_install(&target)?;
    Command::new(target.join("AppRun"))
        .current_dir(&target)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
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
            assert!(app.asset_patterns.contains_key("linux-x64"));
            assert!(app.asset_patterns.contains_key("linux-arm64"));
        }
    }

    #[test]
    fn digest_requires_full_sha256() {
        assert!(parse_digest("sha256:1234").is_none());
        assert!(parse_digest(&format!("sha256:{}", "a".repeat(64))).is_some());
    }

    #[test]
    fn install_and_remove_synthetic_appimage() {
        let home = TempDir::new().unwrap();
        unsafe {
            std::env::set_var("XDG_DATA_HOME", home.path().join("data"));
        }
        let app = manifest()
            .unwrap()
            .apps
            .into_iter()
            .find(|a| a.id == "filmcraft")
            .unwrap();
        let root = apps_root().unwrap();
        fs::create_dir_all(&root).unwrap();
        let workspace = TempDir::new_in(&root).unwrap();
        let archive = workspace.path().join("test.AppImage");
        fs::write(&archive, "#!/bin/sh\nmkdir squashfs-root\nprintf '#!/bin/sh\\nexit 0\\n' > squashfs-root/AppRun\nchmod +x squashfs-root/AppRun\n").unwrap();
        let release = Release {
            version: "0.2.1".into(),
            asset_name: "test.AppImage".into(),
            url: String::new(),
            size: 0,
            sha256: "a".repeat(64),
        };
        install_verified_archive(&app, &release, &archive, workspace.path(), &mut |_, _| {})
            .unwrap();
        assert!(installed_path(&app).unwrap().join("AppRun").is_file());
        assert!(desktop_path(&app).unwrap().is_file());
        assert_eq!(
            load_state().unwrap().apps.get(&app.id).unwrap().version,
            "0.2.1"
        );
        remove(&app).unwrap();
        assert!(!installed_path(&app).unwrap().exists());
        assert!(!desktop_path(&app).unwrap().exists());
    }

    #[test]
    #[ignore = "requires ARTCRAFT_TEST_APPIMAGE containing a verified FilmCraft AppImage"]
    fn installs_real_upstream_appimage() {
        let archive = PathBuf::from(std::env::var("ARTCRAFT_TEST_APPIMAGE").unwrap());
        let home = TempDir::new().unwrap();
        // Run this ignored test alone: XDG_DATA_HOME is process-wide.
        unsafe {
            std::env::set_var("XDG_DATA_HOME", home.path().join("data"));
        }
        let app = manifest()
            .unwrap()
            .apps
            .into_iter()
            .find(|a| a.id == "filmcraft")
            .unwrap();
        let root = apps_root().unwrap();
        fs::create_dir_all(&root).unwrap();
        let workspace = TempDir::new_in(&root).unwrap();
        let release = Release {
            version: "smoke".into(),
            asset_name: "FilmCraft.AppImage".into(),
            url: String::new(),
            size: 0,
            sha256: "a".repeat(64),
        };
        install_verified_archive(&app, &release, &archive, workspace.path(), &mut |_, _| {})
            .unwrap();
        validate_install(&installed_path(&app).unwrap()).unwrap();
        remove(&app).unwrap();
    }
}
