//! `docfoo kg --vis` — the live knowledge-graph visualizer.
//!
//! `run` serves a self-contained browser page from an embedded asset bundle
//! over a loopback HTTP server and streams query stage frames plus synthesis
//! deltas to it over SSE. The page is a dependency-free port of the desktop
//! app's `kg-viz` canvas choreography, so both front ends paint the same
//! layout and the same retrieval beats.
//!
//! The module splits into four pieces:
//! - [`hub`]: the in-memory event log behind the SSE stream;
//! - [`projection`]: `graph.json` → the compact canvas projection;
//! - [`server`]: HTTP routing, the query worker and static/asset file serving;
//! - [`assets`]: the embedded frontend bundle.

pub mod hub;
mod assets;
mod projection;
mod server;

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crate::cli::KgArgs;
use crate::config::ModelPreferences;
use crate::error::{CliError, Result};
use crate::kg::settings;
use crate::workspace::Workspace;

use self::hub::EventHub;
use self::server::ServerState;

/// Inputs to a visualizer session, resolved from `docfoo kg --vis` flags.
pub struct VisOptions {
    pub scope: String,
    pub port: Option<u16>,
    pub open: bool,
    pub model: Option<String>,
    pub reasoning: Option<String>,
}

/// A running visualizer server. Dropping it stops the accept loop.
pub struct VisServer {
    pub addr: SocketAddr,
    state: Arc<ServerState>,
    shutdown: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl VisServer {
    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    pub fn hub(&self) -> Arc<EventHub> {
        Arc::clone(&self.state.hub)
    }

    pub fn shutdown_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.shutdown)
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
    let server = tiny_http::Server::http(("127.0.0.1", options.port.unwrap_or(0))).map_err(|error| {
        CliError::Message(format!("could not bind the visualizer server: {error}"))
    })?;
    let addr = server
        .server_addr()
        .to_ip()
        .ok_or_else(|| CliError::Message("visualizer bound to a non-IP address".to_string()))?;

    let settings = settings::load_settings(&workspace);
    let default_reasoning = options
        .reasoning
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| {
            if settings.query.reasoning {
                "default".to_string()
            } else {
                "off".to_string()
            }
        });

    let default_model = resolve_default_model(&workspace, options.model.as_deref());
    let state = Arc::new(ServerState::new(
        workspace,
        options.scope,
        default_model,
        default_reasoning,
    ));

    let shutdown = Arc::new(AtomicBool::new(false));
    let thread = {
        let state = Arc::clone(&state);
        let shutdown = Arc::clone(&shutdown);
        std::thread::spawn(move || server::serve_loop(server, state, shutdown))
    };

    Ok(VisServer {
        addr,
        state,
        shutdown,
        thread: Some(thread),
    })
}

/// `docfoo kg --vis`: serve, announce, open the browser, wait for Ctrl-C.
pub fn run(workspace: &Workspace, args: &KgArgs, scope: &str) -> Result<()> {
    let options = VisOptions {
        scope: scope.to_string(),
        port: args.port,
        open: !args.no_open,
        model: args.model.clone(),
        reasoning: args.reasoning.clone(),
    };
    let server = serve(workspace.clone(), options)?;
    let url = server.url();
    eprintln!("DocFoo KG visualizer → {url}");
    eprintln!("Ctrl-C stops the server.");
    if !args.no_open {
        crate::web::open_browser(&url);
    }

    // Ctrl-C cancels the running query (if any) and stops the server. This is
    // the only signal handler the process registers in vis mode.
    let hub = server.hub();
    let shutdown = server.shutdown_flag();
    let _ = ctrlc::set_handler(move || {
        hub.cancel();
        shutdown.store(true, Ordering::SeqCst);
    });

    while !server.is_shutting_down() {
        std::thread::sleep(Duration::from_millis(150));
    }
    Ok(())
}

/// The initial model: `--model`, then the shared `kg`/`chat` slots.
fn resolve_default_model(workspace: &Workspace, cli_model: Option<&str>) -> Option<String> {
    if let Some(model) = cli_model.map(str::trim).filter(|value| !value.is_empty()) {
        return Some(model.to_string());
    }
    let prefs = ModelPreferences::load(workspace);
    prefs
        .get("kg")
        .ok()
        .flatten()
        .or_else(|| prefs.get("chat").ok().flatten())
        .map(str::to_string)
}
