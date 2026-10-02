//! Workspace resolution and path layout.
//!
//! Resolution order for the workspace root:
//!   1. `--workspace DIR`
//!   2. `DOCFOO_WORKSPACE`
//!   3. `~/.docfoo` (`USERPROFILE` on Windows, `HOME` elsewhere)
//!
//! The layout matches the desktop app's `db/` folder so `--workspace` can
//! point straight at the app's data. Scan dependencies live in
//! `<root>/models`; `DOCFOO_MODELS_DIR` overrides that location.

use std::path::{Path, PathBuf};

use crate::error::{CliError, Result};

pub const DEFAULT_DIR_NAME: &str = ".docfoo";
pub const AGENT_DIR_NAME: &str = ".agent";
pub const MODELS_DIR_NAME: &str = "models";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workspace {
    pub root: PathBuf,
    pub agent_dir: PathBuf,
    pub models_dir: PathBuf,
}

/// Inputs for [`resolve`], kept explicit so tests never mutate the process
/// environment.
#[derive(Debug, Default, Clone)]
pub struct ResolveInput {
    pub cli_workspace: Option<PathBuf>,
    pub env_workspace: Option<String>,
    pub env_agent_dir: Option<String>,
    pub env_models_dir: Option<String>,
    pub home: Option<PathBuf>,
}

pub fn resolve(input: ResolveInput) -> Result<Workspace> {
    let root = match input.cli_workspace {
        Some(path) => path,
        None => match non_empty(input.env_workspace) {
            Some(value) => PathBuf::from(value),
            None => default_root(input.home.as_deref())?,
        },
    };
    let root = absolutize(&root)?;

    let agent_dir = match non_empty(input.env_agent_dir) {
        Some(value) => absolutize(Path::new(&value))?,
        None => root.join(AGENT_DIR_NAME),
    };

    let models_dir = match non_empty(input.env_models_dir) {
        Some(value) => absolutize(Path::new(&value))?,
        None => root.join(MODELS_DIR_NAME),
    };

    Ok(Workspace {
        root,
        agent_dir,
        models_dir,
    })
}

pub fn resolve_from_env(cli_workspace: Option<&Path>) -> Result<Workspace> {
    resolve(ResolveInput {
        cli_workspace: cli_workspace.map(Path::to_path_buf),
        env_workspace: std::env::var("DOCFOO_WORKSPACE").ok(),
        env_agent_dir: std::env::var("DOCFOO_AGENT_DIR").ok(),
        env_models_dir: std::env::var("DOCFOO_MODELS_DIR").ok(),
        home: home_dir(),
    })
}

impl Workspace {
    pub fn resources_dir(&self) -> PathBuf {
        self.root.join("resources")
    }

    pub fn graphs_dir(&self) -> PathBuf {
        self.root.join("graphs")
    }

    pub fn tmp_dir(&self) -> PathBuf {
        self.root.join("tmp")
    }

    pub fn kg_chats_dir(&self) -> PathBuf {
        self.root.join("kg-chats")
    }

    pub fn notes_path(&self) -> PathBuf {
        self.root.join("notes.json")
    }

    pub fn model_selection_path(&self) -> PathBuf {
        self.root.join("model-selection.json")
    }

    pub fn kg_settings_path(&self) -> PathBuf {
        self.root.join("kg-settings.json")
    }

    /// Create the directories a writing command needs. Read-only commands
    /// (`version`, `help`, `resources --list`) never call this.
    pub fn ensure_scaffold(&self) -> Result<()> {
        for dir in [
            &self.root,
            &self.agent_dir,
            &self.resources_dir(),
            &self.graphs_dir(),
            &self.tmp_dir(),
        ] {
            std::fs::create_dir_all(dir)?;
        }
        Ok(())
    }
}

fn non_empty(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn default_root(home: Option<&Path>) -> Result<PathBuf> {
    let home = home.ok_or_else(|| {
        CliError::Message(
            "cannot determine the home directory; pass --workspace or set DOCFOO_WORKSPACE"
                .to_string(),
        )
    })?;
    Ok(home.join(DEFAULT_DIR_NAME))
}

/// Make a path absolute without requiring it to exist.
pub fn absolutize(path: &Path) -> Result<PathBuf> {
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    let cwd = std::env::current_dir()?;
    Ok(cwd.join(path))
}

fn home_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        std::env::var_os("USERPROFILE").map(PathBuf::from)
    }
    #[cfg(not(windows))]
    {
        std::env::var_os("HOME").map(PathBuf::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(root: &Path) -> ResolveInput {
        ResolveInput {
            cli_workspace: Some(root.join("ws")),
            home: Some(root.join("home")),
            ..ResolveInput::default()
        }
    }

    #[test]
    fn cli_workspace_wins_over_env() {
        let temp = tempfile::tempdir().unwrap();
        let mut resolve_input = input(temp.path());
        resolve_input.env_workspace = Some("ignored".to_string());
        let workspace = resolve(resolve_input).unwrap();
        assert_eq!(workspace.root, temp.path().join("ws"));
    }

    #[test]
    fn env_workspace_used_when_cli_absent() {
        let temp = tempfile::tempdir().unwrap();
        let mut resolve_input = input(temp.path());
        resolve_input.cli_workspace = None;
        resolve_input.env_workspace = Some(temp.path().join("from-env").display().to_string());
        let workspace = resolve(resolve_input).unwrap();
        assert_eq!(workspace.root, temp.path().join("from-env"));
    }

    #[test]
    fn blank_env_workspace_falls_back_to_default() {
        let temp = tempfile::tempdir().unwrap();
        let mut resolve_input = input(temp.path());
        resolve_input.cli_workspace = None;
        resolve_input.env_workspace = Some("   ".to_string());
        let workspace = resolve(resolve_input).unwrap();
        assert_eq!(workspace.root, temp.path().join("home").join(DEFAULT_DIR_NAME));
    }

    #[test]
    fn default_root_is_home_dot_docfoo() {
        let temp = tempfile::tempdir().unwrap();
        let mut resolve_input = input(temp.path());
        resolve_input.cli_workspace = None;
        let workspace = resolve(resolve_input).unwrap();
        assert_eq!(workspace.root, temp.path().join("home").join(DEFAULT_DIR_NAME));
    }

    #[test]
    fn agent_and_models_overrides_are_absolute() {
        let temp = tempfile::tempdir().unwrap();
        let mut resolve_input = input(temp.path());
        resolve_input.env_agent_dir = Some(temp.path().join("agent").display().to_string());
        resolve_input.env_models_dir = Some(temp.path().join("models").display().to_string());
        let workspace = resolve(resolve_input).unwrap();
        assert_eq!(workspace.agent_dir, temp.path().join("agent"));
        assert_eq!(workspace.models_dir, temp.path().join("models"));
    }

    #[test]
    fn default_agent_and_models_paths() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = resolve(input(temp.path())).unwrap();
        assert_eq!(workspace.agent_dir, workspace.root.join(AGENT_DIR_NAME));
        assert_eq!(workspace.models_dir, workspace.root.join(MODELS_DIR_NAME));
    }

    #[test]
    fn ensure_scaffold_creates_expected_dirs() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = resolve(input(temp.path())).unwrap();
        workspace.ensure_scaffold().unwrap();
        for dir in [
            workspace.root.clone(),
            workspace.agent_dir.clone(),
            workspace.resources_dir(),
            workspace.graphs_dir(),
            workspace.tmp_dir(),
        ] {
            assert!(dir.is_dir(), "{} should exist", dir.display());
        }
    }
}
