use crate::model::{
    ApiAsset, ApiRelease, AppManifest, InstallState, InstalledApp, ResolvedRelease,
};
use regex::Regex;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

const USER_AGENT: &str = "ArtCraft-Suite/0.2 (+https://github.com/erikerikerik/artcraft-suite)";

pub fn platform_key() -> &'static str {
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    return "windows-x64";
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    return "macos-arm64";
    #[allow(unreachable_code)]
    "unsupported"
}

pub fn resolve_release(
    app: &AppManifest,
    channel: &str,
    platform: &str,
) -> Result<ResolvedRelease, String> {
    if platform == "unsupported" {
        return Err("This platform is not supported yet".into());
    }
    let url = format!(
        "https://api.github.com/repos/{}/releases?per_page=20",
        app.repository
    );
    let releases: Vec<ApiRelease> = serde_json::from_slice(&curl_bytes(&url)?)
        .map_err(|error| format!("GitHub returned invalid release data: {error}"))?;

    let release = releases
        .into_iter()
        .find(|release| !release.draft && (channel != "Stable" || !release.prerelease))
        .ok_or_else(|| format!("{} has no {} release", app.name, channel.to_lowercase()))?;
    let version = release.tag_name.trim_start_matches(['v', 'V']).to_owned();
    let template = app
        .asset_patterns
        .get(platform)
        .ok_or_else(|| format!("{} has no package rule for {platform}", app.name))?;
    let pattern = template
        .replace("{id}", &regex::escape(app.package_id()))
        .replace("{version}", &regex::escape(&version));
    let matcher = Regex::new(&pattern).map_err(|error| format!("Invalid package rule: {error}"))?;
    let asset = release
        .assets
        .iter()
        .find(|asset| matcher.is_match(&asset.name))
        .cloned()
        .ok_or_else(|| format!("No matching {platform} package was found for {}", app.name))?;

    let sha256 = asset
        .digest
        .as_deref()
        .and_then(parse_digest)
        .map(str::to_owned)
        .or_else(|| {
            read_checksum_asset(&release.assets, &asset.name)
                .ok()
                .flatten()
        })
        .ok_or_else(|| format!("{} does not publish a SHA-256 for {}", app.name, asset.name))?;

    Ok(ResolvedRelease {
        version,
        asset,
        sha256,
    })
}

pub fn download_verified(
    release: &ResolvedRelease,
    destination: &Path,
    mut progress: impl FnMut(f32),
) -> Result<(), String> {
    progress(0.05);
    let status = Command::new(curl_program())
        .args([
            "-fL",
            "--retry",
            "3",
            "--connect-timeout",
            "20",
            "--user-agent",
            USER_AGENT,
            "--output",
        ])
        .arg(destination)
        .arg(&release.asset.browser_download_url)
        .status()
        .map_err(|error| format!("Could not start download: {error}"))?;
    if !status.success() {
        let _ = fs::remove_file(destination);
        return Err(format!("Download failed with status {status}"));
    }
    let downloaded_size = fs::metadata(destination)
        .map_err(|error| format!("Could not inspect download: {error}"))?
        .len();
    if release.asset.size > 0 && downloaded_size != release.asset.size {
        let _ = fs::remove_file(destination);
        return Err(format!(
            "Downloaded size did not match GitHub metadata (expected {}, got {})",
            release.asset.size, downloaded_size
        ));
    }
    progress(0.9);
    let actual = sha256_file(destination)?;
    if !actual.eq_ignore_ascii_case(&release.sha256) {
        let _ = fs::remove_file(destination);
        return Err(format!(
            "SHA-256 verification failed for {}",
            release.asset.name
        ));
    }
    progress(1.0);
    Ok(())
}

