//! Native dependency provisioning for `docfoo scan`.
//!
//! `docfoo setup` makes `models_dir` usable:
//!   - copies the layout model from a local DocFoo checkout (`--from`) or a
//!     `--layout-model-url` override;
//!   - downloads the pinned Linux ONNX Runtime + PDFium shared libraries
//!     (checksummed) and extracts the needed member;
//!   - reports the effective paths, honoring the `DOCFOO_LAYOUT_MODEL`,
//!     `DOCFOO_ORT_DLL` and `DOCFOO_PDFIUM_DLL` overrides the pipeline uses.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{json, Value};

use crate::error::{CliError, Result};
use crate::workspace::Workspace;

/// ONNX Runtime 1.28.0 — the same version as the Windows DLL the app bundles.
#[cfg(not(windows))]
const ORT_URL: &str =
    "https://github.com/microsoft/onnxruntime/releases/download/v1.28.0/onnxruntime-linux-x64-1.28.0.tgz";
#[cfg(not(windows))]
const ORT_SHA256: &str = "a3e1b79d7bb1bf09696ce675f49e4064e6c81f6202b8225624fff0e93f8d6407";
#[cfg(not(windows))]
const ORT_MEMBER: &str = "onnxruntime-linux-x64-1.28.0/lib/libonnxruntime.so.1.28.0";

/// PDFium 151.0.7881.0 (Chromium 7881) — matches the bundled Windows DLL.
#[cfg(not(windows))]
const PDFIUM_URL: &str =
    "https://github.com/bblanchon/pdfium-binaries/releases/download/chromium%2F7881/pdfium-linux-x64.tgz";
#[cfg(not(windows))]
const PDFIUM_SHA256: &str = "1470e21b8b4a3b4ad7f85684e2da11d94f3b69a86d81dee11b9b6709d927ac1d";
#[cfg(not(windows))]
const PDFIUM_MEMBER: &str = "lib/libpdfium.so";

const DOWNLOAD_TIMEOUT_SECS: u64 = 900;

#[derive(Debug, Clone)]
pub struct SetupStatus {
    pub models_dir: PathBuf,
    pub layout_model: Option<PathBuf>,
    pub ort_library: Option<PathBuf>,
    pub pdfium_library: Option<PathBuf>,
}

impl SetupStatus {
    pub fn ready(&self) -> bool {
        self.layout_model.is_some() && self.ort_library.is_some() && self.pdfium_library.is_some()
    }

    pub fn missing(&self) -> Vec<&'static str> {
        let mut missing = Vec::new();
        if self.layout_model.is_none() {
            missing.push("layout model (PP-DocLayoutV3.onnx)");
        }
        if self.ort_library.is_none() {
            missing.push("ONNX Runtime");
        }
        if self.pdfium_library.is_none() {
            missing.push("PDFium");
        }
        missing
    }

    pub fn to_json(&self) -> Value {
        json!({
            "modelsDir": self.models_dir.display().to_string(),
            "layoutModel": self.layout_model.as_ref().map(|p| p.display().to_string()),
            "ortLibrary": self.ort_library.as_ref().map(|p| p.display().to_string()),
            "pdfiumLibrary": self.pdfium_library.as_ref().map(|p| p.display().to_string()),
            "ready": self.ready(),
            "missing": self.missing(),
        })
    }
}

fn effective(env: &str, models_dir: &Path, name: &str) -> Option<PathBuf> {
    if let Some(value) = std::env::var_os(env).filter(|value| !value.is_empty()) {
        let path = PathBuf::from(value);
        if path.is_file() {
            return Some(path);
        }
    }
    let path = models_dir.join(name);
    path.is_file().then_some(path)
}

pub fn check(workspace: &Workspace) -> SetupStatus {
    let models_dir = workspace.models_dir.clone();
    SetupStatus {
        layout_model: effective(
            "DOCFOO_LAYOUT_MODEL",
            &models_dir,
            docfoo_ocr::layout_model_file_name(),
        ),
        ort_library: effective(
            "DOCFOO_ORT_DLL",
            &models_dir,
            docfoo_ocr::ort_library_file_name(),
        ),
        pdfium_library: effective(
            "DOCFOO_PDFIUM_DLL",
            &models_dir,
            docfoo_ocr::pdfium_library_file_name(),
        ),
        models_dir,
    }
}

