//! Backup zip create/restore — port of `src-tauri/src/backup/zip.rs`.
//!
//! The archive format is unchanged (`docfoo-backup` v1), so a backup made by
//! the desktop app restores here and vice versa. Restore extracts into a
//! staging folder, snapshots the live data, swaps atomically, and keeps a
//! journal so a crash can be rolled back on the next run.

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use super::{BackupStats, BACKUP_FORMAT, BACKUP_VERSION};
use crate::util::{atomic_write_json, is_safe_rel, now_ms, report, Quota, QuotaError, QuotaState};

/// Zip-bomb guards for restore. A backup is user data, so these are generous;
/// they exist to stop a crafted or corrupt archive from exhausting the disk.
const RESTORE_QUOTA: Quota = Quota {
    max_entries: 500_000,
    max_entry_bytes: 4 * 1024 * 1024 * 1024,
    max_total_bytes: 16 * 1024 * 1024 * 1024,
    max_ratio: 1_000,
    ratio_min_bytes: 16 * 1024 * 1024,
};

fn restore_quota_error(kind: QuotaError) -> String {
    match kind {
        QuotaError::TooManyEntries => {
            "The backup contains too many files and was not restored.".to_string()
        }
        QuotaError::EntryTooLarge => {
            "The backup contains a file that is too large and was not restored.".to_string()
        }
        QuotaError::TotalTooLarge => {
            "The backup expands to more data than restore allows and was not restored.".to_string()
        }
        QuotaError::CompressionRatio => {
            "The backup looks like a zip bomb and was not restored.".to_string()
        }
    }
}

fn collect_files(dir: &Path, prefix: &str, out: &mut Vec<(String, PathBuf)>) {
    let Ok(read) = fs::read_dir(dir) else {
        return;
    };
    for entry in read.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        let rel = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}/{name}")
        };
        if file_type.is_dir() {
            collect_files(&path, &rel, out);
        } else if file_type.is_file() {
            out.push((rel, path));
        }
    }
}

pub fn create_backup_zip(
    workspace: &Path,
    agent_dir: &Path,
    out_path: &Path,
) -> std::result::Result<BackupStats, String> {
    if let Some(parent) = out_path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("could not create the backup folder: {e}"))?;
    }
    let file = fs::File::create(out_path)
        .map_err(|e| format!("could not create the backup file: {e}"))?;
    let mut zip = ::zip::ZipWriter::new(file);
    let options =
        ::zip::write::SimpleFileOptions::default().compression_method(::zip::CompressionMethod::Deflated);

    let resources_root = workspace.join("resources");
    let mut files: Vec<(String, PathBuf)> = Vec::new();
    if resources_root.is_dir() {
        collect_files(&resources_root, "", &mut files);
    }
    let sessions_root = agent_dir.join("sessions");
    if sessions_root.is_dir() {
        collect_files(&sessions_root, "sessions", &mut files);
    }
    let chats_root = workspace.join("chats");
    if chats_root.is_dir() {
        collect_files(&chats_root, "chats", &mut files);
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));

    let json_files: [(&str, PathBuf); 4] = [
        ("notes.json", workspace.join("notes.json")),
        ("read-state.json", workspace.join("read-state.json")),
        ("current-chat.json", agent_dir.join("current-chat.json")),
        ("chat-dirs.json", agent_dir.join("chat-dirs.json")),
    ];
    let stats = BackupStats {
        resources: files
            .iter()
            .filter(|(rel, _)| !rel.starts_with("sessions/") && !rel.starts_with("chats/"))
            .count(),
        notes: usize::from(json_files[0].1.is_file()),
        chat_dirs: usize::from(json_files[3].1.is_file()),
        sessions: files
            .iter()
            .filter(|(rel, _)| rel.starts_with("sessions/") && rel.ends_with(".jsonl"))
            .count(),
    };
    let read_state = usize::from(json_files[1].1.is_file());

    let add_file =
        |zip: &mut ::zip::ZipWriter<fs::File>, zip_name: &str, abs: &Path| -> Result<(), String> {
            let mut input = fs::File::open(abs)
                .map_err(|e| format!("could not read {}: {e}", abs.display()))?;
            zip.start_file(zip_name, options)
                .map_err(|e| format!("could not write the backup: {e}"))?;
            std::io::copy(&mut input, zip).map_err(|e| format!("could not write the backup: {e}"))?;
            Ok(())
        };

    let manifest = json!({
        "format": BACKUP_FORMAT,
        "version": BACKUP_VERSION,
        "createdAt": now_ms(),
        "platform": "desktop",
        "workspace": workspace.to_string_lossy(),
        "resources": stats.resources,
        "notes": stats.notes,
        "readState": read_state,
        "chatDirs": stats.chat_dirs,
        "sessions": stats.sessions,
    });
    let manifest_text = serde_json::to_string(&manifest).map_err(|e| e.to_string())?;
    zip.start_file("manifest.json", options)
        .map_err(|e| format!("could not write the backup: {e}"))?;
    zip.write_all(manifest_text.as_bytes())
        .map_err(|e| format!("could not write the backup: {e}"))?;

    for (rel, abs) in &files {
        if rel.starts_with("sessions/") || rel.starts_with("chats/") {
            add_file(&mut zip, rel, abs)?;
        } else {
            add_file(&mut zip, &format!("resources/{rel}"), abs)?;
        }
    }
    for (name, path) in json_files {
        if path.is_file() {
            add_file(&mut zip, name, &path)?;
        }
    }
    zip.finish()
        .map_err(|e| format!("could not finish the backup file: {e}"))?;
    Ok(stats)
}

