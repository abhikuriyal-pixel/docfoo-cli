//! Shared assets for the local browsers.
//!
//! `docfoo kg --vis` and `docfoo resources --vis` are served from the same
//! binary and must look identical, so the theme base, the markdown renderer,
//! the theme picker and the vendored KaTeX live here once and are served by
//! both. KaTeX (MIT, `assets/katex/LICENSE`) is vendored from the desktop
//! app's `node_modules` so math renders with the same engine and fonts.

pub mod assets;
pub mod http;

/// Best-effort browser launch; callers always print the URL as a fallback.
pub fn open_browser(url: &str) {
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut command = std::process::Command::new("rundll32");
        command.args(["url.dll,FileProtocolHandler", url]);
        command
    };
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut command = std::process::Command::new("open");
        command.arg(url);
        command
    };
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let mut command = {
        let mut command = std::process::Command::new("xdg-open");
        command.arg(url);
        command
    };

    let spawned = command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .is_ok();

    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    if !spawned {
        let _ = std::process::Command::new("wslview")
            .arg(url)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
    }

    #[cfg(any(target_os = "windows", target_os = "macos"))]
    let _ = spawned;
}
