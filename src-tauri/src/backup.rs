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
use serde::{Deserialize, Serialize};
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
    /// Presets on disk the last backup doesn't cover — see
    /// `changed_since_backup`. None when there's no usable backup.
    pub changed_since_backup: Option<usize>,
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

/// Zip entry recording where every preset in a backup came from. Archives
/// made before it existed only carry `<root label>/<relative path>` entry
/// names, which `restore.rs` can still map back through the configured roots.
pub const MANIFEST_ENTRY: &str = "lineage-manifest.json";

/// Bump on an incompatible change to the manifest layout.
pub const MANIFEST_FORMAT: u32 = 1;

#[derive(Debug, Serialize, Deserialize)]
pub struct BackupManifest {
    pub format: u32,
    pub created_at: i64,
    pub entries: Vec<ManifestEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestEntry {
    /// The zip entry this describes.
    pub entry: String,
    /// Where the preset lived when it was backed up. Informational only:
    /// restore resolves through `root_id` + `rel_path` into a root that is
    /// configured *now*, and never writes to this path directly — so an
    /// archive, hand-made or not, can't aim a write anywhere else on disk.
    pub source_path: String,
    /// Root id first because it survives the user renaming a root's label.
    pub root_id: String,
    pub root_label: String,
    pub rel_path: String,
}

/// How many presets on disk the last backup does not cover: ones that aren't
/// in it at all, plus ones modified after it was taken. None when there is no
/// usable last backup — never made, moved, or unreadable.
///
/// Membership is checked against the archive's contents, not by timestamp:
/// MO2 keeps a file's original mtime when it installs a mod from an archive,
/// so a preset pack installed yesterday can look years older than the backup.
/// Timestamps only decide "modified since", for presets the backup does hold.
/// Presets deleted since the backup don't count — the backup still has them.
pub fn changed_since_backup(
    settings: &crate::settings::AppSettings,
    scan: &scan::ScanResult,
) -> Option<usize> {
    let archive = PathBuf::from(settings.last_backup_path.as_deref()?);
    if !archive.is_file() {
        return None;
    }
    let covered = crate::restore::archived_targets(settings, &archive).ok()?;
    let since = settings.last_backup_at.unwrap_or(0);
    Some(
        scan.files
            .iter()
            .filter(|f| !covered.contains(&f.path.to_ascii_lowercase()) || f.modified > since)
            .count(),
    )
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
            changed_since_backup: changed_since_backup(&settings, &scan),
        })
    })
    .await
    .map_err(|e| format!("backup status task failed: {e}"))?
}

/// Zip entry names must be unique and zip-safe: forward slashes, label
/// sanitized of the characters zip tooling chokes on.
pub(crate) fn sanitize_label(label: &str) -> String {
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
    let backed_up_at = chrono::Utc::now().timestamp();
    let mut manifest_entries: Vec<ManifestEntry> = Vec::with_capacity(scan.total);
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
            manifest_entries.push(ManifestEntry {
                entry: entry.clone(),
                source_path: f.path.clone(),
                root_id: f.root_id.clone(),
                root_label: f.root_label.clone(),
                rel_path: f.rel_path.clone(),
            });
            zip.start_file(entry, options)
                .map_err(|e| format!("backup archive error: {e}"))?;
            zip.write_all(&contents)
                .map_err(|e| format!("backup archive error: {e}"))?;
        }
        // Last, so a backup that fails partway never carries a manifest
        // describing presets it doesn't hold.
        let manifest = BackupManifest {
            format: MANIFEST_FORMAT,
            created_at: backed_up_at,
            entries: std::mem::take(&mut manifest_entries),
        };
        let json = serde_json::to_vec_pretty(&manifest)
            .map_err(|e| format!("backup manifest error: {e}"))?;
        zip.start_file(MANIFEST_ENTRY, options)
            .map_err(|e| format!("backup archive error: {e}"))?;
        zip.write_all(&json)
            .map_err(|e| format!("backup archive error: {e}"))?;
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
        backed_up_at,
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

    fn set_mtime(path: &std::path::Path, unix: i64) {
        let when = std::time::UNIX_EPOCH + std::time::Duration::from_secs(unix as u64);
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(when)
            .unwrap();
    }

    #[test]
    fn changed_since_backup_counts_new_and_modified_but_not_deleted() {
        use crate::settings::{AppSettings, JslotRoot, RootKind};
        let base = std::env::temp_dir().join(format!("lineage-nudge-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let presets = base.join("presets");
        std::fs::create_dir_all(&presets).unwrap();
        for name in ["a", "b", "c"] {
            std::fs::write(presets.join(format!("{name}.jslot")), name).unwrap();
        }
        let mut settings = AppSettings {
            jslot_roots: vec![JslotRoot {
                id: "p".into(),
                label: "Presets".into(),
                path: presets.display().to_string(),
                kind: RootKind::Plain,
            }],
            backup_dir: base.join("Backups").display().to_string(),
            ..Default::default()
        };
        assert_eq!(
            changed_since_backup(&settings, &scan::scan_settings(&settings)),
            None,
            "no backup yet"
        );

        let outcome = perform_backup(&settings, |_| {}).unwrap();
        settings.last_backup_path = Some(outcome.archive_path.clone());
        settings.last_backup_at = Some(outcome.backed_up_at);
        let since = outcome.backed_up_at;
        for name in ["a", "b", "c"] {
            set_mtime(&presets.join(format!("{name}.jslot")), since - 60);
        }
        assert_eq!(changed_since_backup(&settings, &scan::scan_settings(&settings)), Some(0));

        // Edited after the backup: counted.
        std::fs::write(presets.join("a.jslot"), "a2").unwrap();
        set_mtime(&presets.join("a.jslot"), since + 60);
        // Installed after the backup but carrying an old timestamp, the way
        // MO2 extracts a mod: counted, because the archive doesn't hold it.
        std::fs::write(presets.join("d.jslot"), "d").unwrap();
        set_mtime(&presets.join("d.jslot"), since - 100_000);
        // Deleted since: not counted — the backup still has it.
        std::fs::remove_file(presets.join("c.jslot")).unwrap();
        assert_eq!(changed_since_backup(&settings, &scan::scan_settings(&settings)), Some(2));

        // A recorded backup that's since been moved is no backup at all.
        std::fs::remove_file(&outcome.archive_path).unwrap();
        assert_eq!(changed_since_backup(&settings, &scan::scan_settings(&settings)), None);
        let _ = std::fs::remove_dir_all(&base);
    }
}
