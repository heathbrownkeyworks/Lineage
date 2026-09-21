//! End-to-end tests against this machine's real modding setup (read-only for
//! mod folders; all writes go to a temp sandbox). Each test skips quietly on
//! machines without the Nordic Souls MO2 instance.

use lineage_lib::backup;
use lineage_lib::restore;
use lineage_lib::scan;
use lineage_lib::settings::{AppSettings, JslotRoot, ModManagerKind, RootKind};
use lineage_lib::snapshot;
use std::io::Read;
use std::path::{Path, PathBuf};

const MODS: &str = r"D:\Nordic Souls\mods";
const PROFILE: &str = r"D:\Nordic Souls\profiles\Nordic Souls - ENB";

fn real_mo2_settings(backup_dir: &Path) -> Option<AppSettings> {
    if !Path::new(MODS).is_dir() || !Path::new(PROFILE).is_dir() {
        return None;
    }
    Some(AppSettings {
        mod_manager: ModManagerKind::Mo2,
        mo2_instance: r"D:\Nordic Souls".into(),
        mo2_mods_folder: MODS.into(),
        mo2_profile_dir: PROFILE.into(),
        jslot_roots: vec![
            JslotRoot {
                id: "mo2-mods".into(),
                label: "MO2 mods (enabled)".into(),
                path: MODS.into(),
                kind: RootKind::Mo2Mods,
            },
            JslotRoot {
                id: "mo2-overwrite".into(),
                label: "MO2 overwrite".into(),
                path: r"D:\Nordic Souls\overwrite".into(),
                kind: RootKind::Plain,
            },
        ],
        backup_dir: backup_dir.display().to_string(),
        ..Default::default()
    })
}

