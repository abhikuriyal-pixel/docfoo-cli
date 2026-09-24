//! `docfoo update` — GitHub release checks and self-update.
//!
//! The repository is `DOCFOO_REPO` (default `abhikuriyal-pixel/docfoo-cli`),
//! and `DOCFOO_UPDATE_API_URL` can point the check at a mock. A release ships
//! `docfoo-cli-<version>-<platform>.tar.gz` (Linux) or `.zip` (Windows) plus a
//! `.sha256` sidecar; self-update verifies the checksum before replacing the
//! `docfoo` and `docfoo-agent` binaries next to the running executable.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::Value;

use crate::error::{CliError, Result};
use crate::util::sha256_file;

const DEFAULT_REPO: &str = "abhikuriyal-pixel/docfoo-cli";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseInfo {
    pub tag: String,
    pub version: String,
    pub asset_name: String,
    pub asset_url: String,
    pub checksum_url: Option<String>,
}

impl ReleaseInfo {
    pub fn is_newer_than(&self, current: &str) -> bool {
        match (
            semver::Version::parse(&self.version),
            semver::Version::parse(current),
        ) {
            (Ok(latest), Ok(current)) => latest > current,
            _ => self.version != current,
        }
    }
}

pub fn repo() -> String {
    std::env::var("DOCFOO_REPO")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_REPO.to_string())
}

pub fn platform_tag() -> &'static str {
    if cfg!(windows) {
        "windows-x64"
    } else if cfg!(target_arch = "aarch64") {
        "linux-arm64"
    } else {
        "linux-x64"
    }
}

fn archive_suffix() -> &'static str {
    if cfg!(windows) {
        ".zip"
    } else {
        ".tar.gz"
    }
}

fn agent() -> ureq::Agent {
    ureq::config::Config::builder()
        .timeout_per_call(Some(Duration::from_secs(60)))
        .timeout_connect(Some(Duration::from_secs(15)))
        .http_status_as_error(false)
        .build()
        .new_agent()
}

fn user_agent() -> String {
    format!("DocFoo-CLI/{}", env!("CARGO_PKG_VERSION"))
}

/// Parse a GitHub `releases/latest` body for the current platform.
pub fn parse_release(body: &Value) -> Option<ReleaseInfo> {
    let tag = body.get("tag_name")?.as_str()?.to_string();
    let version = tag.trim_start_matches('v').to_string();
    let wanted = format!("docfoo-cli-{version}-{}{}", platform_tag(), archive_suffix());
    let assets = body.get("assets")?.as_array()?;
    let asset = assets
        .iter()
        .find(|asset| asset_name(asset) == wanted)
        .or_else(|| {
            assets
                .iter()
                .find(|asset| asset_name(asset).contains(platform_tag()))
        })?;
    let checksum_url = assets
        .iter()
        .find(|asset| asset_name(asset) == format!("{wanted}.sha256"))
        .map(asset_url)
        .filter(|url| !url.is_empty());
    Some(ReleaseInfo {
        tag,
        version,
        asset_name: asset_name(asset).to_string(),
        asset_url: asset_url(asset),
        checksum_url,
    })
}

fn asset_name(asset: &Value) -> &str {
    asset
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default()
}

fn asset_url(asset: &Value) -> String {
    asset
        .get("browser_download_url")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Latest release for this platform, or `None` when the repo has none.
pub fn check() -> Result<Option<ReleaseInfo>> {
    let api = std::env::var("DOCFOO_UPDATE_API_URL").unwrap_or_else(|_| {
        format!(
            "https://api.github.com/repos/{}/releases/latest",
            repo()
        )
    });
    let mut response = agent()
        .get(&api)
        .header("User-Agent", &user_agent())
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|error| CliError::Message(format!("update check failed: {error}")))?;
    let status = response.status().as_u16();
    let text = response
        .body_mut()
        .read_to_string()
        .map_err(|error| CliError::Message(format!("update check failed: {error}")))?;
    if status == 404 {
        return Ok(None);
    }
    if !(200..300).contains(&status) {
        return Err(CliError::Message(format!(
            "update check failed: GitHub returned {status}"
        )));
    }
    let body: Value = serde_json::from_str(&text)
        .map_err(|error| CliError::Message(format!("update check failed: {error}")))?;
    Ok(parse_release(&body))
}

fn download(url: &str, destination: &Path) -> Result<()> {
    let response = agent()
        .get(url)
        .header("User-Agent", &user_agent())
        .call()
        .map_err(|error| CliError::Message(format!("download failed: {error}")))?;
    let status = response.status().as_u16();
    if status != 200 {
        return Err(CliError::Message(format!(
            "download failed: server error {status}"
        )));
    }
    let mut reader = response.into_body().into_reader();
    let mut file = std::fs::File::create(destination)
        .map_err(|error| CliError::Message(format!("could not write {}: {error}", destination.display())))?;
    std::io::copy(&mut reader, &mut file)
        .map_err(|error| CliError::Message(format!("download failed: {error}")))?;
    Ok(())
}

fn read_checksum(url: &str) -> Result<String> {
    let mut response = agent()
        .get(url)
        .header("User-Agent", &user_agent())
        .call()
        .map_err(|error| CliError::Message(format!("checksum download failed: {error}")))?;
    let text = response
        .body_mut()
        .read_to_string()
        .map_err(|error| CliError::Message(format!("checksum download failed: {error}")))?;
    text.split_whitespace()
        .next()
        .map(str::to_string)
        .ok_or_else(|| CliError::Message("the release checksum file is empty".to_string()))
}

