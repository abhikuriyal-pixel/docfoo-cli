//! Native dependency provisioning for `docfoo scan`.
//!
//! `docfoo setup` makes `workspace.models_dir` (default `<workspace>/models`,
//! i.e. `~/.docfoo/models`) ready for scanning:
//!
//!   - the PP-DocLayoutV3 layout model (`PP-DocLayoutV3.onnx`);
//!   - ONNX Runtime 1.28.0, loaded dynamically by `ort`;
//!   - PDFium (Chromium 7881), loaded dynamically by `pdfium-render`.
//!
//! Every dependency is pinned to an exact version and verified by SHA-256;
//! files that are already present and valid are skipped. `--from DIR` copies
//! the layout model from a local DocFoo models directory instead of
//! downloading it, and `--layout-model-url URL` points the model download at a
//! custom URL. The `DOCFOO_LAYOUT_MODEL`, `DOCFOO_ORT_DLL` and
//! `DOCFOO_PDFIUM_DLL` env vars still take precedence over the models dir.

use std::io::{IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{json, Value};

use crate::error::{CliError, Result};
use crate::workspace::Workspace;

/// PP-DocLayoutV3 layout model — a pinned copy hosted on the public releases
/// repo, so a clean machine needs no desktop app or manual file selection.
const MODEL_URL: &str = "https://github.com/abhikuriyal-pixel/docfoo-cli/releases/download/deps-v1/PP-DocLayoutV3.onnx";
const MODEL_SHA256: &str = "d24809294b2f9f1a9a2767043a64df2714b66e5be056887be2233d1117d784f6";

const DOWNLOAD_TIMEOUT_SECS: u64 = 900;
/// Number of provisioned dependencies (model, ONNX Runtime, PDFium).
const STEPS: u8 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArchiveKind {
    TarGz,
    /// Only used by the Windows ONNX Runtime archive.
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    Zip,
}

/// A pinned, checksummed archive that provides one or more files in the models
/// directory.
struct RemoteDep {
    label: &'static str,
    /// Env var that, when it points at an existing file, replaces this
    /// dependency entirely.
    env: &'static str,
    url: &'static str,
    sha256: &'static str,
    kind: ArchiveKind,
    /// `(member inside the archive, file name in the models dir, sha256 of the
    /// extracted file)`.
    files: &'static [(&'static str, &'static str, &'static str)],
}

#[cfg(windows)]
const ORT: RemoteDep = RemoteDep {
    label: "ONNX Runtime 1.28.0",
    env: "DOCFOO_ORT_DLL",
    url: "https://github.com/microsoft/onnxruntime/releases/download/v1.28.0/onnxruntime-win-x64-1.28.0.zip",
    sha256: "abef733dacbe2f571547a7150b479b5cb9cc0df22f96c24983a42cadb1b4f8bc",
    kind: ArchiveKind::Zip,
    files: &[
        (
            "onnxruntime-win-x64-1.28.0/lib/onnxruntime.dll",
            "onnxruntime.dll",
            "18370c375f07357fa5874344a9d9ac17e6b6fe1eb18b1dd209d79483b4470257",
        ),
        (
            "onnxruntime-win-x64-1.28.0/lib/onnxruntime_providers_shared.dll",
            "onnxruntime_providers_shared.dll",
            "599629fa643707defe9156140ae5edd73531f221aa97b7585b1c9bb0a93586f8",
        ),
    ],
};

#[cfg(not(windows))]
const ORT: RemoteDep = RemoteDep {
    label: "ONNX Runtime 1.28.0",
    env: "DOCFOO_ORT_DLL",
    url: "https://github.com/microsoft/onnxruntime/releases/download/v1.28.0/onnxruntime-linux-x64-1.28.0.tgz",
    sha256: "a3e1b79d7bb1bf09696ce675f49e4064e6c81f6202b8225624fff0e93f8d6407",
    kind: ArchiveKind::TarGz,
    files: &[
        (
            "onnxruntime-linux-x64-1.28.0/lib/libonnxruntime.so.1.28.0",
            "libonnxruntime.so",
            "1461ef7cc3d9e49982591721683cc3e3a55580aeca9a5254e7aac47b75ee4bab",
        ),
        (
            "onnxruntime-linux-x64-1.28.0/lib/libonnxruntime_providers_shared.so",
            "libonnxruntime_providers_shared.so",
            "086ec1d5388f64153d9c63470d126693db9a182c8ce236d3a1119068471b8a0d",
        ),
    ],
};

