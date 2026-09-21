//! Backup browser and restore — the read side of `backup.rs`.
//!
//! Backups used to be write-only from the app: getting a preset back meant
//! opening the zip by hand. This lists the archives in the backup folder,
//! compares every preset in one against what is on disk now, and restores a
//! selection.
//!
//! Safety rules, all enforced here rather than trusted from the UI:
//! - Targets are resolved server-side and only ever inside a root that is
//!   configured *now*. The frontend sends entry names, never paths. A
//!   manifest's recorded absolute path is informational and never written to,
//!   so an archive — hand-made or not — can't aim a write anywhere else.
//! - Relative paths must be plain components: no `..`, no root, no drive
//!   prefix. This is the zip-slip defense.
//! - Nothing is overwritten without a snapshot first. If the snapshot can't
//!   be written the restore aborts before touching anything, and a completed
//!   restore is undoable from History like any other operation.
//! - A destination whose folder no longer exists is reported, not recreated,
//!   matching `snapshot::restore_snapshot_in`. Under an MO2 root that folder
//!   is a mod, and recreating it would conjure a mod MO2 never installed.

use crate::backup::{self, BackupManifest, BackupProgress, ManifestEntry, MANIFEST_ENTRY};
use crate::jslot;
use crate::settings::{self, AppSettings, JslotRoot, RootKind};
use crate::snapshot::{self, FailedFile};
use serde::Serialize;
use std::collections::HashMap;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone, Serialize)]
pub struct BackupArchive {
    pub path: String,
    pub file_name: String,
    pub size: u64,
    /// File mtime, unix seconds.
    pub modified: i64,
    /// When the backup was taken — only known for archives with a manifest.
    pub created_at: Option<i64>,
    pub preset_count: usize,
    pub has_manifest: bool,
    /// False when the file isn't a readable zip. Still listed, so a damaged
    /// backup is visible rather than silently missing.
    pub readable: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryStatus {
    /// On disk and byte-identical — nothing to restore.
    Unchanged,
    /// On disk but different, e.g. BodySlide data removed since the backup.
    Modified,
    /// Gone from disk, but its folder is still there to restore into.
    Missing,
    /// Its folder is gone too. Reported, never recreated.
    FolderGone,
    /// No configured root matches where it came from.
    Unmapped,
}

#[derive(Debug, Clone, Serialize)]
pub struct BackupEntry {
    /// The zip entry name — what the UI sends back to restore it.
    pub entry: String,
    pub file_name: String,
    /// For an MO2 root the mod folder, otherwise the root's label.
    pub group: String,
    pub rel_path: String,
    /// Where a restore would write it; None when unmapped.
    pub target: Option<String>,
    pub status: EntryStatus,
    pub size: u64,
}

#[derive(Debug, Serialize)]
pub struct BackupInspection {
    pub path: String,
    pub has_manifest: bool,
    pub created_at: Option<i64>,
    pub entries: Vec<BackupEntry>,
    /// Top-level archive folders no configured root answers to.
    pub unmapped_roots: Vec<String>,
}

#[derive(Debug, Default, Serialize)]
pub struct BackupRestoreOutcome {
    pub restored: Vec<String>,
    /// Asked for, but already identical on disk — skipped.
    pub unchanged: Vec<String>,
    pub folder_gone: Vec<String>,
    pub unmapped: Vec<String>,
    pub failed: Vec<FailedFile>,
    /// Snapshot of the files this restore overwrote, for Undo. None when it
    /// only recreated missing presets and overwrote nothing.
    pub snapshot_id: Option<String>,
}

fn is_jslot(name: &str) -> bool {
    name.to_ascii_lowercase().ends_with(".jslot")
}

fn read_manifest<R: std::io::Read + std::io::Seek>(
    archive: &mut zip::ZipArchive<R>,
) -> Option<BackupManifest> {
    let mut file = archive.by_name(MANIFEST_ENTRY).ok()?;
    let mut text = String::new();
    file.read_to_string(&mut text).ok()?;
    serde_json::from_str(&text).ok()
}

fn read_entry<R: std::io::Read + std::io::Seek>(
    archive: &mut zip::ZipArchive<R>,
    name: &str,
) -> Result<Vec<u8>, String> {
    let mut file = archive
        .by_name(name)
        .map_err(|e| format!("{name} is missing from the archive: {e}"))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|e| format!("Couldn't read {name} from the archive: {e}"))?;
    Ok(bytes)
}

