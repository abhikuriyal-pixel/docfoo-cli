//! The `docfoo` command tree.
//!
//! Every command in the plan's §4 surface is defined here, even while most are
//! still stubs, so the CLI contract is stable from Stage 1.1 onward.

use std::path::PathBuf;

use clap::{ArgGroup, Args, Parser, Subcommand, ValueEnum};

use crate::output::OutputFormat;

#[derive(Debug, Parser)]
#[command(
    name = "docfoo",
    version,
    about = "DocFoo CLI — knowledge graphs, scans, resources and backups for scripts and agents",
    propagate_version = true,
    subcommand_required = true,
    arg_required_else_help = true
)]
pub struct Cli {
    /// Workspace directory (default: $DOCFOO_WORKSPACE or ~/.docfoo)
    #[arg(long, global = true, value_name = "DIR")]
    pub workspace: Option<PathBuf>,

    /// Print the machine-readable JSON envelope
    #[arg(long, global = true)]
    pub json: bool,

    /// Output format
    #[arg(long, global = true, value_name = "FORMAT", default_value_t = FormatArg::Markdown, value_enum)]
    pub format: FormatArg,

    /// Suppress progress output
    #[arg(long, global = true)]
    pub quiet: bool,

    /// Extra diagnostics on stderr
    #[arg(long, global = true)]
    pub verbose: bool,

    /// Disable ANSI colors
    #[arg(long, global = true)]
    pub no_color: bool,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum FormatArg {
    Markdown,
    Slack,
    Json,
}

impl From<FormatArg> for OutputFormat {
    fn from(value: FormatArg) -> Self {
        match value {
            FormatArg::Markdown => OutputFormat::Markdown,
            FormatArg::Slack => OutputFormat::Slack,
            FormatArg::Json => OutputFormat::Json,
        }
    }
}

impl Cli {
    /// `--json` wins over `--format` when both are present.
    pub fn output_format(&self) -> OutputFormat {
        if self.json {
            OutputFormat::Json
        } else {
            self.format.into()
        }
    }
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Knowledge graph: index, query, status
    Kg(KgArgs),
    /// OCR a PDF or image into a resource card
    Scan(ScanArgs),
    /// Read-only access to the resource library
    Resources(ResourcesArgs),
    /// Read-only access to notes
    Notes(NotesArgs),
    /// Create a backup zip
    Backup(BackupArgs),
    /// Restore a backup zip
    Restore(RestoreArgs),
    /// Community collections
    Collections(CollectionsArgs),
    /// Model selection
    Model(ModelArgs),
    /// Provider credentials
    Auth(AuthArgs),
    /// Provision native scan dependencies
    Setup(SetupArgs),
    /// Print version information
    Version(VersionArgs),
    /// Check for or install updates
    Update(UpdateArgs),
}

impl Command {
    pub fn name(&self) -> &'static str {
        match self {
            Command::Kg(_) => "kg",
            Command::Scan(_) => "scan",
            Command::Resources(_) => "resources",
            Command::Notes(_) => "notes",
            Command::Backup(_) => "backup",
            Command::Restore(_) => "restore",
            Command::Collections(_) => "collections",
            Command::Model(_) => "model",
            Command::Auth(_) => "auth",
            Command::Setup(_) => "setup",
            Command::Version(_) => "version",
            Command::Update(_) => "update",
        }
    }
}

#[derive(Debug, Args)]
#[command(group = ArgGroup::new("action").required(true).multiple(false).args(["query", "index", "status"]))]
pub struct KgArgs {
    /// Ask the knowledge graph a question (one-shot final answer)
    #[arg(long, value_name = "QUESTION")]
    pub query: Option<String>,
    /// Build or refresh the graph
    #[arg(long)]
    pub index: bool,
    /// Show built graphs and coverage
    #[arg(long)]
    pub status: bool,
    /// Resource folder scope (empty = whole library)
    #[arg(long, value_name = "DIR", default_value = "")]
    pub scope: String,
    /// Model key (`provider/model`)
    #[arg(long, value_name = "KEY")]
    pub model: Option<String>,
    /// Thinking level (off, minimal, low, medium, high, xhigh, max)
    #[arg(long, value_name = "LEVEL")]
    pub reasoning: Option<String>,
    /// Rebuild the graph from scratch
    #[arg(long)]
    pub fresh: bool,
    /// Persist the answer to kg-chats/
    #[arg(long)]
    pub save: bool,
    /// Stream synthesis deltas to stderr
    #[arg(long)]
    pub stream: bool,
    /// Prefix the output with the [[hermes:final]] sentinel
    #[arg(long)]
    pub hermes_final: bool,
    /// Convert GFM tables to bullet lines in slack format
    #[arg(long)]
    pub plain_tables: bool,
    /// Omit the Sources section in slack format
    #[arg(long)]
    pub no_sources: bool,
    /// Quote exact lines under each source in slack format
    #[arg(long)]
    pub quote_sources: bool,
    /// Bound the rendered output length
    #[arg(long, value_name = "N")]
    pub max_chars: Option<usize>,
}