/// Provision what is missing. Returns the resulting status; callers decide
/// whether a still-incomplete setup is fatal.
pub fn provision(
    workspace: &Workspace,
    from: Option<&Path>,
    force: bool,
    layout_model_url: Option<&str>,
) -> Result<SetupStatus> {
    std::fs::create_dir_all(&workspace.models_dir)?;
    let before = check(workspace);

    if force || before.layout_model.is_none() {
        let destination = workspace
            .models_dir
            .join(docfoo_ocr::layout_model_file_name());
        if let Some(from) = from {
            let source = from.join(docfoo_ocr::layout_model_file_name());
            if !source.is_file() {
                return Err(CliError::Message(format!(
                    "{} was not found in {}",
                    docfoo_ocr::layout_model_file_name(),
                    from.display()
                )));
            }
            eprintln!("copying {} → {}", source.display(), destination.display());
            std::fs::copy(&source, &destination)?;
        } else if let Some(url) = layout_model_url {
            eprintln!("downloading layout model from {url}");
            download_to(url, &destination)?;
        }
    }

    #[cfg(not(windows))]
    {
        let status = check(workspace);
        if force || status.ort_library.is_none() {
            let destination = workspace
                .models_dir
                .join(docfoo_ocr::ort_library_file_name());
            eprintln!("downloading ONNX Runtime 1.28.0…");
            download_member(ORT_URL, ORT_SHA256, ORT_MEMBER, &destination)?;
        }
        let status = check(workspace);
        if force || status.pdfium_library.is_none() {
            let destination = workspace
                .models_dir
                .join(docfoo_ocr::pdfium_library_file_name());
            eprintln!("downloading PDFium 151.0.7881.0…");
            download_member(PDFIUM_URL, PDFIUM_SHA256, PDFIUM_MEMBER, &destination)?;
        }
    }

    #[cfg(windows)]
    {
        // Windows reuses the app's DLLs; `--from` points at its models/ dir.
        if let Some(from) = from {
            for name in ["onnxruntime.dll", "pdfium.dll"] {
                let source = from.join(name);
                let destination = workspace.models_dir.join(name);
                if source.is_file() && (force || !destination.is_file()) {
                    eprintln!("copying {} → {}", source.display(), destination.display());
                    std::fs::copy(&source, &destination)?;
                }
            }
        }
    }

    Ok(check(workspace))
}

fn agent() -> ureq::Agent {
    ureq::config::Config::builder()
        .timeout_per_call(Some(Duration::from_secs(DOWNLOAD_TIMEOUT_SECS)))
        .timeout_connect(Some(Duration::from_secs(15)))
        .http_status_as_error(false)
        .build()
        .new_agent()
}

fn download_to(url: &str, destination: &Path) -> Result<()> {
    let response = agent()
        .get(url)
        .header("User-Agent", concat!("DocFoo-CLI/", env!("CARGO_PKG_VERSION")))
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

/// Download a tarball, verify its checksum, and extract one member to
/// `destination` (written as a regular file, so symlinked members work).
#[cfg(not(windows))]
fn download_member(url: &str, sha256: &str, member: &str, destination: &Path) -> Result<()> {
    let archive_path = destination.with_file_name(format!(
        ".{}.download",
        destination
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| "archive".to_string())
    ));
    download_to(url, &archive_path)?;
    let actual = crate::util::sha256_file(&archive_path)?;
    if !actual.eq_ignore_ascii_case(sha256) {
        let _ = std::fs::remove_file(&archive_path);
        return Err(CliError::Message(format!(
            "checksum mismatch for {url}: expected {sha256}, got {actual}"
        )));
    }

    let result = extract_member(&archive_path, member, destination);
    let _ = std::fs::remove_file(&archive_path);
    result
}

/// Extract one member from a `.tar.gz` to `destination` as a regular file
/// (so symlinked archive members work on every platform).
#[cfg(any(not(windows), test))]
fn extract_member(archive_path: &Path, member: &str, destination: &Path) -> Result<()> {
    let file = std::fs::File::open(archive_path)?;
    let decoder = flate2::read::GzDecoder::new(file);
    let mut archive = tar::Archive::new(decoder);
    let mut found = false;
    for entry in archive.entries().map_err(|error| CliError::Message(error.to_string()))? {
        let mut entry = entry.map_err(|error| CliError::Message(error.to_string()))?;
        let path = entry.path().map_err(|error| CliError::Message(error.to_string()))?;
        if path.to_string_lossy() == member {
            let mut out = std::fs::File::create(destination).map_err(|error| {
                CliError::Message(format!("could not write {}: {error}", destination.display()))
            })?;
            std::io::copy(&mut entry, &mut out)
                .map_err(|error| CliError::Message(error.to_string()))?;
            found = true;
            break;
        }
    }
    if !found {
        return Err(CliError::Message(format!(
            "{member} was not found in the downloaded archive"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effective_prefers_env_overrides() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("custom.so");
        std::fs::write(&file, b"x").unwrap();
        std::env::set_var("DOCFOO_TEST_OVERRIDE_SO", &file);
        let resolved = effective("DOCFOO_TEST_OVERRIDE_SO", temp.path(), "missing.so");
        std::env::remove_var("DOCFOO_TEST_OVERRIDE_SO");
        assert_eq!(resolved.as_deref(), Some(file.as_path()));
    }

    #[test]
    fn missing_status_lists_every_dependency() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = Workspace {
            root: temp.path().to_path_buf(),
            agent_dir: temp.path().join(".agent"),
            models_dir: temp.path().join("models"),
        };
        let status = check(&workspace);
        assert!(!status.ready());
        assert_eq!(status.missing().len(), 3);
    }

    #[test]
    fn extract_member_writes_a_regular_file() {
        use std::io::Write as _;
        let temp = tempfile::tempdir().unwrap();
        let archive_path = temp.path().join("pkg.tgz");
        {
            let file = std::fs::File::create(&archive_path).unwrap();
            let encoder = flate2::write::GzEncoder::new(file, flate2::Compression::default());
            let mut builder = tar::Builder::new(encoder);
            let data = b"hello";
            let mut header = tar::Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder
                .append_data(&mut header, "pkg/lib/libx.so.1", &data[..])
                .unwrap();
            builder.into_inner().unwrap().finish().unwrap();
        }
        let destination = temp.path().join("libx.so");
        extract_member(&archive_path, "pkg/lib/libx.so.1", &destination).unwrap();
        assert_eq!(std::fs::read(&destination).unwrap(), b"hello");
        let _ = std::io::stdout().flush();
    }
}
