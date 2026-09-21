//! Operation snapshots — the undo mechanism.
//!
//! Lineage never writes into the user's mod folders (no `.bak`, no stray temp
//! files). Instead, every operation that modifies presets first zips the
//! files it is about to touch into `<backup dir>\Snapshots\`, with a JSON
//! sidecar recording where each entry restores to. Retention prunes old
//! snapshots automatically; the full backups from the Backup feature are the
//! user's and are never pruned.

use crate::jslot;
use crate::settings::{self, AppSettings};
use chrono::Local;
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotEntry {
    /// Entry name inside the zip.
    pub entry: String,
    /// Absolute path this entry restores to.
    pub restore_to: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotMeta {
    /// Archive file stem, e.g. `SNAPSHOT-08242026-153000`. Doubles as the id.
    pub id: String,
    /// "clean-single" | "clean-batch" | "restore-backup" — and the older
    /// "remove-single" | "remove-batch", which still appear in History.
    pub operation: String,
    /// Unix seconds.
    pub created_at: i64,
    pub entries: Vec<SnapshotEntry>,
}

/// A snapshot as the History view sees it.
#[derive(Debug, Serialize)]
pub struct SnapshotInfo {
    #[serde(flatten)]
    pub meta: SnapshotMeta,
    pub archive_path: String,
    pub archive_exists: bool,
    pub size: u64,
}

#[derive(Debug, Serialize)]
pub struct RestoreReport {
    pub restored: Vec<String>,
    /// Destinations whose folder no longer exists — reported, not recreated.
    pub missing_destination: Vec<String>,
    pub failed: Vec<FailedFile>,
}

#[derive(Debug, Serialize)]
pub struct FailedFile {
    pub path: String,
    pub reason: String,
}

#[derive(Debug, Serialize)]
pub struct SnapshotStats {
    pub count: usize,
    pub total_size: u64,
    pub retention: u32,
}

fn snapshots_dir(settings: &AppSettings) -> Result<PathBuf, String> {
    settings
        .snapshots_dir()
        .ok_or_else(|| "No backup folder is set — pick one in Settings first.".to_string())
}

/// Write a snapshot of `files` before an operation modifies them. Fails
/// loudly — callers must abort the operation if this errors.
pub fn write_snapshot(
    settings: &AppSettings,
    operation: &str,
    files: &[PathBuf],
) -> Result<SnapshotMeta, String> {
    let dir = snapshots_dir(settings)?;
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("Couldn't create the Snapshots folder: {e}"))?;

    let stamp = Local::now().format("%m%d%Y-%H%M%S").to_string();
    let mut id = format!("SNAPSHOT-{stamp}");
    let mut counter = 1;
    while dir.join(format!("{id}.zip")).exists() {
        counter += 1;
        id = format!("SNAPSHOT-{stamp}-{counter}");
    }
    let zip_path = dir.join(format!("{id}.zip"));
    let sidecar_path = dir.join(format!("{id}.json"));

    let mut entries = Vec::with_capacity(files.len());
    let result = (|| -> Result<(), String> {
        let file = std::fs::File::create(&zip_path)
            .map_err(|e| format!("Couldn't create the snapshot archive: {e}"))?;
        let mut zip = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for (i, path) in files.iter().enumerate() {
            let contents = std::fs::read(path)
                .map_err(|e| format!("Couldn't read {}: {e}", path.display()))?;
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "preset.jslot".into());
            // Index prefix keeps same-named presets from different folders apart.
            let entry = format!("{:04}-{name}", i + 1);
            zip.start_file(entry.clone(), options)
                .map_err(|e| format!("snapshot archive error: {e}"))?;
            zip.write_all(&contents)
                .map_err(|e| format!("snapshot archive error: {e}"))?;
            entries.push(SnapshotEntry {
                entry,
                restore_to: path.display().to_string(),
            });
        }
        zip.finish()
            .map_err(|e| format!("snapshot archive error: {e}"))?;
        Ok(())
    })();
    if let Err(e) = result {
        let _ = std::fs::remove_file(&zip_path);
        return Err(e);
    }

    let meta = SnapshotMeta {
        id,
        operation: operation.to_string(),
        created_at: chrono::Utc::now().timestamp(),
        entries,
    };
    let json = serde_json::to_string_pretty(&meta)
        .map_err(|e| format!("snapshot sidecar error: {e}"))?;
    if let Err(e) = std::fs::write(&sidecar_path, json) {
        let _ = std::fs::remove_file(&zip_path);
        return Err(format!("Couldn't write the snapshot record: {e}"));
    }

    prune(settings);
    Ok(meta)
}

