//! Remove BodySlide — single-file and batch operations.
//!
//! Both paths snapshot the exact files they are about to modify (§ operation
//! snapshots) before touching anything. Batch additionally enforces the
//! backup guard rail server-side: no recorded (and still existing) full
//! backup, no batch run — the frontend shows the friendly version of the
//! same rule.

use crate::jslot::{self, PresetInspection, RemovalDetail};
use crate::scan;
use crate::settings::{self};
use crate::snapshot::{self, FailedFile};
use serde::Serialize;
use std::path::PathBuf;

/// Inspect one preset for the single-file Remove screen (includes the
/// section's verbatim JSON for the preview).
#[tauri::command]
pub async fn inspect_preset(path: String) -> Result<PresetInspection, String> {
    tauri::async_runtime::spawn_blocking(move || {
        Ok(jslot::inspect_path_full(std::path::Path::new(&path)))
    })
    .await
    .map_err(|e| format!("inspect task failed: {e}"))?
}

#[derive(Debug, Serialize)]
pub struct SingleRemoveOutcome {
    pub detail: RemovalDetail,
    /// Snapshot to restore from for Undo.
    pub snapshot_id: String,
}

/// Remove the body-morph section from one preset. Snapshot first, then an
/// atomic write. Never writes anything else into the preset's folder.
#[tauri::command]
pub async fn remove_body_morphs(
    app: tauri::AppHandle,
    path: String,
) -> Result<SingleRemoveOutcome, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let settings = settings::load_from_app(&app)?;
        let file = PathBuf::from(&path);
        let meta = snapshot::write_snapshot(&settings, "remove-single", &[file.clone()])?;
        let detail = jslot::remove_body_morphs_in_place(&file)?;
        Ok(SingleRemoveOutcome {
            detail,
            snapshot_id: meta.id,
        })
    })
    .await
    .map_err(|e| format!("remove task failed: {e}"))?
}

#[derive(Debug, Clone, Serialize)]
pub struct BatchItem {
    pub path: String,
    pub file_name: String,
    pub rel_path: String,
    pub root_label: String,
    pub morph_count: usize,
}

#[derive(Debug, Serialize)]
pub struct BatchScanReport {
    /// Total JSLOT files found across all roots.
    pub total: usize,
    /// Files containing a body-morph section — the batch candidates.
    pub with_section: Vec<BatchItem>,
    /// Files with no body-morph section (already clean).
    pub without_section: usize,
    /// Files that could not be parsed, with reasons.
    pub failed: Vec<FailedFile>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BatchProgress {
    pub current: usize,
    pub total: usize,
    pub name: String,
    /// "scanning" | "removing"
    pub stage: String,
}

/// Scan every root and classify each preset for the Batch Remove screen.
#[tauri::command]
pub async fn batch_scan(
    app: tauri::AppHandle,
    on_progress: tauri::ipc::Channel<BatchProgress>,
) -> Result<BatchScanReport, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let settings = settings::load_from_app(&app)?;
        let scan = scan::scan_settings(&settings);
        let mut report = BatchScanReport {
            total: scan.total,
            with_section: Vec::new(),
            without_section: 0,
            failed: Vec::new(),
        };
        for (i, f) in scan.files.iter().enumerate() {
            let _ = on_progress.send(BatchProgress {
                current: i + 1,
                total: scan.total,
                name: f.file_name.clone(),
                stage: "scanning".into(),
            });
            let inspection = jslot::inspect_path(std::path::Path::new(&f.path));
            if let Some(reason) = inspection.parse_error {
                report.failed.push(FailedFile {
                    path: f.path.clone(),
                    reason,
                });
            } else if inspection.section.is_some() {
                report.with_section.push(BatchItem {
                    path: f.path.clone(),
                    file_name: f.file_name.clone(),
                    rel_path: f.rel_path.clone(),
                    root_label: f.root_label.clone(),
                    morph_count: inspection.morph_count,
                });
            } else {
                report.without_section += 1;
            }
        }
        Ok(report)
    })
    .await
    .map_err(|e| format!("batch scan task failed: {e}"))?
}

#[derive(Debug, Serialize)]
pub struct BatchRemoveReport {
    pub snapshot_id: String,
    pub modified: Vec<RemovalDetail>,
    /// Selected files that turned out to have no section by the time we got
    /// to them (e.g. changed between scan and confirm).
    pub skipped: Vec<String>,
    pub failed: Vec<FailedFile>,
}

/// Run the batch removal over the user's selected files.
///
/// Order of operations is deliberate: guard rail → snapshot (abort if it
/// cannot be written) → sequential removal that never aborts the whole run
/// because one file failed.
#[tauri::command]
pub async fn batch_remove(
    app: tauri::AppHandle,
    paths: Vec<String>,
    on_progress: tauri::ipc::Channel<BatchProgress>,
) -> Result<BatchRemoveReport, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let settings = settings::load_from_app(&app)?;

        // Guard rail: a batch run requires a full backup that still exists.
        let backup_ok = settings.last_backup_at.is_some()
            && settings
                .last_backup_path
                .as_deref()
                .map(|p| PathBuf::from(p).is_file())
                .unwrap_or(false);
        if !backup_ok {
            return Err(
                "Back up your presets before running a batch removal — the recorded backup is missing. Use the Backup page first.".to_string(),
            );
        }
        if paths.is_empty() {
            return Err("No files selected — nothing to change.".to_string());
        }

        // Snapshot the exact selection. If this fails, nothing is modified.
        let files: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
        let meta = snapshot::write_snapshot(&settings, "remove-batch", &files)?;

        let mut report = BatchRemoveReport {
            snapshot_id: meta.id,
            modified: Vec::new(),
            skipped: Vec::new(),
            failed: Vec::new(),
        };
        let total = files.len();
        for (i, file) in files.iter().enumerate() {
            let _ = on_progress.send(BatchProgress {
                current: i + 1,
                total,
                name: file
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                stage: "removing".into(),
            });
            match jslot::remove_body_morphs_in_place(file) {
                Ok(detail) => report.modified.push(detail),
                Err(reason) if reason.contains("nothing to remove") => {
                    report.skipped.push(file.display().to_string());
                }
                Err(reason) => report.failed.push(FailedFile {
                    path: file.display().to_string(),
                    reason,
                }),
            }
        }
        Ok(report)
    })
    .await
    .map_err(|e| format!("batch remove task failed: {e}"))?
}