pub fn load_state() -> InstallState {
    fs::read_to_string(state_path())
        .ok()
        .and_then(|json| serde_json::from_str(&json).ok())
        .unwrap_or_default()
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
pub fn install(
    app: &AppManifest,
    release: &ResolvedRelease,
    package: &Path,
) -> Result<InstalledApp, String> {
    #[cfg(target_os = "windows")]
    let launch_path = install_zip(app, package)?;
    #[cfg(target_os = "macos")]
    let launch_path = install_dmg(app, package)?;

    let installed = InstalledApp {
        id: app.id.clone(),
        version: release.version.clone(),
        launch_path: launch_path.to_string_lossy().into_owned(),
        source_asset: release.asset.name.clone(),
        sha256: release.sha256.clone(),
        installed_at_unix: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
    };
    let mut state = load_state();
    state.apps.insert(app.id.clone(), installed.clone());
    save_state(&state)?;
    Ok(installed)
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub fn install(
    _app: &AppManifest,
    _release: &ResolvedRelease,
    _package: &Path,
) -> Result<InstalledApp, String> {
    Err("Installation is not supported on this platform".into())
}

pub fn uninstall(id: &str) -> Result<(), String> {
    let mut state = load_state();
    if state.apps.remove(id).is_none() {
        return Ok(());
    }
    let root = apps_root();
    let target = root.join(id);
    let root = normalized_with_separator(&root)?;
    let target_full = fs::canonicalize(&target).unwrap_or(target.clone());
    if !target_full.to_string_lossy().starts_with(&root) {
        return Err("Refusing to remove a path outside the manager-owned app directory".into());
    }
    if target.exists() {
        fs::remove_dir_all(&target).map_err(|error| format!("Could not remove app: {error}"))?;
    }
    save_state(&state)
}

pub fn launch(installed: &InstalledApp) -> Result<(), String> {
    let path = Path::new(&installed.launch_path);
    if !path.exists() {
        return Err(
            "The installed application is missing; reinstall it to repair the entry".into(),
        );
    }
    #[cfg(target_os = "macos")]
    {
        Command::new("open")
            .arg(path)
            .spawn()
            .map_err(|error| format!("Could not open app: {error}"))?;
    }
    #[cfg(not(target_os = "macos"))]
    {
        open::that(path).map_err(|error| format!("Could not open app: {error}"))?;
    }
    Ok(())
}

pub fn is_installed(installed: Option<&InstalledApp>) -> bool {
    installed
        .map(|entry| Path::new(&entry.launch_path).exists())
        .unwrap_or(false)
}

fn parse_digest(digest: &str) -> Option<&str> {
    let value = digest
        .strip_prefix("sha256:")
        .or_else(|| digest.strip_prefix("SHA256:"))?;
    (value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())).then_some(value)
}

fn read_checksum_asset(assets: &[ApiAsset], file_name: &str) -> Result<Option<String>, String> {
    let Some(checksums) = assets
        .iter()
        .find(|asset| asset.name.eq_ignore_ascii_case("SHA256SUMS.txt"))
    else {
        return Ok(None);
    };
    let text = String::from_utf8(curl_bytes(&checksums.browser_download_url)?)
        .map_err(|error| format!("Checksum file was not UTF-8: {error}"))?;
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        let Some(hash) = parts.next() else { continue };
        let Some(name) = parts.next() else { continue };
        if name.trim_start_matches('*').eq_ignore_ascii_case(file_name)
            && hash.len() == 64
            && hash.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Ok(Some(hash.to_ascii_lowercase()));
        }
    }
    Ok(None)
}