fn safe_zip_rel(name: &str, prefix: &str) -> Option<String> {
    let normalized = name.replace('\\', "/");
    let rel = normalized.strip_prefix(&format!("{prefix}/"))?;
    is_safe_rel(rel).then(|| rel.to_string())
}

fn norm_root(path: &str) -> String {
    path.trim_end_matches(['/', '\\']).to_string()
}

fn rewrite_paths(value: &mut Value, from: &str, to: &str) {
    let from_fwd: String = from.replace('\\', "/");
    let rebase = |text: &mut String| {
        if let Some(rest) = text.strip_prefix(from) {
            *text = format!("{to}{rest}");
        } else if from_fwd != from {
            if let Some(rest) = text.strip_prefix(from_fwd.as_str()) {
                *text = format!("{to}{rest}");
            }
        }
    };
    match value {
        Value::String(text) => rebase(text),
        Value::Array(array) => {
            for item in array.iter_mut() {
                rewrite_paths(item, from, to);
            }
        }
        Value::Object(object) => {
            let keys: Vec<String> = object.keys().cloned().collect();
            for key in keys {
                if let Some(mut val) = object.remove(&key) {
                    rewrite_paths(&mut val, from, to);
                    let mut new_key = key;
                    rebase(&mut new_key);
                    object.insert(new_key, val);
                }
            }
        }
        _ => {}
    }
}

fn rewrite_session_file(path: &Path, from: &str, to: &str) -> Result<(), String> {
    let text = fs::read_to_string(path)
        .map_err(|e| format!("could not read {}: {e}", path.display()))?;
    let mut out = String::with_capacity(text.len());
    let mut changed = false;
    for line in text.lines() {
        if line.trim().is_empty() {
            out.push_str(line);
            out.push('\n');
            continue;
        }
        match serde_json::from_str::<Value>(line) {
            Ok(mut value) => {
                let before = serde_json::to_string(&value).unwrap_or_default();
                rewrite_paths(&mut value, from, to);
                let after = serde_json::to_string(&value).map_err(|e| e.to_string())?;
                if after != before {
                    changed = true;
                }
                out.push_str(&after);
            }
            Err(_) => out.push_str(line),
        }
        out.push('\n');
    }
    if changed {
        fs::write(path, out).map_err(|e| format!("could not write {}: {e}", path.display()))?;
    }
    Ok(())
}

fn rewrite_meta_file(path: &Path, from: &str, to: &str) -> Result<(), String> {
    let text = fs::read_to_string(path)
        .map_err(|e| format!("could not read {}: {e}", path.display()))?;
    let mut value: Value =
        serde_json::from_str(&text).map_err(|e| format!("{} is corrupt: {e}", path.display()))?;
    rewrite_paths(&mut value, from, to);
    fs::write(
        path,
        serde_json::to_string_pretty(&value).map_err(|e| e.to_string())?,
    )
    .map_err(|e| format!("could not write {}: {e}", path.display()))
}

#[derive(serde::Serialize, serde::Deserialize)]
struct RestoreJournal {
    phase: String,
    stage: String,
    snapshot: String,
    /// Kept for compatibility with older app backups that carried
    /// calendar.json; the CLI never writes it.
    #[serde(default)]
    has_calendar: bool,
}