fn sandbox(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("lineage-e2e-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Scan the real MO2 setup: enabled-mod filtering must find a substantial
/// preset collection (this machine has ~1,800).
#[test]
fn real_scan_finds_the_preset_collection() {
    let tmp = sandbox("scan");
    let Some(settings) = real_mo2_settings(&tmp) else { return };
    let result = scan::scan_settings(&settings);
    assert!(
        result.total > 500,
        "expected a large preset collection, found {}",
        result.total
    );
    assert!(result.roots[0].exists);
    // Every file must carry a root-relative path for archive anchoring.
    assert!(result.files.iter().all(|f| !f.rel_path.is_empty()));
    let _ = std::fs::remove_dir_all(&tmp);
}

/// Full backup of the real collection into a sandbox: naming, no-overwrite
/// counter, entry paths anchored per root, and byte-identical content.
#[test]
fn real_backup_roundtrips_bytes() {
    let tmp = sandbox("backup");
    let Some(settings) = real_mo2_settings(&tmp) else { return };

    let mut seen_progress = 0usize;
    let outcome = backup::perform_backup(&settings, |p| {
        assert!(p.current <= p.total);
        seen_progress = p.current;
    })
    .expect("backup runs");
    assert!(seen_progress > 0, "progress was streamed");
    let archive = PathBuf::from(&outcome.archive_path);
    assert!(archive.is_file());
    let name = archive.file_name().unwrap().to_string_lossy().into_owned();
    assert!(
        name.starts_with("JSLOT-BACKUP-") && name.ends_with(".zip"),
        "archive name {name}"
    );

    // Second run must not overwrite: counter suffix.
    let second = backup::perform_backup(&settings, |_| {}).expect("second backup runs");
    assert_ne!(second.archive_path, outcome.archive_path);
    assert!(PathBuf::from(&second.archive_path)
        .file_name()
        .unwrap()
        .to_string_lossy()
        .contains("-2"));

    // Spot-check: one entry restores byte-identical to its source file.
    let scan = scan::scan_settings(&settings);
    assert_eq!(outcome.file_count, scan.total);
    let sample = &scan.files[0];
    let file = std::fs::File::open(&archive).unwrap();
    let mut zip = zip::ZipArchive::new(file).unwrap();
    let entry_name = format!(
        "{}/{}",
        sample.root_label.clone(),
        sample.rel_path.replace('\\', "/")
    );
    let mut entry = zip
        .by_name(&entry_name)
        .unwrap_or_else(|_| panic!("entry {entry_name} present"));
    let mut archived = Vec::new();
    entry.read_to_end(&mut archived).unwrap();
    let original = std::fs::read(&sample.path).unwrap();
    assert_eq!(archived, original, "archive preserves bytes");

    let _ = std::fs::remove_dir_all(&tmp);
}

/// The destructive path end-to-end on COPIES of real presets: snapshot →
/// remove → verify only the section changed → restore → byte-identical.
#[test]
fn snapshot_clean_restore_on_real_preset_copies() {
    let tmp = sandbox("remove");
    let corpus =
        Path::new(r"D:\Nordic Souls\mods\ColdSun's Face Presets\SKSE\Plugins\CharGen\Presets");
    if !corpus.is_dir() {
        return;
    }
    // Copy a handful of real presets that carry bodyMorphs into the sandbox.
    use lineage_lib::clean::CleanCategory::*;
    let defaults = [BodyMorphs, BodyOverlays, Skeleton, WeaponCamera];
    let work = tmp.join("mod").join("SKSE").join("Plugins").join("CharGen").join("Presets");
    std::fs::create_dir_all(&work).unwrap();
    let mut copies: Vec<PathBuf> = Vec::new();
    for entry in std::fs::read_dir(corpus).unwrap().flatten() {
        if copies.len() >= 3 {
            break;
        }
        let path = entry.path();
        let is_jslot = path
            .extension()
            .map(|e| e.eq_ignore_ascii_case("jslot"))
            .unwrap_or(false);
        if !is_jslot {
            continue;
        }
        let inspection = lineage_lib::clean::inspect(&path, false);
        if inspection.parse_error.is_none()
            && inspection.findings.iter().any(|f| f.category == BodyMorphs)
        {
            let dest = work.join(path.file_name().unwrap());
            std::fs::copy(&path, &dest).unwrap();
            copies.push(dest);
        }
    }
    if copies.is_empty() {
        return;
    }

    let settings = AppSettings {
        jslot_roots: vec![JslotRoot {
            id: "sandbox".into(),
            label: "Sandbox".into(),
            path: tmp.join("mod").display().to_string(),
            kind: RootKind::Plain,
        }],
        backup_dir: tmp.join("Backups").display().to_string(),
        ..Default::default()
    };

    let originals: Vec<Vec<u8>> = copies.iter().map(|p| std::fs::read(p).unwrap()).collect();

    // Snapshot first — the invariant every destructive op relies on.
    let meta = snapshot::write_snapshot(&settings, "clean-batch", &copies).expect("snapshot");
    assert_eq!(meta.entries.len(), copies.len());

    // Clean with the defaults.
    for path in &copies {
        let detail = lineage_lib::clean::clean_in_place(path, &defaults)
            .expect("clean")
            .expect("each copy carries body morphs");
        assert!(detail.removed.iter().all(|f| f.count > 0));
        let after = std::fs::read_to_string(path).unwrap();
        assert!(!after.contains("\"bodyMorphs\""));
    }
    // Nothing but the presets in the folder — no .bak, no temp leftovers.
    let stray: Vec<_> = std::fs::read_dir(&work)
        .unwrap()
        .flatten()
        .filter(|e| {
            !e.path()
                .extension()
                .map(|x| x.eq_ignore_ascii_case("jslot"))
                .unwrap_or(false)
        })
        .collect();
    assert!(stray.is_empty(), "stray files in mod folder: {stray:?}");

    // Restore — byte-identical to the pre-clean originals.
    let report = snapshot::restore_snapshot_in(&settings, &meta.id).expect("restore");
    assert_eq!(report.restored.len(), copies.len());
    assert!(report.failed.is_empty());
    assert!(report.missing_destination.is_empty());
    for (path, original) in copies.iter().zip(&originals) {
        assert_eq!(&std::fs::read(path).unwrap(), original, "{}", path.display());
    }

    let _ = std::fs::remove_dir_all(&tmp);
}

/// The one real backup on this machine predates manifests, so restore has to
/// map it back through the root label alone. Inspection is read-only; the
/// write path is covered by the unit tests, which run in temp folders.
#[test]
fn real_legacy_backup_maps_onto_the_live_collection() {
    let archive = Path::new(r"C:\Users\coldsun\Documents\Lineage\Backups\JSLOT-BACKUP-08242026.zip");
    let tmp = sandbox("inspect");
    let Some(settings) = real_mo2_settings(&tmp) else { return };
    if !archive.is_file() {
        return;
    }
    let inspection = restore::inspect(&settings, archive, |_| {}).expect("inspects");
    assert!(!inspection.has_manifest, "this archive predates manifests");
    assert!(inspection.entries.len() > 1000, "saw {}", inspection.entries.len());
    assert!(
        inspection.unmapped_roots.is_empty(),
        "every preset should map through its root label: {:?}",
        inspection.unmapped_roots
    );
    let mut counts = std::collections::BTreeMap::new();
    for e in &inspection.entries {
        *counts.entry(format!("{:?}", e.status)).or_insert(0usize) += 1;
        if let Some(target) = &e.target {
            assert!(
                target.to_ascii_lowercase().starts_with(&MODS.to_ascii_lowercase()),
                "{target} resolved outside the mods root"
            );
        }
    }
    eprintln!("status counts for the real backup: {counts:?}");
    assert!(counts.get("Unmapped").is_none(), "{counts:?}");
    let _ = std::fs::remove_dir_all(&tmp);
}

/// The failure mode that matters for the nudge is a path-normalization
/// mismatch between the scan and the archive, which would make every preset
/// look uncovered and nag forever. Against the real legacy backup, only the
/// handful actually edited since should count.
#[test]
fn real_backup_covers_nearly_everything_on_disk() {
    let archive = Path::new(r"C:\Users\coldsun\Documents\Lineage\Backups\JSLOT-BACKUP-08242026.zip");
    let tmp = sandbox("nudge");
    let Some(mut settings) = real_mo2_settings(&tmp) else { return };
    if !archive.is_file() {
        return;
    }
    let taken = std::fs::metadata(archive)
        .unwrap()
        .modified()
        .unwrap()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    settings.last_backup_path = Some(archive.display().to_string());
    settings.last_backup_at = Some(taken);
    let scan = scan::scan_settings(&settings);
    let changed = backup::changed_since_backup(&settings, &scan).expect("the backup is usable");
    eprintln!("{changed} of {} presets not covered by the real backup", scan.total);
    assert!(
        changed * 2 < scan.total,
        "{changed} of {} uncovered — the scan and the archive disagree on paths",
        scan.total
    );
    let _ = std::fs::remove_dir_all(&tmp);
}