/// Where one archive entry belongs: a configured root and the path under it.
struct Resolved<'a> {
    root: &'a JslotRoot,
    rel_path: String,
}

fn resolve<'a>(
    entry: &str,
    manifest: Option<&HashMap<&str, &ManifestEntry>>,
    roots: &'a [JslotRoot],
) -> Option<Resolved<'a>> {
    if let Some(m) = manifest.and_then(|m| m.get(entry)) {
        let root = roots
            .iter()
            .find(|r| r.id == m.root_id)
            .or_else(|| roots.iter().find(|r| r.label == m.root_label))?;
        return Some(Resolved {
            root,
            rel_path: m.rel_path.clone(),
        });
    }
    // An archive from before manifests: `<sanitized root label>/<rel path>`.
    let (top, rest) = entry.split_once('/')?;
    Some(Resolved {
        root: root_for_label(top, roots)?,
        rel_path: rest.replace('/', "\\"),
    })
}

/// The one configured root whose label sanitizes to an archive's top folder.
/// The backup writer appends `-2`, `-3`… only when two roots collide on the
/// same sanitized label, and which root got which suffix can't be recovered
/// from today's settings — so a collision is unmapped rather than a guess.
fn root_for_label<'a>(top: &str, roots: &'a [JslotRoot]) -> Option<&'a JslotRoot> {
    let mut matching = roots
        .iter()
        .filter(|r| backup::sanitize_label(&r.label) == top);
    let first = matching.next()?;
    matching.next().is_none().then_some(first)
}

/// `rel` joined under `base`, refusing anything that could step outside it.
fn safe_join(base: &str, rel: &str) -> Option<PathBuf> {
    let base = base.trim();
    if base.is_empty() || rel.trim().is_empty() {
        return None;
    }
    let mut out = PathBuf::from(base);
    for component in Path::new(rel).components() {
        match component {
            Component::Normal(part) => out.push(part),
            // `..`, `.`, a root or a drive prefix — never follow it.
            _ => return None,
        }
    }
    Some(out)
}

fn target_for(entry: &str, manifest: Option<&HashMap<&str, &ManifestEntry>>, roots: &[JslotRoot]) -> Option<(PathBuf, String, String)> {
    let resolved = resolve(entry, manifest, roots)?;
    let target = safe_join(&resolved.root.path, &resolved.rel_path)?;
    let group = match resolved.root.kind {
        RootKind::Mo2Mods => resolved
            .rel_path
            .split(['\\', '/'])
            .next()
            .unwrap_or_default()
            .to_string(),
        RootKind::Plain => resolved.root.label.clone(),
    };
    Some((target, group, resolved.rel_path))
}

fn classify(target: &Path, archived: &[u8]) -> EntryStatus {
    if target.is_file() {
        match std::fs::read(target) {
            Ok(current) if current == archived => EntryStatus::Unchanged,
            // Present but unreadable counts as modified: the restore will
            // try it and report the failure rather than skip it silently.
            _ => EntryStatus::Modified,
        }
    } else if target.parent().map(Path::is_dir).unwrap_or(false) {
        EntryStatus::Missing
    } else {
        EntryStatus::FolderGone
    }
}

fn open(path: &Path) -> Result<zip::ZipArchive<std::fs::File>, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("Couldn't open the backup: {e}"))?;
    zip::ZipArchive::new(file).map_err(|e| format!("This isn't a readable zip archive: {e}"))
}

fn manifest_index(manifest: &Option<BackupManifest>) -> Option<HashMap<&str, &ManifestEntry>> {
    manifest
        .as_ref()
        .map(|m| m.entries.iter().map(|e| (e.entry.as_str(), e)).collect())
}