const RESTORE_DIRS: [&str; 3] = ["resources", "sessions", "chats"];
const RESTORE_JSON: [&str; 4] = [
    "notes.json",
    "read-state.json",
    "current-chat.json",
    "chat-dirs.json",
];

fn journal_path(workspace: &Path) -> PathBuf {
    workspace.join("tmp").join("restore-journal.json")
}

fn write_journal(workspace: &Path, journal: &RestoreJournal) -> Result<(), String> {
    let path = journal_path(workspace);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("could not prepare the restore: {e}"))?;
    }
    atomic_write_json(&path, &serde_json::to_value(journal).map_err(|e| e.to_string())?)
        .map_err(|e| format!("could not prepare the restore: {e}"))
}

fn remove_journal(workspace: &Path) {
    let _ = fs::remove_file(journal_path(workspace));
}

fn copy_entry(entry: &mut impl Read, dest: &Path) -> Result<(), String> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("could not create a folder: {e}"))?;
    }
    let mut out =
        fs::File::create(dest).map_err(|e| format!("could not write {}: {e}", dest.display()))?;
    std::io::copy(entry, &mut out).map_err(|e| format!("could not write {}: {e}", dest.display()))?;
    Ok(())
}

/// Extract every usable entry into `stage` (never touches live data) and
/// report what a successful restore will produce.
fn extract_staged(
    archive: &mut ::zip::ZipArchive<fs::File>,
    stage: &Path,
    rewrite: &Option<(String, String)>,
) -> Result<BackupStats, String> {
    let mut extracted = BackupStats::default();
    let mut quota = QuotaState::default();
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|e| format!("the backup is corrupt: {e}"))?;
        if entry.is_dir() {
            continue;
        }
        let raw_name = entry.name().to_string();
        if raw_name == "manifest.json" {
            continue;
        }
        if let Err(kind) = RESTORE_QUOTA.check(&mut quota, entry.size(), entry.compressed_size()) {
            return Err(restore_quota_error(kind));
        }
        let normalized = raw_name.replace('\\', "/");
        if let Some(rel) = safe_zip_rel(&raw_name, "resources") {
            copy_entry(
                &mut entry,
                &stage
                    .join("resources")
                    .join(rel.replace('/', std::path::MAIN_SEPARATOR_STR)),
            )?;
            extracted.resources += 1;
        } else if let Some(rel) = safe_zip_rel(&raw_name, "sessions") {
            let target_rel = rel.strip_prefix(".trash/").unwrap_or(&rel);
            let dest = stage
                .join("sessions")
                .join(target_rel.replace('/', std::path::MAIN_SEPARATOR_STR));
            copy_entry(&mut entry, &dest)?;
            if rel.ends_with(".jsonl") {
                extracted.sessions += 1;
                if let Some((from, to)) = rewrite {
                    rewrite_session_file(&dest, from, to)?;
                }
            }
        } else if let Some(rel) = safe_zip_rel(&raw_name, "chats") {
            copy_entry(
                &mut entry,
                &stage
                    .join("chats")
                    .join(rel.replace('/', std::path::MAIN_SEPARATOR_STR)),
            )?;
        } else if normalized == "notes.json" {
            copy_entry(&mut entry, &stage.join("notes.json"))?;
            extracted.notes += 1;
        } else if normalized == "read-state.json" {
            copy_entry(&mut entry, &stage.join("read-state.json"))?;
        } else if normalized == "current-chat.json" || normalized == "chat-dirs.json" {
            let dest = stage.join(&normalized);
            copy_entry(&mut entry, &dest)?;
            if let Some((from, to)) = rewrite {
                rewrite_meta_file(&dest, from, to)?;
            }
            if normalized == "chat-dirs.json" {
                extracted.chat_dirs += 1;
            }
        } else if normalized.starts_with("resources/")
            || normalized.starts_with("sessions/")
            || normalized.starts_with("chats/")
        {
            return Err(
                "The backup contains an unsafe file path and was not restored.".to_string(),
            );
        }
        // Unknown top-level names are ignored (forward compatibility).
    }
    Ok(extracted)
}

fn live_dir(workspace: &Path, agent_dir: &Path, name: &str) -> PathBuf {
    match name {
        "sessions" => agent_dir.join(name),
        _ => workspace.join(name),
    }
}