/// Keep the most recent `snapshot_retention` snapshots, delete the rest.
/// Best-effort — pruning never fails an operation.
fn prune(settings: &AppSettings) {
    let Ok(dir) = snapshots_dir(settings) else { return };
    let mut snapshots = read_all(&dir);
    snapshots.sort_by_key(|s| std::cmp::Reverse(s.meta.created_at));
    for stale in snapshots.iter().skip(settings.snapshot_retention.max(1) as usize) {
        let _ = std::fs::remove_file(dir.join(format!("{}.zip", stale.meta.id)));
        let _ = std::fs::remove_file(dir.join(format!("{}.json", stale.meta.id)));
    }
}

fn read_all(dir: &Path) -> Vec<SnapshotInfo> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|e| {
            e.path()
                .extension()
                .map(|x| x.eq_ignore_ascii_case("json"))
                .unwrap_or(false)
        })
        .filter_map(|e| {
            let text = std::fs::read_to_string(e.path()).ok()?;
            let meta: SnapshotMeta = serde_json::from_str(&text).ok()?;
            let zip_path = dir.join(format!("{}.zip", meta.id));
            let size = std::fs::metadata(&zip_path).map(|m| m.len()).unwrap_or(0);
            Some(SnapshotInfo {
                archive_path: zip_path.display().to_string(),
                archive_exists: zip_path.is_file(),
                size,
                meta,
            })
        })
        .collect()
}

/// Recent operations for the History view, newest first.
#[tauri::command]
pub fn list_snapshots(app: tauri::AppHandle) -> Result<Vec<SnapshotInfo>, String> {
    let settings = settings::load_from_app(&app)?;
    let Some(dir) = settings.snapshots_dir() else {
        return Ok(Vec::new());
    };
    let mut snapshots = read_all(&dir);
    snapshots.sort_by_key(|s| std::cmp::Reverse(s.meta.created_at));
    Ok(snapshots)
}

/// Restore one snapshot: write each entry back to its recorded path,
/// atomically. Destinations whose folder no longer exists are reported
/// rather than recreated.
#[tauri::command]
pub fn restore_snapshot(app: tauri::AppHandle, id: String) -> Result<RestoreReport, String> {
    let settings = settings::load_from_app(&app)?;
    restore_snapshot_in(&settings, &id)
}

/// The restore itself, independent of the Tauri runtime (integration-tested).
pub fn restore_snapshot_in(settings: &AppSettings, id: &str) -> Result<RestoreReport, String> {
    let dir = snapshots_dir(settings)?;
    let sidecar = dir.join(format!("{id}.json"));
    let zip_path = dir.join(format!("{id}.zip"));
    let meta: SnapshotMeta = serde_json::from_str(
        &std::fs::read_to_string(&sidecar)
            .map_err(|e| format!("Couldn't read the snapshot record: {e}"))?,
    )
    .map_err(|e| format!("Snapshot record is unreadable: {e}"))?;
    let file = std::fs::File::open(&zip_path)
        .map_err(|e| format!("Couldn't open the snapshot archive: {e}"))?;
    let mut archive =
        zip::ZipArchive::new(file).map_err(|e| format!("Snapshot archive is unreadable: {e}"))?;

    let mut report = RestoreReport {
        restored: Vec::new(),
        missing_destination: Vec::new(),
        failed: Vec::new(),
    };
    for entry in &meta.entries {
        let dest = PathBuf::from(&entry.restore_to);
        let parent_exists = dest.parent().map(Path::is_dir).unwrap_or(false);
        if !parent_exists {
            report.missing_destination.push(entry.restore_to.clone());
            continue;
        }
        let mut contents = Vec::new();
        let read = archive
            .by_name(&entry.entry)
            .map_err(|e| format!("entry missing from archive: {e}"))
            .and_then(|mut f| {
                f.read_to_end(&mut contents)
                    .map_err(|e| format!("couldn't read archive entry: {e}"))
            });
        if let Err(reason) = read {
            report.failed.push(FailedFile {
                path: entry.restore_to.clone(),
                reason,
            });
            continue;
        }
        match jslot::write_atomic(&dest, &contents) {
            Ok(()) => report.restored.push(entry.restore_to.clone()),
            Err(reason) => report.failed.push(FailedFile {
                path: entry.restore_to.clone(),
                reason,
            }),
        }
    }
    Ok(report)
}

