//! Clean Preset — single-file and batch operations.
//!
//! Both paths snapshot the exact files they are about to modify (§ operation
//! snapshots) before touching anything. Batch additionally enforces the
//! backup guard rail server-side: no recorded (and still existing) full
//! backup, no batch run — the frontend shows the friendly version of the
//! same rule. What gets removed, and what never does, lives in `clean.rs`.

use crate::clean::{self, CleanCategory, CleanDetail, PresetInspection};
use crate::scan;
use crate::settings::{self};
use crate::snapshot::{self, FailedFile};
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Inspect one preset for the single-file screen, with each category's
/// verbatim preview.
#[tauri::command]
pub async fn inspect_preset(path: String) -> Result<PresetInspection, String> {
    tauri::async_runtime::spawn_blocking(move || Ok(clean::inspect(Path::new(&path), true)))
        .await
        .map_err(|e| format!("inspect task failed: {e}"))?
}

#[derive(Debug, Serialize)]
pub struct SingleCleanOutcome {
    pub detail: CleanDetail,
    /// Snapshot to restore from for Undo.
    pub snapshot_id: String,
}

/// Clean one preset. Checks there's something to remove first, so a no-op
/// never lands in History; then snapshot, then an atomic write.
#[tauri::command]
pub async fn clean_preset(
    app: tauri::AppHandle,
    path: String,
    categories: Vec<CleanCategory>,
) -> Result<SingleCleanOutcome, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let settings = settings::load_from_app(&app)?;
        let file = PathBuf::from(&path);
        let inspection = clean::inspect(&file, false);
        if let Some(reason) = inspection.parse_error {
            return Err(reason);
        }
        if !inspection.findings.iter().any(|f| categories.contains(&f.category)) {
            return Err("Nothing in the chosen categories — the preset wasn't changed.".into());
        }
        let meta = snapshot::write_snapshot(&settings, "clean-single", &[file.clone()])?;
        let detail = clean::clean_in_place(&file, &categories)?
            .ok_or("Nothing in the chosen categories — the preset wasn't changed.")?;
        Ok(SingleCleanOutcome {
            detail,
            snapshot_id: meta.id,
        })
    })
    .await
    .map_err(|e| format!("clean task failed: {e}"))?
}

#[derive(Debug, Clone, Serialize)]
pub struct BatchItem {
    pub path: String,
    pub file_name: String,
    pub rel_path: String,
    pub root_label: String,
    /// Entries per category. The UI filters by the chosen categories live,
    /// so changing the choice never needs a rescan.
    pub counts: BTreeMap<CleanCategory, usize>,
}

#[derive(Debug, Serialize)]
pub struct BatchScanReport {
    /// Total JSLOT files found across all roots.
    pub total: usize,
    /// Files holding anything in any category — the batch candidates.
    pub candidates: Vec<BatchItem>,
    /// Files with nothing in any category.
    pub nothing_to_clean: usize,
    /// Files that could not be parsed, with reasons.
    pub failed: Vec<FailedFile>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BatchProgress {
    pub current: usize,
    pub total: usize,
    pub name: String,
    /// "scanning" | "cleaning"
    pub stage: String,
}

/// Scan every root and record what each preset holds, per category.
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
            candidates: Vec::new(),
            nothing_to_clean: 0,
            failed: Vec::new(),
        };
        for (i, f) in scan.files.iter().enumerate() {
            let _ = on_progress.send(BatchProgress {
                current: i + 1,
                total: scan.total,
                name: f.file_name.clone(),
                stage: "scanning".into(),
            });
            let inspection = clean::inspect(Path::new(&f.path), false);
            if let Some(reason) = inspection.parse_error {
                report.failed.push(FailedFile {
                    path: f.path.clone(),
                    reason,
                });
            } else if inspection.findings.is_empty() {
                report.nothing_to_clean += 1;
            } else {
                report.candidates.push(BatchItem {
                    path: f.path.clone(),
                    file_name: f.file_name.clone(),
                    rel_path: f.rel_path.clone(),
                    root_label: f.root_label.clone(),
                    counts: inspection
                        .findings
                        .iter()
                        .map(|finding| (finding.category, finding.count))
                        .collect(),
                });
            }
        }
        Ok(report)
    })
    .await
    .map_err(|e| format!("batch scan task failed: {e}"))?
}

#[derive(Debug, Serialize)]
pub struct BatchCleanReport {
    pub snapshot_id: String,
    pub cleaned: Vec<CleanDetail>,
    /// Selected files holding nothing in the chosen categories by the time
    /// we got to them (e.g. changed between scan and confirm).
    pub skipped: Vec<String>,
    pub failed: Vec<FailedFile>,
}

/// Clean the user's selected files.
///
/// Order of operations is deliberate: guard rail → snapshot (abort if it
/// cannot be written) → sequential cleaning that never aborts the whole run
/// because one file failed.
#[tauri::command]
pub async fn batch_clean(
    app: tauri::AppHandle,
    paths: Vec<String>,
    categories: Vec<CleanCategory>,
    on_progress: tauri::ipc::Channel<BatchProgress>,
) -> Result<BatchCleanReport, String> {
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
                "Back up your presets before running a batch clean — the recorded backup is missing. Use the Backup page first.".to_string(),
            );
        }
        if paths.is_empty() {
            return Err("No files selected — nothing to change.".to_string());
        }
        if categories.is_empty() {
            return Err("Nothing chosen to remove — pick at least one category.".to_string());
        }

        // Snapshot the exact selection. If this fails, nothing is modified.
        let files: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
        let meta = snapshot::write_snapshot(&settings, "clean-batch", &files)?;

        let mut report = BatchCleanReport {
            snapshot_id: meta.id,
            cleaned: Vec::new(),
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
                stage: "cleaning".into(),
            });
            match clean::clean_in_place(file, &categories) {
                Ok(Some(detail)) => report.cleaned.push(detail),
                Ok(None) => report.skipped.push(file.display().to_string()),
                Err(reason) => report.failed.push(FailedFile {
                    path: file.display().to_string(),
                    reason,
                }),
            }
        }
        Ok(report)
    })
    .await
    .map_err(|e| format!("batch clean task failed: {e}"))?
}