fn extract(archive: &Path, destination: &Path) -> Result<()> {
    std::fs::create_dir_all(destination)?;
    if archive.extension().and_then(|ext| ext.to_str()) == Some("zip") {
        let file = std::fs::File::open(archive)?;
        let mut zip = ::zip::ZipArchive::new(file)
            .map_err(|error| CliError::Message(format!("could not open the update archive: {error}")))?;
        zip.extract(destination)
            .map_err(|error| CliError::Message(format!("could not extract the update: {error}")))?;
        return Ok(());
    }
    let file = std::fs::File::open(archive)?;
    let decoder = flate2::read::GzDecoder::new(file);
    let mut tar = tar::Archive::new(decoder);
    tar.unpack(destination)
        .map_err(|error| CliError::Message(format!("could not extract the update: {error}")))?;
    Ok(())
}

fn find_extracted(root: &Path, name: &str) -> Option<PathBuf> {
    let direct = root.join(name);
    if direct.is_file() {
        return Some(direct);
    }
    let read = std::fs::read_dir(root).ok()?;
    for entry in read.flatten() {
        if entry.path().is_dir() {
            if let Some(found) = find_extracted(&entry.path(), name) {
                return Some(found);
            }
        }
    }
    None
}

/// Replace `docfoo` and `docfoo-agent` next to the running executable.
/// On Windows a running executable cannot be replaced, so new files are left
/// as `.new` with instructions.
pub fn self_update(release: &ReleaseInfo) -> Result<PathBuf> {
    let exe = std::env::current_exe()
        .map_err(|error| CliError::Message(format!("could not locate the running binary: {error}")))?;
    let exe_dir = exe
        .parent()
        .ok_or_else(|| CliError::Message("could not locate the install directory".to_string()))?
        .to_path_buf();
    let temp = exe_dir.join(format!(".update-{}", release.version));
    let _ = std::fs::remove_dir_all(&temp);
    std::fs::create_dir_all(&temp)?;
    let archive = temp.join(&release.asset_name);
    eprintln!("downloading {}…", release.asset_name);
    download(&release.asset_url, &archive)?;

    let expected = match &release.checksum_url {
        Some(url) => Some(read_checksum(url)?),
        None => {
            return Err(CliError::Message(
                "the release has no .sha256 checksum — refusing to self-update".to_string(),
            ))
        }
    };
    if let Some(expected) = expected {
        let actual = sha256_file(&archive)?;
        if !actual.eq_ignore_ascii_case(&expected) {
            let _ = std::fs::remove_dir_all(&temp);
            return Err(CliError::Message(format!(
                "checksum mismatch: expected {expected}, got {actual}"
            )));
        }
    }

    let extracted = temp.join("extracted");
    extract(&archive, &extracted)?;
    let mut installed = Vec::new();
    for stem in ["docfoo", "docfoo-agent"] {
        let name = if cfg!(windows) {
            format!("{stem}.exe")
        } else {
            stem.to_string()
        };
        let Some(source) = find_extracted(&extracted, &name) else {
            continue;
        };
        let target = exe_dir.join(&name);
        if cfg!(windows) {
            let staged = exe_dir.join(format!("{name}.new"));
            std::fs::copy(&source, &staged)?;
            installed.push(staged);
        } else {
            let staged = exe_dir.join(format!(".{name}.new"));
            std::fs::copy(&source, &staged)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755));
            }
            std::fs::rename(&staged, &target).map_err(|error| {
                CliError::Message(format!("could not replace {}: {error}", target.display()))
            })?;
            installed.push(target);
        }
    }
    let _ = std::fs::remove_dir_all(&temp);
    if installed.is_empty() {
        return Err(CliError::Message(
            "the update archive did not contain docfoo or docfoo-agent".to_string(),
        ));
    }
    Ok(exe_dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn release_body(tag: &str, asset_name: &str) -> Value {
        json!({
            "tag_name": tag,
            "assets": [
                { "name": asset_name, "browser_download_url": "https://example.com/a" },
                { "name": format!("{asset_name}.sha256"), "browser_download_url": "https://example.com/a.sha256" }
            ]
        })
    }

    #[test]
    fn parses_the_platform_asset_and_checksum() {
        let name = format!("docfoo-cli-0.2.0-{}{}", platform_tag(), archive_suffix());
        let release = parse_release(&release_body("v0.2.0", &name)).unwrap();
        assert_eq!(release.tag, "v0.2.0");
        assert_eq!(release.version, "0.2.0");
        assert_eq!(release.asset_name, name);
        assert_eq!(release.asset_url, "https://example.com/a");
        assert_eq!(release.checksum_url.as_deref(), Some("https://example.com/a.sha256"));
    }

    #[test]
    fn ignores_releases_without_a_platform_asset() {
        assert!(parse_release(&release_body("v0.2.0", "docfoo-cli-0.2.0-solaris.tgz")).is_none());
    }

    #[test]
    fn version_comparison_uses_semver() {
        let release = ReleaseInfo {
            tag: "v0.2.0".to_string(),
            version: "0.2.0".to_string(),
            asset_name: "a".to_string(),
            asset_url: "u".to_string(),
            checksum_url: None,
        };
        assert!(release.is_newer_than("0.1.0"));
        assert!(!release.is_newer_than("0.2.0"));
        assert!(!release.is_newer_than("0.3.0"));
    }
}
