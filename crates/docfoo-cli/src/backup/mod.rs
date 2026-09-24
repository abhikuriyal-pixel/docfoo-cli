//! Local backup and restore — the same `docfoo-backup` v1 zip format as the
//! desktop app, so backups move between the app and the CLI unchanged.

pub mod zip;

pub const BACKUP_FORMAT: &str = "docfoo-backup";
pub const BACKUP_VERSION: u32 = 1;
pub const BACKUP_FOLDER: &str = "DocFooBackup";
pub const BACKUP_FILE: &str = "docfoo-backup.zip";

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct BackupStats {
    #[serde(default)]
    pub resources: usize,
    #[serde(default)]
    pub notes: usize,
    #[serde(default)]
    pub chat_dirs: usize,
    #[serde(default)]
    pub sessions: usize,
}