fn live_json(workspace: &Path, agent_dir: &Path, name: &str) -> PathBuf {
    match name {
        "current-chat.json" | "chat-dirs.json" => agent_dir.join(name),
        _ => workspace.join(name),
    }
}

fn move_path(from: &Path, to: &Path) -> Result<(), String> {
    if !from.exists() {
        return Ok(());
    }
    if to.exists() {
        if to.is_dir() {
            fs::remove_dir_all(to).map_err(|e| format!("could not replace {}: {e}", to.display()))?;
        } else {
            fs::remove_file(to).map_err(|e| format!("could not replace {}: {e}", to.display()))?;
        }
    }
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("could not create a folder: {e}"))?;
    }
    fs::rename(from, to).map_err(|e| format!("could not move {}: {e}", from.display()))
}

fn snapshot_live(
    workspace: &Path,
    agent_dir: &Path,
    snapshot: &Path,
    has_calendar: bool,
) -> Result<(), String> {
    fs::create_dir_all(snapshot).map_err(|e| format!("could not prepare the restore: {e}"))?;
    for name in RESTORE_DIRS {
        move_path(&live_dir(workspace, agent_dir, name), &snapshot.join(name))?;
    }
    for name in RESTORE_JSON {
        move_path(&live_json(workspace, agent_dir, name), &snapshot.join(name))?;
    }
    if has_calendar {
        move_path(&workspace.join("calendar.json"), &snapshot.join("calendar.json"))?;
    }
    Ok(())
}

fn swap_in(stage: &Path, workspace: &Path, agent_dir: &Path, has_calendar: bool) -> Result<(), String> {
    for name in RESTORE_DIRS {
        let staged = stage.join(name);
        let live = live_dir(workspace, agent_dir, name);
        if staged.exists() {
            move_path(&staged, &live)?;
        } else {
            fs::create_dir_all(&live)
                .map_err(|e| format!("could not create {}: {e}", live.display()))?;
        }
    }
    for name in RESTORE_JSON {
        let staged = stage.join(name);
        if staged.exists() {
            move_path(&staged, &live_json(workspace, agent_dir, name))?;
        }
    }
    if has_calendar {
        let staged = stage.join("calendar.json");
        if staged.exists() {
            move_path(&staged, &workspace.join("calendar.json"))?;
        }
    }
    Ok(())
}

fn rollback_item(snapshot_item: &Path, live: &Path, remove_live_when_missing: bool) -> Result<(), String> {
    if snapshot_item.exists() {
        if live.exists() {
            if live.is_dir() {
                fs::remove_dir_all(live).map_err(|e| format!("could not undo the restore: {e}"))?;
            } else {
                fs::remove_file(live).map_err(|e| format!("could not undo the restore: {e}"))?;
            }
        }
        if let Some(parent) = live.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("could not undo the restore: {e}"))?;
        }
        fs::rename(snapshot_item, live).map_err(|e| format!("could not undo the restore: {e}"))?;
    } else if remove_live_when_missing && live.exists() {
        if live.is_dir() {
            fs::remove_dir_all(live).map_err(|e| format!("could not undo the restore: {e}"))?;
        } else {
            fs::remove_file(live).map_err(|e| format!("could not undo the restore: {e}"))?;
        }
    }
    Ok(())
}

fn rollback_restore(
    workspace: &Path,
    agent_dir: &Path,
    snapshot: &Path,
    remove_created: bool,
    has_calendar: bool,
) -> Result<(), String> {
    for name in RESTORE_DIRS {
        rollback_item(
            &snapshot.join(name),
            &live_dir(workspace, agent_dir, name),
            remove_created,
        )?;
    }
    for name in RESTORE_JSON {
        rollback_item(
            &snapshot.join(name),
            &live_json(workspace, agent_dir, name),
            remove_created,
        )?;
    }
    if has_calendar {
        rollback_item(
            &snapshot.join("calendar.json"),
            &workspace.join("calendar.json"),
            remove_created,
        )?;
    }
    Ok(())
}