#[cfg(target_os = "windows")]
fn install_zip(app: &AppManifest, package: &Path) -> Result<PathBuf, String> {
    let root = apps_root();
    fs::create_dir_all(&root)
        .map_err(|error| format!("Could not create app directory: {error}"))?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let staging = root.join(format!(".{}-staging-{nonce}", app.id));
    let target = root.join(&app.id);
    let backup = root.join(format!(".{}-previous", app.id));
    fs::create_dir_all(&staging).map_err(|error| format!("Could not stage app: {error}"))?;
    let result = (|| {
        extract_zip_safely(package, &staging)?;
        let executable =
            find_executable(&staging, &[app.package_id(), &app.id]).ok_or_else(|| {
                format!(
                    "The verified archive did not contain {}.exe",
                    app.package_id()
                )
            })?;
        let relative = executable
            .strip_prefix(&staging)
            .map_err(|_| "Invalid executable path".to_owned())?
            .to_owned();
        if backup.exists() {
            fs::remove_dir_all(&backup)
                .map_err(|error| format!("Could not clear backup: {error}"))?;
        }
        if target.exists() {
            fs::rename(&target, &backup)
                .map_err(|error| format!("Could not stage previous version: {error}"))?;
        }
        if let Err(error) = fs::rename(&staging, &target) {
            if backup.exists() {
                let _ = fs::rename(&backup, &target);
            }
            return Err(format!("Could not activate app: {error}"));
        }
        if backup.exists() {
            fs::remove_dir_all(&backup)
                .map_err(|error| format!("Could not remove backup: {error}"))?;
        }
        Ok(target.join(relative))
    })();
    if staging.exists() {
        let _ = fs::remove_dir_all(&staging);
    }
    result
}

#[cfg(target_os = "windows")]
fn extract_zip_safely(package: &Path, destination: &Path) -> Result<(), String> {
    let script = r#"
Add-Type -AssemblyName System.IO.Compression.FileSystem
$archive = [IO.Compression.ZipFile]::OpenRead($args[0])
$root = [IO.Path]::GetFullPath($args[1]) + [IO.Path]::DirectorySeparatorChar
try {
  foreach ($entry in $archive.Entries) {
    $target = [IO.Path]::GetFullPath([IO.Path]::Combine($args[1], $entry.FullName))
    if (-not $target.StartsWith($root, [StringComparison]::OrdinalIgnoreCase)) { throw 'Unsafe path in ZIP package.' }
    if ([String]::IsNullOrEmpty($entry.Name)) { [IO.Directory]::CreateDirectory($target) | Out-Null; continue }
    [IO.Directory]::CreateDirectory([IO.Path]::GetDirectoryName($target)) | Out-Null
    [IO.Compression.ZipFileExtensions]::ExtractToFile($entry, $target, $true)
  }
} finally { $archive.Dispose() }
"#;
    let output = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .arg(package)
        .arg(destination)
        .output()
        .map_err(|error| format!("Could not extract package: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "Could not extract package: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(())
}

fn curl_program() -> &'static str {
    #[cfg(target_os = "windows")]
    return "curl.exe";
    #[allow(unreachable_code)]
    "curl"
}

fn curl_bytes(url: &str) -> Result<Vec<u8>, String> {
    let output = Command::new(curl_program())
        .args([
            "-fsSL",
            "--retry",
            "2",
            "--connect-timeout",
            "20",
            "--user-agent",
            USER_AGENT,
            url,
        ])
        .output()
        .map_err(|error| format!("Could not start network request: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "Network request failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(output.stdout)
}

fn sha256_file(path: &Path) -> Result<String, String> {
    #[cfg(target_os = "windows")]
    let output = Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "(Get-FileHash -LiteralPath $args[0] -Algorithm SHA256).Hash",
        ])
        .arg(path)
        .output()
        .map_err(|error| format!("Could not verify package: {error}"))?;
    #[cfg(not(target_os = "windows"))]
    let output = Command::new("shasum")
        .args(["-a", "256"])
        .arg(path)
        .output()
        .map_err(|error| format!("Could not verify package: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "Could not verify package: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let hash = text
        .split_whitespace()
        .next()
        .ok_or_else(|| "Checksum tool returned no hash".to_owned())?;
    if hash.len() != 64 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("Checksum tool returned an invalid SHA-256".into());
    }
    Ok(hash.to_ascii_lowercase())
}

#[cfg(target_os = "windows")]
fn find_executable(directory: &Path, accepted_stems: &[&str]) -> Option<PathBuf> {
    let mut directories = vec![directory.to_owned()];
    let mut matches = Vec::new();
    while let Some(current) = directories.pop() {
        let entries = fs::read_dir(current).ok()?;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                directories.push(path);
            } else if path
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"))
                && path.file_stem().is_some_and(|stem| {
                    accepted_stems
                        .iter()
                        .any(|accepted| stem.eq_ignore_ascii_case(accepted))
                })
            {
                matches.push(path);
            }
        }
    }
    matches.sort_by_key(|path| path.components().count());
    matches.into_iter().next()
}