#[derive(Debug, Args)]
pub struct ScanArgs {
    /// PDF or image file
    pub file: PathBuf,
    /// Parallel region OCR requests
    #[arg(long, value_name = "N")]
    pub parallel: Option<usize>,
    /// OCR model key
    #[arg(long, value_name = "KEY")]
    pub text_model: Option<String>,
    /// Figure/table analysis model key (defaults to --text_model)
    #[arg(long, value_name = "KEY")]
    pub figure_model: Option<String>,
    /// Destination folder under resources/
    #[arg(long, value_name = "DIR")]
    pub output: Option<PathBuf>,
    /// 1-based page numbers, comma separated
    #[arg(long, value_name = "PAGES", value_delimiter = ',')]
    pub pages: Option<Vec<u32>>,
    /// Override the region OCR prompt
    #[arg(long)]
    pub prompt: Option<String>,
    /// Override the figure/table analysis prompt
    #[arg(long)]
    pub analysis_prompt: Option<String>,
    /// Disable figure/table analysis
    #[arg(long)]
    pub no_figures: bool,
}

#[derive(Debug, Args)]
#[command(group = ArgGroup::new("action").required(true).multiple(false).args(["list", "read", "outline", "search"]))]
pub struct ResourcesArgs {
    /// List the resource tree (one level by default)
    #[arg(long)]
    pub list: bool,
    /// With --list: recurse into folders
    #[arg(long)]
    pub tree: bool,
    /// With --read: print ready-to-paste figure markdown lines
    #[arg(long)]
    pub figures: bool,
    /// Print a file's text
    #[arg(long, value_name = "REL")]
    pub read: Option<String>,
    /// First line to read (1-based)
    #[arg(long, value_name = "N")]
    pub offset: Option<usize>,
    /// Maximum lines to read
    #[arg(long, value_name = "N")]
    pub limit: Option<usize>,
    /// Prefix each line with its number
    #[arg(long)]
    pub numbered: bool,
    /// Print headings with line numbers and figure/table counts
    #[arg(long, value_name = "REL")]
    pub outline: Option<String>,
    /// Search file lines
    #[arg(long, value_name = "QUERY")]
    pub search: Option<String>,
    /// Context lines around a search hit
    #[arg(long, value_name = "N")]
    pub context: Option<usize>,
}

#[derive(Debug, Args)]
#[command(group = ArgGroup::new("action").required(true).multiple(false).args(["list", "read"]))]
pub struct NotesArgs {
    /// List notes
    #[arg(long)]
    pub list: bool,
    /// Only notes for this resource rel path
    #[arg(long, value_name = "REL")]
    pub resource: Option<String>,
    /// Print one note by id
    #[arg(long, value_name = "ID")]
    pub read: Option<String>,
}

#[derive(Debug, Args)]
pub struct BackupArgs {
    /// Output zip path (default: docfoo-backup-<timestamp>.zip)
    #[arg(long, value_name = "FILE")]
    pub out: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct RestoreArgs {
    /// Backup zip to restore
    pub file: PathBuf,
    /// Skip the confirmation prompt
    #[arg(long)]
    pub yes: bool,
}

#[derive(Debug, Args)]
#[command(group = ArgGroup::new("action").required(true).multiple(false).args(["list", "info", "download"]))]
pub struct CollectionsArgs {
    /// List community collections
    #[arg(long)]
    pub list: bool,
    /// Show one collection's details
    #[arg(long, value_name = "ID")]
    pub info: Option<String>,
    /// Download and install a collection
    #[arg(long, value_name = "ID|NAME")]
    pub download: Option<String>,
    /// Collection type to install
    #[arg(long = "type", value_name = "TYPE", value_enum)]
    pub kind: Option<CollectionKind>,
    /// Override the installed name
    #[arg(long, value_name = "NAME")]
    pub name: Option<String>,
    /// Overwrite an existing install
    #[arg(long)]
    pub force: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum CollectionKind {
    Resource,
    Kg,
}

#[derive(Debug, Args)]
#[command(group = ArgGroup::new("action").required(true).multiple(false).args(["get", "set", "list"]))]
pub struct ModelArgs {
    /// Print a model slot (default: kg)
    #[arg(long, value_name = "SLOT", num_args = 0..=1, default_missing_value = "kg")]
    pub get: Option<String>,
    /// Set a model slot: --set SLOT provider/model
    #[arg(long, value_names = ["SLOT", "KEY"], num_args = 2)]
    pub set: Option<Vec<String>>,
    /// List providers and models
    #[arg(long)]
    pub list: bool,
    /// With --list: only this provider
    #[arg(long, value_name = "PROVIDER")]
    pub provider: Option<String>,
}

#[derive(Debug, Args)]
#[command(group = ArgGroup::new("action").required(true).multiple(false).args(["status", "set", "logout"]))]
pub struct AuthArgs {
    /// Show provider auth status
    #[arg(long)]
    pub status: bool,
    /// With --status: only this provider
    #[arg(long, value_name = "PROVIDER")]
    pub provider: Option<String>,
    /// Set an API key: --set PROVIDER [--key KEY]
    #[arg(long, value_name = "PROVIDER")]
    pub set: Option<String>,
    /// API key value (prompted on a TTY when omitted)
    #[arg(long, value_name = "KEY")]
    pub key: Option<String>,
    /// Remove a stored credential
    #[arg(long, value_name = "PROVIDER")]
    pub logout: Option<String>,
}

#[derive(Debug, Args)]
pub struct SetupArgs {
    /// Only check what is missing
    #[arg(long)]
    pub check: bool,
    /// Copy the layout model from a local DocFoo models directory
    #[arg(long, value_name = "DIR")]
    pub from: Option<PathBuf>,
    /// Re-download/re-copy even when present
    #[arg(long)]
    pub force: bool,
}

#[derive(Debug, Args)]
pub struct VersionArgs {
    /// Include paths and platform details
    #[arg(long)]
    pub verbose: bool,
}

#[derive(Debug, Args)]
pub struct UpdateArgs {
    /// Only check for a newer version
    #[arg(long)]
    pub check: bool,
}