/// Finish or undo a restore that was interrupted (crash / power loss).
/// Safe to call on every startup.
pub fn recover_interrupted_restore(workspace: &Path, agent_dir: &Path) {
    let path = journal_path(workspace);
    let Ok(text) = fs::read_to_string(&path) else {
        return;
    };
    let Ok(journal) = serde_json::from_str::<RestoreJournal>(&text) else {
        let _ = fs::remove_file(&path);
        return;
    };
    let stage = PathBuf::from(&journal.stage);
    let snapshot = PathBuf::from(&journal.snapshot);
    // A "swapping" journal whose stage no longer holds anything means the swap
    // finished and only cleanup was interrupted. Rolling back here would undo
    // a restore that actually completed.
    let swap_finished = journal.phase == "swapping" && stage_is_empty(&stage);
    if journal.phase == "done" || swap_finished {
        let _ = fs::remove_dir_all(&stage);
        let _ = fs::remove_dir_all(&snapshot);
    } else {
        match rollback_restore(
            workspace,
            agent_dir,
            &snapshot,
            journal.phase == "swapping",
            journal.has_calendar,
        ) {
            Ok(()) => {
                let _ = fs::remove_dir_all(&snapshot);
            }
            Err(error) => {
                // Keep the snapshot: it is the only copy of the pre-restore data.
                report("Could not roll back an interrupted restore", error);
            }
        }
        let _ = fs::remove_dir_all(&stage);
    }
    let _ = fs::remove_file(&path);
}

/// True when the restore staging folder is missing or has no entries left.
fn stage_is_empty(stage: &Path) -> bool {
    match fs::read_dir(stage) {
        Ok(mut entries) => entries.next().is_none(),
        Err(_) => true,
    }
}

