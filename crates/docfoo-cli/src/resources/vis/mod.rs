//! `docfoo resources --vis` — a read-only browser for the resource library.
//!
//! Serves an embedded page over loopback HTTP: a card browser with random
//! figure covers (die button to re-roll), a document reader with outline and
//! figure/text sliders, and highlight notes written to the desktop app's
//! `notes.json`. The page never writes resource files — no create, rename,
//! delete, move or edit.
//!
//! - [`server`]: HTTP routing and the resource/notes API;
//! - [`assets`]: the embedded frontend bundle.

pub mod assets;
pub mod server;

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crate::cli::ResourcesArgs;
use crate::error::{CliError, Result};
use crate::resources::read;
use crate::workspace::Workspace;

use self::server::ServerState;

/// Inputs to a resource-browser session, resolved from `resources --vis`.
pub struct VisOptions {
    /// Folder to open first (empty = the library root).
    pub rel: String,
    pub port: Option<u16>,
    pub open: bool,
}

/// A running resource-browser server. Dropping it stops the accept loop.
pub struct VisServer {
    pub addr: SocketAddr,
    shutdown: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl VisServer {
    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    pub fn is_shutting_down(&self) -> bool {
        self.shutdown.load(Ordering::SeqCst)
    }

    pub fn shutdown(&self) {
        self.shutdown.store(true, Ordering::SeqCst);
    }
}

impl Drop for VisServer {
    fn drop(&mut self) {
        self.shutdown();
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }
}

/// Start the server (used by `run` and by tests, which pass `open: false`).
pub fn serve(workspace: Workspace, options: VisOptions) -> Result<VisServer> {
    let root = workspace.resources_dir();
    let start_rel = options.rel.trim().replace('\\', "/");
    let start_rel = start_rel.trim_matches('/').to_string();
    let rel = if start_rel.is_empty() {
        String::new()
    } else {
        let (normalized, path) = read::resolve_rel(&root, &start_rel)?;
        if !path.is_dir() {
            return Err(CliError::NotFound(format!(
                "resource folder not found: {normalized}"
            )));
        }
        normalized
    };
    start(workspace, rel, options.port)
}

fn start(workspace: Workspace, rel: String, port: Option<u16>) -> Result<VisServer> {
    let server = tiny_http::Server::http(("127.0.0.1", port.unwrap_or(0))).map_err(|error| {
        CliError::Message(format!("could not bind the resource browser server: {error}"))
    })?;
    let addr = server
        .server_addr()
        .to_ip()
        .ok_or_else(|| CliError::Message("resource browser bound to a non-IP address".to_string()))?;

    let state = Arc::new(ServerState::new(workspace, rel));
    let shutdown = Arc::new(AtomicBool::new(false));
    let thread = {
        let state = Arc::clone(&state);
        let shutdown = Arc::clone(&shutdown);
        std::thread::spawn(move || server::serve_loop(server, state, shutdown))
    };

    Ok(VisServer {
        addr,
        shutdown,
        thread: Some(thread),
    })
}

/// `docfoo resources --vis`: serve, announce, open the browser, wait for Ctrl-C.
pub fn run(workspace: &Workspace, args: &ResourcesArgs, rel: &str) -> Result<()> {
    let options = VisOptions {
        rel: rel.to_string(),
        port: args.port,
        open: !args.no_open,
    };
    let server = serve(workspace.clone(), options)?;
    let url = server.url();
    eprintln!("DocFoo resources → {url}");
    eprintln!("Ctrl-C stops the server.");
    if !args.no_open {
        crate::web::open_browser(&url);
    }

    let shutdown = Arc::clone(&server.shutdown);
    let _ = ctrlc::set_handler(move || {
        shutdown.store(true, Ordering::SeqCst);
    });

    while !server.is_shutting_down() {
        std::thread::sleep(Duration::from_millis(150));
    }
    Ok(())
}
