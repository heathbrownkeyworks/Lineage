//! Full JSLOT backups — the safety net every destructive feature depends on.
//!
//! One action zips every discovered preset into the configured backup
//! destination. Relative paths are preserved inside the archive, anchored to
//! each configured root and prefixed with that root's label, so a restored
//! preset can be traced back to the mod it came from. Existing archives are
//! never overwritten.

use crate::scan;
use crate::settings::{self};
use chrono::Local;
use serde::Serialize;
use std::io::Write;
use std::path::PathBuf;

#[derive(Debug, Serialize)]
pub struct BackupStatus {
    pub destination: String,
    pub file_count: usize,
    pub total_size: u64,
    pub last_backup_at: Option<i64>,
    pub last_backup_path: Option<String>,
    /// False when the recorded archive no longer exists on disk — the UI then
    /// treats the backup as missing.
    pub last_backup_exists: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct BackupProgress {
    pub current: usize,
    pub total: usize,
    pub name: String,
}

#[derive(Debug, Serialize)]
pub struct BackupOutcome {
    pub archive_path: String,
    pub file_count: usize,
    pub total_size: u64,
    pub backed_up_at: i64,
}

/// Everything the Backup screen shows. Scans the roots, so it runs on a
/// blocking worker.
#[tauri::command]
pub async fn get_backup_status(app: tauri::AppHandle) -> Result<BackupStatus, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let settings = settings::load_from_app(&app)?;
        let scan = scan::scan_settings(&settings);
        let last_backup_exists = settings
            .last_backup_path
            .as_deref()
            .map(|p| PathBuf::from(p).is_file())
            .unwrap_or(false);
        Ok(BackupStatus {
            destination: settings.backup_dir.clone(),
            file_count: scan.total,
            total_size: scan.files.iter().map(|f| f.size).sum(),
            last_backup_at: settings.last_backup_at,
            last_backup_path: settings.last_backup_path.clone(),
            last_backup_exists,
        })
    })
    .await
    .map_err(|e| format!("backup status task failed: {e}"))?
}

/// Zip entry names must be unique and zip-safe: forward slashes, label
/// sanitized of the characters zip tooling chokes on.
fn sanitize_label(label: &str) -> String {
    let cleaned: String = label
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c => c,
        })
        .collect();
    let trimmed = cleaned.trim().trim_matches('.').to_string();
    if trimmed.is_empty() { "root".into() } else { trimmed }
}

/// The backup itself, independent of the Tauri runtime so it's directly
/// integration-testable. Scans, zips, never overwrites an existing archive
/// (appends `-2`, `-3`, …). The caller persists `last_backup_*`.
pub fn perform_backup(
    settings: &crate::settings::AppSettings,
    mut progress: impl FnMut(BackupProgress),
) -> Result<BackupOutcome, String> {
    let dest = settings
        .backup_dir_path()
        .ok_or_else(|| "No backup folder is set — pick one in Settings first.".to_string())?;
    std::fs::create_dir_all(&dest)
        .map_err(|e| format!("Couldn't create the backup folder: {e}"))?;

    let scan = scan::scan_settings(settings);
    if scan.total == 0 {
        return Err(
            "No JSLOT files were found under the configured locations — nothing to back up. Check your JSLOT locations in Settings.".to_string(),
        );
    }

    // JSLOT-BACKUP-MMDDYYYY.zip, counter suffix instead of overwriting.
    let stamp = Local::now().format("%m%d%Y").to_string();
    let mut archive_path = dest.join(format!("JSLOT-BACKUP-{stamp}.zip"));
    let mut counter = 1;
    while archive_path.exists() {
        counter += 1;
        archive_path = dest.join(format!("JSLOT-BACKUP-{stamp}-{counter}.zip"));
    }

    let mut total_size = 0u64;
    let result = (|| -> Result<(), String> {
        let file = std::fs::File::create(&archive_path)
            .map_err(|e| format!("Couldn't create the backup archive: {e}"))?;
        let mut zip = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        let mut used_entries: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        for (i, f) in scan.files.iter().enumerate() {
            progress(BackupProgress {
                current: i + 1,
                total: scan.total,
                name: f.file_name.clone(),
            });
            let contents =
                std::fs::read(&f.path).map_err(|e| format!("Couldn't read {}: {e}", f.path))?;
            total_size += contents.len() as u64;
            let mut entry = format!(
                "{}/{}",
                sanitize_label(&f.root_label),
                f.rel_path.replace('\\', "/")
            );
            // Two roots with the same label + rel path: disambiguate.
            let mut n = 1;
            while !used_entries.insert(entry.to_ascii_lowercase()) {
                n += 1;
                entry = format!(
                    "{}-{n}/{}",
                    sanitize_label(&f.root_label),
                    f.rel_path.replace('\\', "/")
                );
            }
            zip.start_file(entry, options)
                .map_err(|e| format!("backup archive error: {e}"))?;
            zip.write_all(&contents)
                .map_err(|e| format!("backup archive error: {e}"))?;
        }
        zip.finish()
            .map_err(|e| format!("backup archive error: {e}"))?;
        Ok(())
    })();
    if let Err(e) = result {
        let _ = std::fs::remove_file(&archive_path);
        return Err(e);
    }

    Ok(BackupOutcome {
        archive_path: archive_path.display().to_string(),
        file_count: scan.total,
        total_size,
        backed_up_at: chrono::Utc::now().timestamp(),
    })
}

/// Run the full backup. Streams per-file progress and records
/// `last_backup_at` / `last_backup_path` on success.
#[tauri::command]
pub async fn run_backup(
    app: tauri::AppHandle,
    on_progress: tauri::ipc::Channel<BackupProgress>,
) -> Result<BackupOutcome, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let mut settings = settings::load_from_app(&app)?;
        let outcome = perform_backup(&settings, |p| {
            let _ = on_progress.send(p);
        })?;
        settings.last_backup_at = Some(outcome.backed_up_at);
        settings.last_backup_path = Some(outcome.archive_path.clone());
        settings::save_to_app(&app, &settings)?;
        Ok(outcome)
    })
    .await
    .map_err(|e| format!("backup task failed: {e}"))?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_sanitize_to_zip_safe_names() {
        assert_eq!(sanitize_label("MO2 mods (enabled)"), "MO2 mods (enabled)");
        assert_eq!(sanitize_label("Bad:Label*?"), "Bad_Label__");
        assert_eq!(sanitize_label("  . "), "root");
    }
}