pub fn restore_from_zip(
    zip_path: &Path,
    workspace: &Path,
    agent_dir: &Path,
) -> Result<BackupStats, String> {
    // A previous interrupted restore must be settled before a new one starts.
    recover_interrupted_restore(workspace, agent_dir);

    let file =
        fs::File::open(zip_path).map_err(|e| format!("could not open the backup file: {e}"))?;
    let mut archive =
        ::zip::ZipArchive::new(file).map_err(|e| format!("this is not a valid zip file: {e}"))?;
    let mut manifest_text = String::new();
    {
        let mut manifest_entry = archive.by_name("manifest.json").map_err(|_| {
            "This file is not a DocFoo backup (no manifest inside).".to_string()
        })?;
        manifest_entry
            .read_to_string(&mut manifest_text)
            .map_err(|e| format!("the backup manifest could not be read: {e}"))?;
    }
    let manifest: Value = serde_json::from_str(&manifest_text)
        .map_err(|_| "the backup manifest is corrupt.".to_string())?;
    if manifest["format"].as_str() != Some(BACKUP_FORMAT)
        || manifest["version"].as_u64() != Some(BACKUP_VERSION as u64)
    {
        return Err("This backup was made by an incompatible version of DocFoo — restoring it could corrupt your data. Nothing was changed.".to_string());
    }
    let rewrite: Option<(String, String)> = match manifest["workspace"].as_str() {
        Some(from) if norm_root(from) != norm_root(&workspace.to_string_lossy()) => {
            Some((norm_root(from), norm_root(&workspace.to_string_lossy())))
        }
        _ => None,
    };

    let tmp_root = workspace.join("tmp");
    fs::create_dir_all(&tmp_root).map_err(|e| format!("could not prepare the restore: {e}"))?;
    let stamp = format!("{}-{}", now_ms(), std::process::id());
    let stage = tmp_root.join(format!("restore-{stamp}"));
    let snapshot = tmp_root.join(format!("pre-restore-{stamp}"));
    let mut journal = RestoreJournal {
        phase: "extracting".to_string(),
        stage: stage.to_string_lossy().to_string(),
        snapshot: snapshot.to_string_lossy().to_string(),
        has_calendar: false,
    };
    write_journal(workspace, &journal)?;

    // Stage 1: extract + validate everything without touching live data.
    let extracted = match extract_staged(&mut archive, &stage, &rewrite) {
        Ok(stats) => stats,
        Err(error) => {
            let _ = fs::remove_dir_all(&stage);
            remove_journal(workspace);
            return Err(format!("{error} Nothing was changed."));
        }
    };

    // Stage 2: snapshot the live data (renames — cheap, same volume).
    journal.phase = "snapshot".to_string();
    if let Err(error) = write_journal(workspace, &journal) {
        let _ = fs::remove_dir_all(&stage);
        remove_journal(workspace);
        return Err(error);
    }
    if let Err(error) = snapshot_live(workspace, agent_dir, &snapshot, journal.has_calendar) {
        match rollback_restore(workspace, agent_dir, &snapshot, false, journal.has_calendar) {
            Ok(()) => {
                let _ = fs::remove_dir_all(&snapshot);
            }
            Err(rollback) => report("Could not roll back the failed restore", rollback),
        }
        let _ = fs::remove_dir_all(&stage);
        remove_journal(workspace);
        return Err(format!("{error} Nothing was changed."));
    }

    // Stage 3: swap the staged data in.
    journal.phase = "swapping".to_string();
    if let Err(error) = write_journal(workspace, &journal) {
        match rollback_restore(workspace, agent_dir, &snapshot, false, journal.has_calendar) {
            Ok(()) => {
                let _ = fs::remove_dir_all(&snapshot);
            }
            Err(rollback) => report("Could not roll back the failed restore", rollback),
        }
        let _ = fs::remove_dir_all(&stage);
        remove_journal(workspace);
        return Err(format!("{error} Nothing was changed."));
    }
    if let Err(error) = swap_in(&stage, workspace, agent_dir, journal.has_calendar) {
        match rollback_restore(workspace, agent_dir, &snapshot, true, journal.has_calendar) {
            Ok(()) => {
                let _ = fs::remove_dir_all(&snapshot);
            }
            Err(rollback) => report("Could not roll back the failed restore", rollback),
        }
        let _ = fs::remove_dir_all(&stage);
        remove_journal(workspace);
        return Err(format!("{error} Nothing was changed."));
    }

    // Success: mark done first (crash recovery then only cleans up), then GC.
    journal.phase = "done".to_string();
    if let Err(error) = write_journal(workspace, &journal) {
        report("Could not finalize the restore journal", error);
    }
    let _ = fs::remove_dir_all(&snapshot);
    let _ = fs::remove_dir_all(&stage);
    remove_journal(workspace);
    Ok(extracted)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recovery_keeps_a_completed_swap() {
        let root = std::env::temp_dir().join(format!("docfoo-recover-done-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let workspace = root.join("ws");
        let agent = root.join("agent");
        fs::create_dir_all(workspace.join("resources")).unwrap();
        fs::write(workspace.join("resources/kept.md"), b"restored").unwrap();
        let stage = workspace.join("tmp/restore-x");
        fs::create_dir_all(&stage).unwrap();
        let snapshot = workspace.join("tmp/pre-restore-x");
        fs::create_dir_all(snapshot.join("resources")).unwrap();
        fs::write(snapshot.join("resources/old.md"), b"old").unwrap();
        let journal = RestoreJournal {
            phase: "swapping".to_string(),
            stage: stage.to_string_lossy().to_string(),
            snapshot: snapshot.to_string_lossy().to_string(),
            has_calendar: false,
        };
        write_journal(&workspace, &journal).unwrap();
        recover_interrupted_restore(&workspace, &agent);
        assert!(
            workspace.join("resources/kept.md").is_file(),
            "a completed swap must not be rolled back"
        );
        assert!(!snapshot.exists());
        assert!(!stage.exists());
        assert!(!journal_path(&workspace).exists());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn recovery_rolls_back_an_unfinished_swap() {
        let root =
            std::env::temp_dir().join(format!("docfoo-recover-partial-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let workspace = root.join("ws");
        let agent = root.join("agent");
        fs::create_dir_all(workspace.join("resources")).unwrap();
        fs::write(workspace.join("resources/new.md"), b"new").unwrap();
        let stage = workspace.join("tmp/restore-x");
        fs::create_dir_all(stage.join("resources")).unwrap();
        fs::write(stage.join("resources/pending.md"), b"pending").unwrap();
        let snapshot = workspace.join("tmp/pre-restore-x");
        fs::create_dir_all(snapshot.join("resources")).unwrap();
        fs::write(snapshot.join("resources/old.md"), b"old").unwrap();
        let journal = RestoreJournal {
            phase: "swapping".to_string(),
            stage: stage.to_string_lossy().to_string(),
            snapshot: snapshot.to_string_lossy().to_string(),
            has_calendar: false,
        };
        write_journal(&workspace, &journal).unwrap();
        recover_interrupted_restore(&workspace, &agent);
        assert!(
            workspace.join("resources/old.md").is_file(),
            "a partial swap must be rolled back"
        );
        assert!(!workspace.join("resources/new.md").exists());
        assert!(!snapshot.exists());
        assert!(!stage.exists());
        assert!(!journal_path(&workspace).exists());
        let _ = fs::remove_dir_all(&root);
    }
}