#[cfg(target_os = "macos")]
fn install_dmg(app: &AppManifest, package: &Path) -> Result<PathBuf, String> {
    let output = Command::new("hdiutil")
        .args(["attach", "-nobrowse", "-readonly"])
        .arg(package)
        .output()
        .map_err(|error| format!("Could not mount package: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "Could not mount package: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mount = stdout
        .lines()
        .filter_map(|line| line.split('\t').next_back())
        .map(PathBuf::from)
        .find(|path| path.starts_with("/Volumes/"))
        .ok_or_else(|| "Could not locate mounted package".to_owned())?;
    let result = (|| {
        let source = fs::read_dir(&mount)
            .map_err(|error| format!("Could not inspect package: {error}"))?
            .flatten()
            .map(|entry| entry.path())
            .find(|path| {
                path.extension()
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("app"))
            })
            .ok_or_else(|| "The verified package did not contain an application".to_owned())?;
        let target_dir = apps_root().join(&app.id);
        fs::create_dir_all(&target_dir)
            .map_err(|error| format!("Could not create app directory: {error}"))?;
        let target = target_dir.join(
            source
                .file_name()
                .ok_or_else(|| "Invalid app bundle name".to_owned())?,
        );
        if target.exists() {
            fs::remove_dir_all(&target)
                .map_err(|error| format!("Could not replace app: {error}"))?;
        }
        let status = Command::new("ditto")
            .arg(&source)
            .arg(&target)
            .status()
            .map_err(|error| format!("Could not copy app: {error}"))?;
        status
            .success()
            .then_some(target)
            .ok_or_else(|| "Could not copy app bundle".to_owned())
    })();
    let _ = Command::new("hdiutil").arg("detach").arg(&mount).status();
    result
}

fn suite_root() -> PathBuf {
    #[cfg(target_os = "windows")]
    if let Some(path) = std::env::var_os("LOCALAPPDATA") {
        return PathBuf::from(path).join("ArtCraftSuite");
    }
    #[cfg(target_os = "macos")]
    if let Some(path) = std::env::var_os("HOME") {
        return PathBuf::from(path)
            .join("Library")
            .join("Application Support")
            .join("ArtCraftSuite");
    }
    std::env::temp_dir().join("ArtCraftSuite")
}

fn apps_root() -> PathBuf {
    suite_root().join("apps")
}

fn state_path() -> PathBuf {
    suite_root().join("state.json")
}

fn save_state(state: &InstallState) -> Result<(), String> {
    let path = state_path();
    let parent = path
        .parent()
        .ok_or_else(|| "Invalid state path".to_owned())?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("Could not create state directory: {error}"))?;
    let temporary = path.with_extension("json.tmp");
    let json = serde_json::to_vec_pretty(state)
        .map_err(|error| format!("Could not serialize state: {error}"))?;
    fs::write(&temporary, json).map_err(|error| format!("Could not write state: {error}"))?;
    if path.exists() {
        fs::remove_file(&path).map_err(|error| format!("Could not replace state: {error}"))?;
    }
    fs::rename(temporary, path).map_err(|error| format!("Could not save state: {error}"))
}

fn normalized_with_separator(path: &Path) -> Result<String, String> {
    let full = fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
    let mut text = full.to_string_lossy().into_owned();
    if !text.ends_with(std::path::MAIN_SEPARATOR) {
        text.push(std::path::MAIN_SEPARATOR);
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digest_parser_only_accepts_sha256_hex() {
        assert_eq!(
            parse_digest(&format!("sha256:{}", "a".repeat(64))),
            Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
        );
        assert_eq!(parse_digest("sha256:nope"), None);
        assert_eq!(parse_digest(&format!("sha512:{}", "a".repeat(64))), None);
    }
}