/// Every `JSLOT-BACKUP-*.zip` in `dir`, newest first.
pub fn list_archives(dir: &Path) -> Vec<BackupArchive> {
    let Ok(read) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for item in read.flatten() {
        let path = item.path();
        let file_name = item.file_name().to_string_lossy().into_owned();
        let lower = file_name.to_ascii_lowercase();
        if !(lower.starts_with("jslot-backup-") && lower.ends_with(".zip")) || !path.is_file() {
            continue;
        }
        let meta = item.metadata().ok();
        let modified = meta
            .as_ref()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let (readable, preset_count, manifest) = match open(&path) {
            Ok(mut archive) => {
                let count = archive.file_names().filter(|n| is_jslot(n)).count();
                (true, count, read_manifest(&mut archive))
            }
            Err(_) => (false, 0, None),
        };
        out.push(BackupArchive {
            path: path.display().to_string(),
            file_name,
            size: meta.map(|m| m.len()).unwrap_or(0),
            modified,
            created_at: manifest.as_ref().map(|m| m.created_at),
            preset_count,
            has_manifest: manifest.is_some(),
            readable,
        });
    }
    out.sort_by_key(|a| std::cmp::Reverse(a.created_at.unwrap_or(a.modified)));
    out
}

/// Compare every preset in an archive with what is on disk now.
pub fn inspect(
    settings: &AppSettings,
    path: &Path,
    mut progress: impl FnMut(BackupProgress),
) -> Result<BackupInspection, String> {
    let mut archive = open(path)?;
    let manifest = read_manifest(&mut archive);
    let index = manifest_index(&manifest);
    let names: Vec<String> = archive
        .file_names()
        .filter(|n| is_jslot(n))
        .map(str::to_string)
        .collect();

    let mut entries = Vec::with_capacity(names.len());
    let mut unmapped_roots: Vec<String> = Vec::new();
    for (i, name) in names.iter().enumerate() {
        let file_name = name.rsplit('/').next().unwrap_or(name).to_string();
        progress(BackupProgress {
            current: i + 1,
            total: names.len(),
            name: file_name.clone(),
        });
        let archived = read_entry(&mut archive, name)?;
        let size = archived.len() as u64;
        let entry = match target_for(name, index.as_ref(), &settings.jslot_roots) {
            Some((target, group, rel_path)) => BackupEntry {
                entry: name.clone(),
                file_name,
                group,
                rel_path,
                status: classify(&target, &archived),
                target: Some(target.display().to_string()),
                size,
            },
            None => {
                let (top, rest) = name.split_once('/').unwrap_or(("", name));
                if !top.is_empty() && !unmapped_roots.iter().any(|r| r == top) {
                    unmapped_roots.push(top.to_string());
                }
                BackupEntry {
                    entry: name.clone(),
                    file_name,
                    group: top.to_string(),
                    rel_path: rest.replace('/', "\\"),
                    target: None,
                    status: EntryStatus::Unmapped,
                    size,
                }
            }
        };
        entries.push(entry);
    }
    Ok(BackupInspection {
        path: path.display().to_string(),
        has_manifest: manifest.is_some(),
        created_at: manifest.as_ref().map(|m| m.created_at),
        entries,
        unmapped_roots,
    })
}

/// Restore the named entries. Targets are re-resolved and re-classified here
/// — nothing the UI computed earlier is trusted, and the disk may have
/// changed since the archive was inspected.
pub fn restore(
    settings: &AppSettings,
    path: &Path,
    wanted: &[String],
    mut progress: impl FnMut(BackupProgress),
) -> Result<BackupRestoreOutcome, String> {
    let mut archive = open(path)?;
    let manifest = read_manifest(&mut archive);
    let index = manifest_index(&manifest);

    let mut outcome = BackupRestoreOutcome::default();
    let mut writes: Vec<(PathBuf, Vec<u8>)> = Vec::new();
    let mut overwrites: Vec<PathBuf> = Vec::new();
    for name in wanted {
        if !is_jslot(name) {
            outcome.failed.push(FailedFile {
                path: name.clone(),
                reason: "not a .jslot preset".into(),
            });
            continue;
        }
        let Some((target, _, _)) = target_for(name, index.as_ref(), &settings.jslot_roots) else {
            outcome.unmapped.push(name.clone());
            continue;
        };
        let archived = match read_entry(&mut archive, name) {
            Ok(bytes) => bytes,
            Err(reason) => {
                outcome.failed.push(FailedFile {
                    path: target.display().to_string(),
                    reason,
                });
                continue;
            }
        };
        match classify(&target, &archived) {
            EntryStatus::Unchanged => outcome.unchanged.push(target.display().to_string()),
            EntryStatus::FolderGone => outcome.folder_gone.push(target.display().to_string()),
            EntryStatus::Modified => {
                overwrites.push(target.clone());
                writes.push((target, archived));
            }
            EntryStatus::Missing => writes.push((target, archived)),
            EntryStatus::Unmapped => unreachable!("classify never reports unmapped"),
        }
    }

    if !overwrites.is_empty() {
        let meta = snapshot::write_snapshot(settings, "restore-backup", &overwrites).map_err(|e| {
            format!("Restore stopped before changing anything — couldn't snapshot the presets it would overwrite: {e}")
        })?;
        outcome.snapshot_id = Some(meta.id);
    }

    let total = writes.len();
    for (i, (target, bytes)) in writes.iter().enumerate() {
        progress(BackupProgress {
            current: i + 1,
            total,
            name: target
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
        });
        match jslot::write_atomic(target, bytes) {
            Ok(()) => outcome.restored.push(target.display().to_string()),
            Err(reason) => outcome.failed.push(FailedFile {
                path: target.display().to_string(),
                reason,
            }),
        }
    }
    Ok(outcome)
}