#[cfg(windows)]
const PDFIUM: RemoteDep = RemoteDep {
    label: "PDFium 151.0.7881.0",
    env: "DOCFOO_PDFIUM_DLL",
    url: "https://github.com/bblanchon/pdfium-binaries/releases/download/chromium%2F7881/pdfium-win-x64.tgz",
    sha256: "73cc0de638ac2095e7445bf56a38200a5b7c7ca0e9f4ba144598f2457377ac08",
    kind: ArchiveKind::TarGz,
    files: &[(
        "bin/pdfium.dll",
        "pdfium.dll",
        "79d4676b656cfb1abcea88f9ade3b4b0826c5200382db5f4ec72a636c598c118",
    )],
};

#[cfg(not(windows))]
const PDFIUM: RemoteDep = RemoteDep {
    label: "PDFium 151.0.7881.0",
    env: "DOCFOO_PDFIUM_DLL",
    url: "https://github.com/bblanchon/pdfium-binaries/releases/download/chromium%2F7881/pdfium-linux-x64.tgz",
    sha256: "1470e21b8b4a3b4ad7f85684e2da11d94f3b69a86d81dee11b9b6709d927ac1d",
    kind: ArchiveKind::TarGz,
    files: &[(
        "lib/libpdfium.so",
        "libpdfium.so",
        "f728930966f503652b92acc89b9374a2eeca00ce42e26dccd3e4b5c5161b2d64",
    )],
};

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
///
/// Progress goes to stderr; the caller emits the final JSON envelope.
pub fn provision(
    workspace: &Workspace,
    from: Option<&Path>,
    force: bool,
    layout_model_url: Option<&str>,
) -> Result<SetupStatus> {
    std::fs::create_dir_all(&workspace.models_dir)?;
    eprintln!("setup: models dir {}", workspace.models_dir.display());

    // 1. Layout model.
    let model_name = docfoo_ocr::layout_model_file_name();
    let model_destination = workspace.models_dir.join(model_name);
    let model_override = std::env::var_os("DOCFOO_LAYOUT_MODEL")
        .map(PathBuf::from)
        .filter(|path| path.is_file());
    if let Some(path) = model_override {
        progress(
            1,
            &format!("layout model: using DOCFOO_LAYOUT_MODEL override ({})", path.display()),
        );
    } else if force || !is_valid(&model_destination, MODEL_SHA256) {
        match (from, layout_model_url) {
            (Some(from), _) => copy_layout_model(1, from, &model_destination)?,
            (None, Some(url)) => download_file(1, "layout model", url, None, &model_destination)?,
            (None, None) => download_file(
                1,
                "layout model",
                MODEL_URL,
                Some(MODEL_SHA256),
                &model_destination,
            )?,
        }
    } else {
        progress(1, "layout model: ok (sha256 verified)");
    }

    // 2. ONNX Runtime.
    ensure_remote(2, &ORT, &workspace.models_dir, force)?;

    // 3. PDFium.
    ensure_remote(3, &PDFIUM, &workspace.models_dir, force)?;

    Ok(check(workspace))
}

/// True when the path exists and its SHA-256 matches the pinned value.
fn is_valid(path: &Path, sha256: &str) -> bool {
    path.is_file()
        && crate::util::sha256_file(path)
            .map(|actual| actual.eq_ignore_ascii_case(sha256))
            .unwrap_or(false)
}

fn progress(step: u8, message: &str) {
    eprintln!("[{step}/{STEPS}] {message}");
}

/// Copy the layout model from a local DocFoo models directory.
fn copy_layout_model(step: u8, from: &Path, destination: &Path) -> Result<()> {
    let name = docfoo_ocr::layout_model_file_name();
    let source = from.join(name);
    if !source.is_file() {
        return Err(CliError::Message(format!(
            "{name} was not found in {}",
            from.display()
        )));
    }
    progress(step, &format!("layout model: copying {}", source.display()));
    std::fs::copy(&source, destination).map_err(|error| {
        CliError::Message(format!("could not copy {}: {error}", source.display()))
    })?;
    let actual = crate::util::sha256_file(destination)?;
    if actual.eq_ignore_ascii_case(MODEL_SHA256) {
        progress(step, "layout model: copied (sha256 verified)");
    } else {
        progress(
            step,
            &format!("layout model: warning: sha256 {actual} differs from the pinned model"),
        );
    }
    Ok(())
}

