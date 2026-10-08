use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SuiteManifest {
    pub schema_version: u32,
    pub apps: Vec<AppManifest>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppManifest {
    pub id: String,
    pub name: String,
    pub description: String,
    pub repository: String,
    #[serde(default)]
    pub package_id: Option<String>,
    pub asset_patterns: BTreeMap<String, String>,
}

impl AppManifest {
    pub fn package_id(&self) -> &str {
        self.package_id.as_deref().unwrap_or(&self.id)
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct ApiRelease {
    pub tag_name: String,
    pub draft: bool,
    pub prerelease: bool,
    pub assets: Vec<ApiAsset>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ApiAsset {
    pub name: String,
    pub browser_download_url: String,
    pub size: u64,
    pub digest: Option<String>,
}

#[derive(Clone, Debug)]
pub struct ResolvedRelease {
    pub version: String,
    pub asset: ApiAsset,
    pub sha256: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledApp {
    #[serde(alias = "Id")]
    pub id: String,
    #[serde(alias = "Version")]
    pub version: String,
    #[serde(alias = "ExecutablePath", alias = "executablePath")]
    pub launch_path: String,
    #[serde(alias = "SourceAsset")]
    pub source_asset: String,
    #[serde(alias = "Sha256")]
    pub sha256: String,
    #[serde(default)]
    pub installed_at_unix: u64,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct InstallState {
    #[serde(default, alias = "Apps")]
    pub apps: BTreeMap<String, InstalledApp>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_existing_dotnet_install_state() {
        let json = r#"{
          "Apps": {
            "photocraft": {
              "Id": "photocraft",
              "Version": "0.3.0",
              "ExecutablePath": "C:\\Apps\\photocraft.exe",
              "SourceAsset": "photocraft.zip",
              "Sha256": "abc",
              "InstalledAt": "2026-10-07T12:00:00Z"
            }
          }
        }"#;
        let state: InstallState = serde_json::from_str(json).unwrap();
        assert_eq!(
            state.apps["photocraft"].launch_path,
            "C:\\Apps\\photocraft.exe"
        );
    }
}