/// Archives in the configured backup folder, newest first.
#[tauri::command]
pub async fn list_backups(app: tauri::AppHandle) -> Result<Vec<BackupArchive>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let settings = settings::load_from_app(&app)?;
        Ok(settings
            .backup_dir_path()
            .map(|dir| list_archives(&dir))
            .unwrap_or_default())
    })
    .await
    .map_err(|e| format!("backup listing task failed: {e}"))?
}

#[tauri::command]
pub async fn inspect_backup(
    app: tauri::AppHandle,
    path: String,
    on_progress: tauri::ipc::Channel<BackupProgress>,
) -> Result<BackupInspection, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let settings = settings::load_from_app(&app)?;
        inspect(&settings, Path::new(&path), |p| {
            let _ = on_progress.send(p);
        })
    })
    .await
    .map_err(|e| format!("backup inspection task failed: {e}"))?
}

#[tauri::command]
pub async fn restore_backup(
    app: tauri::AppHandle,
    path: String,
    entries: Vec<String>,
    on_progress: tauri::ipc::Channel<BackupProgress>,
) -> Result<BackupRestoreOutcome, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let settings = settings::load_from_app(&app)?;
        restore(&settings, Path::new(&path), &entries, |p| {
            let _ = on_progress.send(p);
        })
    })
    .await
    .map_err(|e| format!("restore task failed: {e}"))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn temp_root(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lineage-restore-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn root(id: &str, label: &str, path: &Path, kind: RootKind) -> JslotRoot {
        JslotRoot {
            id: id.into(),
            label: label.into(),
            path: path.display().to_string(),
            kind,
        }
    }

    fn settings_for(base: &Path, roots: Vec<JslotRoot>) -> AppSettings {
        AppSettings {
            jslot_roots: roots,
            backup_dir: base.join("Backups").display().to_string(),
            snapshot_retention: 10,
            ..Default::default()
        }
    }

    fn write_zip(path: &Path, files: &[(&str, &[u8])]) {
        let file = std::fs::File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default();
        for (name, bytes) in files {
            zip.start_file(*name, options).unwrap();
            zip.write_all(bytes).unwrap();
        }
        zip.finish().unwrap();
    }

    fn status_of(inspection: &BackupInspection, file: &str) -> EntryStatus {
        inspection
            .entries
            .iter()
            .find(|e| e.file_name == file)
            .unwrap_or_else(|| panic!("no entry for {file}"))
            .status
    }

    #[test]
    fn safe_join_refuses_anything_that_leaves_the_root() {
        let base = r"D:\mods";
        assert_eq!(
            safe_join(base, r"ModA\Presets\x.jslot"),
            Some(PathBuf::from(r"D:\mods\ModA\Presets\x.jslot"))
        );
        for escape in [
            r"..\x.jslot",
            r"ModA\..\..\x.jslot",
            r".\x.jslot",
            r"\Windows\x.jslot",
            r"C:\x.jslot",
            "",
        ] {
            assert_eq!(safe_join(base, escape), None, "{escape:?} must be refused");
        }
        assert_eq!(safe_join("  ", "x.jslot"), None, "an empty root is never a target");
    }

    /// The one backup that exists on this machine predates manifests, so the
    /// label-based mapping is what actually has to work.
    #[test]
    fn legacy_archive_maps_through_the_root_label_and_classifies_each_preset() {
        let base = temp_root("legacy");
        let mods = base.join("mods");
        let presets = mods.join("ModA").join("Presets");
        std::fs::create_dir_all(&presets).unwrap();
        std::fs::write(presets.join("same.jslot"), b"SAME").unwrap();
        std::fs::write(presets.join("changed.jslot"), b"NEW").unwrap();
        // gone.jslot is absent but its folder exists; ModB doesn't exist at all.

        let archive = base.join("JSLOT-BACKUP-01012026.zip");
        write_zip(
            &archive,
            &[
                ("MO2 mods (enabled)/ModA/Presets/same.jslot", b"SAME"),
                ("MO2 mods (enabled)/ModA/Presets/changed.jslot", b"OLD"),
                ("MO2 mods (enabled)/ModA/Presets/gone.jslot", b"GONE"),
                ("MO2 mods (enabled)/ModB/Presets/orphan.jslot", b"ORPHAN"),
                ("Some Old Root/x.jslot", b"X"),
            ],
        );
        let settings = settings_for(
            &base,
            vec![root("mo2-mods", "MO2 mods (enabled)", &mods, RootKind::Mo2Mods)],
        );

        let inspection = inspect(&settings, &archive, |_| {}).unwrap();
        assert!(!inspection.has_manifest);
        assert_eq!(inspection.entries.len(), 5);
        assert_eq!(status_of(&inspection, "same.jslot"), EntryStatus::Unchanged);
        assert_eq!(status_of(&inspection, "changed.jslot"), EntryStatus::Modified);
        assert_eq!(status_of(&inspection, "gone.jslot"), EntryStatus::Missing);
        assert_eq!(status_of(&inspection, "orphan.jslot"), EntryStatus::FolderGone);
        assert_eq!(status_of(&inspection, "x.jslot"), EntryStatus::Unmapped);
        assert_eq!(inspection.unmapped_roots, vec!["Some Old Root".to_string()]);
        // Under an MO2 root the group is the mod folder.
        let changed = inspection.entries.iter().find(|e| e.file_name == "changed.jslot").unwrap();
        assert_eq!(changed.group, "ModA");
        assert_eq!(
            changed.target.as_deref(),
            Some(presets.join("changed.jslot").display().to_string().as_str())
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn restore_snapshots_what_it_overwrites_and_never_recreates_a_folder() {
        let base = temp_root("restore");
        let mods = base.join("mods");
        let presets = mods.join("ModA").join("Presets");
        std::fs::create_dir_all(&presets).unwrap();
        std::fs::write(presets.join("same.jslot"), b"SAME").unwrap();
        std::fs::write(presets.join("changed.jslot"), b"NEW").unwrap();

        let archive = base.join("JSLOT-BACKUP-01012026.zip");
        write_zip(
            &archive,
            &[
                ("MO2 mods (enabled)/ModA/Presets/same.jslot", b"SAME"),
                ("MO2 mods (enabled)/ModA/Presets/changed.jslot", b"OLD"),
                ("MO2 mods (enabled)/ModA/Presets/gone.jslot", b"GONE"),
                ("MO2 mods (enabled)/ModB/Presets/orphan.jslot", b"ORPHAN"),
            ],
        );
        let settings = settings_for(
            &base,
            vec![root("mo2-mods", "MO2 mods (enabled)", &mods, RootKind::Mo2Mods)],
        );
        let all: Vec<String> = inspect(&settings, &archive, |_| {})
            .unwrap()
            .entries
            .into_iter()
            .map(|e| e.entry)
            .collect();

        let outcome = restore(&settings, &archive, &all, |_| {}).unwrap();
        assert_eq!(outcome.restored.len(), 2, "changed + gone");
        assert_eq!(outcome.unchanged.len(), 1);
        assert_eq!(outcome.folder_gone.len(), 1);
        assert!(outcome.failed.is_empty(), "{:?}", outcome.failed);

        assert_eq!(std::fs::read(presets.join("changed.jslot")).unwrap(), b"OLD");
        assert_eq!(std::fs::read(presets.join("gone.jslot")).unwrap(), b"GONE");
        assert!(!mods.join("ModB").exists(), "a vanished mod folder is never recreated");

        // Undo: the snapshot holds what the restore overwrote, and only that.
        let id = outcome.snapshot_id.expect("an overwrite always snapshots first");
        let undo = snapshot::restore_snapshot_in(&settings, &id).unwrap();
        assert_eq!(undo.restored.len(), 1);
        assert_eq!(std::fs::read(presets.join("changed.jslot")).unwrap(), b"NEW");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn recreating_missing_presets_only_takes_no_snapshot() {
        let base = temp_root("missing-only");
        let mods = base.join("mods");
        let presets = mods.join("ModA");
        std::fs::create_dir_all(&presets).unwrap();
        let archive = base.join("JSLOT-BACKUP-01012026.zip");
        write_zip(&archive, &[("Mods/ModA/gone.jslot", b"GONE")]);
        let settings = settings_for(&base, vec![root("m", "Mods", &mods, RootKind::Mo2Mods)]);
        let outcome = restore(&settings, &archive, &["Mods/ModA/gone.jslot".into()], |_| {}).unwrap();
        assert_eq!(outcome.restored.len(), 1);
        assert_eq!(outcome.snapshot_id, None, "nothing was overwritten");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn manifest_resolves_by_root_id_after_a_label_rename() {
        let base = temp_root("manifest");
        let mods = base.join("mods");
        std::fs::create_dir_all(mods.join("ModA")).unwrap();
        std::fs::write(mods.join("ModA").join("a.jslot"), b"A").unwrap();
        let mut settings = settings_for(&base, vec![root("mods", "Old Label", &mods, RootKind::Plain)]);

        let outcome = backup::perform_backup(&settings, |_| {}).unwrap();
        let archive = PathBuf::from(&outcome.archive_path);
        let listed = list_archives(&base.join("Backups"));
        assert_eq!(listed.len(), 1);
        assert!(listed[0].has_manifest && listed[0].readable);
        assert_eq!(listed[0].preset_count, 1, "the manifest is not counted as a preset");

        settings.jslot_roots[0].label = "Renamed".into();
        let inspection = inspect(&settings, &archive, |_| {}).unwrap();
        assert!(inspection.has_manifest);
        assert_eq!(inspection.entries.len(), 1);
        assert_eq!(inspection.entries[0].status, EntryStatus::Unchanged);
        assert_eq!(inspection.entries[0].group, "Renamed", "plain roots group by label");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// A manifest can name any path at all; only configured roots are ever
    /// written to, and a relative path that climbs out is refused.
    #[test]
    fn restore_never_writes_outside_a_configured_root() {
        let base = temp_root("escape");
        let mods = base.join("mods");
        std::fs::create_dir_all(&mods).unwrap();
        let outside = base.join("outside");
        std::fs::create_dir_all(&outside).unwrap();

        let manifest = BackupManifest {
            format: 1,
            created_at: 0,
            entries: vec![
                ManifestEntry {
                    entry: "Unknown/a.jslot".into(),
                    source_path: outside.join("a.jslot").display().to_string(),
                    root_id: "not-configured".into(),
                    root_label: "Unknown".into(),
                    rel_path: "a.jslot".into(),
                },
                ManifestEntry {
                    entry: "Mods/b.jslot".into(),
                    source_path: String::new(),
                    root_id: "m".into(),
                    root_label: "Mods".into(),
                    rel_path: r"..\outside\b.jslot".into(),
                },
            ],
        };
        let json = serde_json::to_vec(&manifest).unwrap();
        let archive = base.join("JSLOT-BACKUP-01012026.zip");
        write_zip(
            &archive,
            &[("Unknown/a.jslot", b"A"), ("Mods/b.jslot", b"B"), (MANIFEST_ENTRY, &json)],
        );
        let settings = settings_for(&base, vec![root("m", "Mods", &mods, RootKind::Plain)]);
        let outcome = restore(
            &settings,
            &archive,
            &["Unknown/a.jslot".into(), "Mods/b.jslot".into()],
            |_| {},
        )
        .unwrap();
        assert_eq!(outcome.unmapped.len(), 2, "{outcome:?}");
        assert!(outcome.restored.is_empty());
        assert!(!outside.join("a.jslot").exists());
        assert!(!outside.join("b.jslot").exists());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn roots_colliding_on_a_sanitized_label_are_unmapped_not_guessed() {
        let a = PathBuf::from(r"D:\a");
        let b = PathBuf::from(r"D:\b");
        let roots = vec![
            root("1", "Mods:One", &a, RootKind::Plain),
            root("2", "Mods*One", &b, RootKind::Plain),
        ];
        assert!(root_for_label("Mods_One", &roots).is_none());
        assert!(root_for_label("Mods_One", &roots[..1]).is_some());
    }
}