/// Download a single file, optionally verifying it against a pinned SHA-256.
fn download_file(
    step: u8,
    label: &str,
    url: &str,
    sha256: Option<&str>,
    destination: &Path,
) -> Result<()> {
    match sha256 {
        Some(expected) => {
            progress(step, &format!("{label}: downloading (pinned {})", &expected[..12]));
            download_to(url, destination)?;
            let actual = crate::util::sha256_file(destination)?;
            if !actual.eq_ignore_ascii_case(expected) {
                let _ = std::fs::remove_file(destination);
                return Err(CliError::Message(format!(
                    "checksum mismatch for {url} (expected {expected}, got {actual})"
                )));
            }
            progress(step, &format!("{label}: installed (sha256 verified)"));
        }
        None => {
            progress(step, &format!("{label}: downloading from {url}"));
            download_to(url, destination)?;
            progress(step, &format!("{label}: installed (custom URL, checksum not pinned)"));
        }
    }
    Ok(())
}

/// Install every file of a pinned archive that is missing or invalid.
fn ensure_remote(step: u8, dep: &RemoteDep, models_dir: &Path, force: bool) -> Result<()> {
    if let Some(path) = std::env::var_os(dep.env)
        .map(PathBuf::from)
        .filter(|path| path.is_file())
    {
        progress(
            step,
            &format!("{}: using {} override ({})", dep.label, dep.env, path.display()),
        );
        return Ok(());
    }
    let needed = dep
        .files
        .iter()
        .any(|(_, name, sha)| force || !is_valid(&models_dir.join(name), sha));
    if !needed {
        progress(step, &format!("{}: ok (sha256 verified)", dep.label));
        return Ok(());
    }

    progress(step, &format!("{}: downloading {}", dep.label, dep.url));
    let archive = models_dir.join(format!(
        ".{}.download",
        dep.label.replace(' ', "-").to_lowercase()
    ));
    let result = (|| -> Result<()> {
        download_to(dep.url, &archive)?;
        let actual = crate::util::sha256_file(&archive)?;
        if !actual.eq_ignore_ascii_case(dep.sha256) {
            return Err(CliError::Message(format!(
                "checksum mismatch for {} (expected {}, got {actual})",
                dep.url, dep.sha256
            )));
        }
        progress(step, &format!("{}: sha256 verified, extracting", dep.label));
        for (member, name, sha) in dep.files {
            let destination = models_dir.join(name);
            if !force && is_valid(&destination, sha) {
                continue;
            }
            extract_member(&archive, dep.kind, member, &destination)?;
            let extracted = crate::util::sha256_file(&destination)?;
            if !extracted.eq_ignore_ascii_case(sha) {
                let _ = std::fs::remove_file(&destination);
                return Err(CliError::Message(format!(
                    "{name} from {} failed checksum verification",
                    dep.url
                )));
            }
            progress(step, &format!("{}: installed {name}", dep.label));
        }
        Ok(())
    })();
    let _ = std::fs::remove_file(&archive);
    result
}

fn agent() -> ureq::Agent {
    ureq::config::Config::builder()
        .timeout_per_call(Some(Duration::from_secs(DOWNLOAD_TIMEOUT_SECS)))
        .timeout_connect(Some(Duration::from_secs(15)))
        .http_status_as_error(false)
        .build()
        .new_agent()
}

fn user_agent() -> String {
    format!("DocFoo-CLI/{}", env!("CARGO_PKG_VERSION"))
}

fn human_bytes(bytes: u64) -> String {
    const MIB: f64 = 1024.0 * 1024.0;
    if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / MIB)
    } else {
        format!("{:.0} KB", bytes as f64 / 1024.0)
    }
}