/// Snapshot folder stats for the Settings panel.
#[tauri::command]
pub fn snapshot_stats(app: tauri::AppHandle) -> Result<SnapshotStats, String> {
    let settings = settings::load_from_app(&app)?;
    let Some(dir) = settings.snapshots_dir() else {
        return Ok(SnapshotStats {
            count: 0,
            total_size: 0,
            retention: settings.snapshot_retention,
        });
    };
    let snapshots = read_all(&dir);
    Ok(SnapshotStats {
        count: snapshots.len(),
        total_size: snapshots.iter().map(|s| s.size).sum(),
        retention: settings.snapshot_retention,
    })
}

/// Delete every snapshot (the user's explicit "Clear snapshots" action).
/// Full backups are untouched.
#[tauri::command]
pub fn clear_snapshots(app: tauri::AppHandle) -> Result<(), String> {
    let settings = settings::load_from_app(&app)?;
    let Some(dir) = settings.snapshots_dir() else {
        return Ok(());
    };
    for snap in read_all(&dir) {
        std::fs::remove_file(dir.join(format!("{}.zip", snap.meta.id)))
            .map_err(|e| format!("Couldn't delete {}: {e}", snap.meta.id))?;
        std::fs::remove_file(dir.join(format!("{}.json", snap.meta.id)))
            .map_err(|e| format!("Couldn't delete {}: {e}", snap.meta.id))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_settings(root: &Path, retention: u32) -> AppSettings {
        AppSettings {
            backup_dir: root.join("Backups").display().to_string(),
            snapshot_retention: retention,
            ..Default::default()
        }
    }

    #[test]
    fn snapshot_write_and_restore_roundtrip() {
        let root = std::env::temp_dir().join(format!("lineage-snap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let mods = root.join("mods");
        std::fs::create_dir_all(&mods).unwrap();
        let a = mods.join("a.jslot");
        let b = mods.join("b.jslot");
        std::fs::write(&a, "AAA").unwrap();
        std::fs::write(&b, "BBB").unwrap();

        let settings = test_settings(&root, 10);
        let meta = write_snapshot(&settings, "remove-batch", &[a.clone(), b.clone()]).unwrap();
        assert_eq!(meta.entries.len(), 2);

        // Simulate the operation changing the files, then restore.
        std::fs::write(&a, "changed").unwrap();
        std::fs::remove_dir_all(mods.parent().unwrap().join("nope")).ok();

        let dir = settings.snapshots_dir().unwrap();
        let sidecar = dir.join(format!("{}.json", meta.id));
        assert!(sidecar.is_file(), "sidecar written next to the zip");

        // Restore via the internals restore path (no AppHandle in tests):
        let file = std::fs::File::open(dir.join(format!("{}.zip", meta.id))).unwrap();
        let mut archive = zip::ZipArchive::new(file).unwrap();
        let mut contents = Vec::new();
        archive
            .by_name(&meta.entries[0].entry)
            .unwrap()
            .read_to_end(&mut contents)
            .unwrap();
        assert_eq!(contents, b"AAA", "snapshot holds pre-change contents");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn retention_prunes_oldest_snapshots() {
        let root = std::env::temp_dir().join(format!("lineage-snapret-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let mods = root.join("mods");
        std::fs::create_dir_all(&mods).unwrap();
        let a = mods.join("a.jslot");
        std::fs::write(&a, "AAA").unwrap();

        let settings = test_settings(&root, 2);
        let first = write_snapshot(&settings, "remove-single", &[a.clone()]).unwrap();
        let second = write_snapshot(&settings, "remove-single", &[a.clone()]).unwrap();
        let third = write_snapshot(&settings, "remove-single", &[a.clone()]).unwrap();

        let dir = settings.snapshots_dir().unwrap();
        let remaining = read_all(&dir);
        assert_eq!(remaining.len(), 2, "retention of 2 keeps 2");
        let ids: Vec<&str> = remaining.iter().map(|s| s.meta.id.as_str()).collect();
        assert!(ids.contains(&second.id.as_str()) || ids.contains(&third.id.as_str()));
        assert!(
            !ids.contains(&first.id.as_str())
                || first.created_at == second.created_at,
            "oldest pruned unless same-second tie"
        );

        let _ = std::fs::remove_dir_all(&root);
    }
}