/// Stream a URL to `destination`, writing to a `.part` file first so an
/// interrupted download never leaves a file that later runs would trust.
fn download_to(url: &str, destination: &Path) -> Result<()> {
    let name = destination
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "download".to_string());
    let mut part = destination.to_path_buf();
    part.set_file_name(format!(".{name}.part"));
    let _ = std::fs::remove_file(&part);

    let response = agent()
        .get(url)
        .header("User-Agent", user_agent())
        .call()
        .map_err(|error| CliError::Message(format!("download failed for {url}: {error}")))?;
    let status = response.status().as_u16();
    if status != 200 {
        return Err(CliError::Message(format!(
            "download failed for {url}: server error {status}"
        )));
    }
    let total = response
        .headers()
        .get("content-length")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok());
    match total {
        Some(total) => eprintln!("  {} from {url}", human_bytes(total)),
        None => eprintln!("  from {url}"),
    }

    let mut reader = response.into_body().into_reader();
    let mut file = std::fs::File::create(&part).map_err(|error| {
        CliError::Message(format!("could not write {}: {error}", part.display()))
    })?;
    let tty = std::io::stderr().is_terminal();
    let mut buffer = [0u8; 64 * 1024];
    let mut written: u64 = 0;
    let mut next_percent: u64 = 5;
    let mut last_reported_bytes: u64 = 0;
    let read_error = |error: std::io::Error| {
        CliError::Message(format!("download failed for {url}: {error}"))
    };
    loop {
        let read = reader.read(&mut buffer).map_err(read_error)?;
        if read == 0 {
            break;
        }
        file.write_all(&buffer[..read])
            .map_err(|error| CliError::Message(format!("could not write {}: {error}", part.display())))?;
        written += read as u64;
        if let Some(total) = total.filter(|total| *total > 0) {
            let percent = written.saturating_mul(100) / total;
            if percent >= next_percent {
                next_percent = percent / 5 * 5 + 5;
                if tty {
                    eprint!("\r  {percent}%");
                } else {
                    eprintln!("  {percent}%");
                }
            }
        } else if written / (10 * 1024 * 1024) > last_reported_bytes / (10 * 1024 * 1024) {
            last_reported_bytes = written;
            eprintln!("  {} downloaded", human_bytes(written));
        }
    }
    if tty && total.is_some() {
        eprintln!();
    }
    file.flush()
        .map_err(|error| CliError::Message(format!("could not write {}: {error}", part.display())))?;
    drop(file);
    std::fs::rename(&part, destination).map_err(|error| {
        let _ = std::fs::remove_file(&part);
        CliError::Message(format!(
            "could not write {}: {error}",
            destination.display()
        ))
    })
}

fn extract_member(archive: &Path, kind: ArchiveKind, member: &str, destination: &Path) -> Result<()> {
    match kind {
        ArchiveKind::TarGz => extract_tar_member(archive, member, destination),
        ArchiveKind::Zip => extract_zip_member(archive, member, destination),
    }
}

/// Extract one member from a `.zip` to `destination`.
fn extract_zip_member(archive: &Path, member: &str, destination: &Path) -> Result<()> {
    let file = std::fs::File::open(archive)
        .map_err(|error| CliError::Message(format!("could not read {}: {error}", archive.display())))?;
    let mut zip = zip::ZipArchive::new(file)
        .map_err(|error| CliError::Message(format!("could not read {}: {error}", archive.display())))?;
    let mut entry = zip.by_name(member).map_err(|_| {
        CliError::Message(format!("{member} was not found in {}", archive.display()))
    })?;
    let mut out = std::fs::File::create(destination).map_err(|error| {
        CliError::Message(format!("could not write {}: {error}", destination.display()))
    })?;
    std::io::copy(&mut entry, &mut out)
        .map_err(|error| CliError::Message(format!("could not extract {member}: {error}")))?;
    Ok(())
}

/// Extract one member from a `.tar.gz` to `destination` as a regular file (so
/// symlinked archive members work on every platform).
fn extract_tar_member(archive_path: &Path, member: &str, destination: &Path) -> Result<()> {
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
    fn is_valid_checks_sha256() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("dep.bin");
        std::fs::write(&file, b"hello").unwrap();
        let sha = crate::util::sha256_file(&file).unwrap();
        assert!(is_valid(&file, &sha));
        assert!(!is_valid(&file, &"0".repeat(64)));
        assert!(!is_valid(&temp.path().join("missing.bin"), &sha));
    }

    #[test]
    fn extract_tar_member_writes_a_regular_file() {
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
        extract_member(
            &archive_path,
            ArchiveKind::TarGz,
            "pkg/lib/libx.so.1",
            &destination,
        )
        .unwrap();
        assert_eq!(std::fs::read(&destination).unwrap(), b"hello");
        let _ = std::io::stdout().flush();
    }

    #[test]
    fn extract_zip_member_writes_a_file() {
        use std::io::Write as _;
        let temp = tempfile::tempdir().unwrap();
        let archive_path = temp.path().join("pkg.zip");
        {
            let file = std::fs::File::create(&archive_path).unwrap();
            let mut writer = zip::ZipWriter::new(file);
            let options = zip::write::SimpleFileOptions::default();
            writer.start_file("pkg/lib/x.dll", options).unwrap();
            writer.write_all(b"hello").unwrap();
            writer.finish().unwrap();
        }
        let destination = temp.path().join("x.dll");
        extract_member(&archive_path, ArchiveKind::Zip, "pkg/lib/x.dll", &destination).unwrap();
        assert_eq!(std::fs::read(&destination).unwrap(), b"hello");
    }
}
