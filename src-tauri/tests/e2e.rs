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

/// A real pack, as it would ship: requirements computed after an in-memory
/// clean with the default categories, rendered in the house BBCode. Offline
/// (no Nexus names), read-only against the real mods folder.
#[test]
fn real_pack_requirements_render_in_the_house_style() {
    use lineage_lib::clean::CleanCategory::*;
    use lineage_lib::requirements::{self, ExportFormat, RenderLine};
    let pack = Path::new(r"D:\Nordic Souls\mods\ColdSun's Face Presets");
    let tmp = sandbox("requirements");
    let Some(settings) = real_mo2_settings(&tmp) else { return };
    let Some(appdata) = std::env::var_os("APPDATA") else { return };
    if !pack.is_dir() {
        return;
    }
    let library = lineage_lib::library::load_from_dir(&PathBuf::from(appdata).join("com.coldsun.lineage"));
    let paths = requirements::presets_in(pack).unwrap();
    let before: Vec<Vec<u8>> = paths.iter().map(|p| std::fs::read(p).unwrap()).collect();

    let report = requirements::build_report(
        None,
        &settings,
        &library,
        &paths,
        &[BodyMorphs, BodyOverlays, Skeleton, WeaponCamera],
        &mut |_| {},
    );
    assert!(report.preset_count > 10, "saw {} presets", report.preset_count);
    assert_eq!(report.requirements[0].name, "RaceMenu", "RaceMenu leads");
    assert_eq!(report.requirements[0].used_by, report.preset_count);
    let mut links = std::collections::HashSet::new();
    for r in &report.requirements {
        if let Some(url) = &r.url {
            assert!(links.insert(url.to_ascii_lowercase()), "{} is listed twice", r.name);
        }
    }

    let lines: Vec<RenderLine> = report
        .requirements
        .iter()
        .map(|r| RenderLine { name: r.name.clone(), url: r.url.clone(), used_by: r.used_by })
        .collect();
    let bbcode = requirements::render(&lines, report.preset_count, ExportFormat::Bbcode, true);
    assert!(!bbcode.contains('\u{2014}') && !bbcode.contains('\u{2013}'));
    eprintln!(
        "{} presets, {} requirements, {} unidentified references\n{bbcode}",
        report.preset_count,
        report.requirements.len(),
        report.unknown.len()
    );
    for u in report.unknown.iter().take(8) {
        eprintln!("  unidentified: {} ({}) used by {}", u.asset.value, u.asset.kind, u.used_by);
    }

    // Read-only: the clean happened in memory.
    let after: Vec<Vec<u8>> = paths.iter().map(|p| std::fs::read(p).unwrap()).collect();
    assert!(before == after, "a preset on disk changed");
    let _ = std::fs::remove_dir_all(&tmp);
}

/// Pack real presets: all of ColdSun's Face Presets cleaned, a small pack
/// with its head exports, and subfolder packs (Anuketh's named folder,
/// Miggyluv's Female/Male). Every pack is verified by build_pack itself;
/// here the layout is checked and no source file may change.
#[test]
fn real_packs_build_verify_and_leave_sources_alone() {
    use lineage_lib::clean::CleanCategory::*;
    use lineage_lib::package;
    use lineage_lib::requirements;
    let chargen = Path::new(MODS).join(r"ColdSun's Face Presets\SKSE\Plugins\CharGen");
    let anuketh = Path::new(MODS).join("Anuketh's NPC Replacer Presets AIO - All in One");
    let miggy = Path::new(MODS).join("Miggyluv's Nord Presets (Vol.1)");
    if !chargen.is_dir() || !anuketh.is_dir() || !miggy.is_dir() {
        return;
    }
    let tmp = sandbox("package");
    let defaults = [BodyMorphs, BodyOverlays, Skeleton, WeaponCamera];
    let stamp = |p: &Path| {
        let m = std::fs::metadata(p).unwrap();
        (m.len(), m.modified().unwrap())
    };
    let names = |zip_path: &Path| -> Vec<String> {
        let mut zip = zip::ZipArchive::new(std::fs::File::open(zip_path).unwrap()).unwrap();
        (0..zip.len()).map(|i| zip.by_index(i).unwrap().name().to_string()).collect()
    };

    // 1. The whole collection, as it would ship.
    let all = requirements::presets_in(&chargen.join("Presets")).unwrap();
    let before: Vec<Vec<u8>> = all.iter().map(|p| std::fs::read(p).unwrap()).collect();
    let out = package::build_pack(&all, &defaults, None, false, &tmp.join("all.zip"), &mut |_| {}).unwrap();
    assert_eq!(out.presets, all.len());
    assert!(out.cleaned > 0, "some of the collection carries body or placement data");
    let after: Vec<Vec<u8>> = all.iter().map(|p| std::fs::read(p).unwrap()).collect();
    assert!(before == after, "a preset on disk changed");
    eprintln!("collection: {} presets, {} cleaned", out.presets, out.cleaned);

    // 2. The three presets with the smallest head exports, exports included.
    let head_size = |p: &String| -> u64 {
        let stem = Path::new(p).file_stem().unwrap().to_string_lossy().into_owned();
        ["nif", "dds"]
            .iter()
            .filter_map(|ext| std::fs::metadata(chargen.join(format!("{stem}.{ext}"))).ok())
            .map(|m| m.len())
            .sum()
    };
    let mut with_heads: Vec<(u64, String)> =
        all.iter().map(|p| (head_size(p), p.clone())).filter(|(size, _)| *size > 0).collect();
    with_heads.sort();
    let small: Vec<String> = with_heads.into_iter().take(3).map(|(_, p)| p).collect();
    let heads: Vec<PathBuf> = small
        .iter()
        .flat_map(|p| {
            let stem = Path::new(p).file_stem().unwrap().to_string_lossy().into_owned();
            ["nif", "dds"].map(|ext| chargen.join(format!("{stem}.{ext}")))
        })
        .filter(|p| p.is_file())
        .collect();
    let head_stamps: Vec<_> = heads.iter().map(|p| stamp(p)).collect();
    let out = package::build_pack(&small, &defaults, None, true, &tmp.join("heads.zip"), &mut |_| {}).unwrap();
    assert_eq!((out.presets, out.head_exports), (3, heads.len()));
    let packed = names(&tmp.join("heads.zip"));
    for h in &heads {
        let entry = format!("SKSE/Plugins/CharGen/{}", h.file_name().unwrap().to_string_lossy());
        assert!(packed.contains(&entry), "{entry} missing from {packed:?}");
    }
    assert_eq!(heads.iter().map(|p| stamp(p)).collect::<Vec<_>>(), head_stamps);

    // 3. Subfolders survive: Anuketh's named folder, Miggyluv's Female/Male.
    let mut sub = requirements::presets_in(&anuketh).unwrap();
    sub.extend(requirements::presets_in(&miggy).unwrap());
    let out = package::build_pack(&sub, &defaults, None, true, &tmp.join("sub.zip"), &mut |_| {}).unwrap();
    assert_eq!(out.presets, sub.len());
    let packed = names(&tmp.join("sub.zip"));
    assert!(packed.iter().any(|n| n.starts_with("SKSE/Plugins/CharGen/Presets/[Anuketh Presets]/")));
    assert!(packed.iter().any(|n| n.starts_with("SKSE/Plugins/CharGen/Presets/Female/")));
    assert!(packed.iter().any(|n| n.starts_with("SKSE/Plugins/CharGen/Presets/Male/")));
    eprintln!("subfolders: {} presets, {} cleaned, {} head exports", out.presets, out.cleaned, out.head_exports);
    let _ = std::fs::remove_dir_all(&tmp);
}

/// The BSA reader against the real base-game archives: every name the
/// header promises comes out, and textures sit where they should.
#[test]
fn real_game_archives_list_every_file() {
    let data = Path::new(r"D:\Nordic Souls\Game Root\Data");
    if !data.is_dir() {
        return;
    }
    let mut total = 0usize;
    for n in 0..=8 {
        let archive = data.join(format!("Skyrim - Textures{n}.bsa"));
        if !archive.is_file() {
            continue;
        }
        let bytes = std::fs::read(&archive).unwrap();
        let promised = u32::from_le_bytes(bytes[20..24].try_into().unwrap()) as usize;
        let mut names = Vec::new();
        lineage_lib::bsa::for_each_path(&archive, &mut |p| names.push(p.to_string())).unwrap();
        assert_eq!(names.len(), promised, "{}", archive.display());
        assert!(names.iter().all(|p| p.starts_with("textures\\") && p == &p.to_lowercase()));
        total += names.len();
    }
    assert!(total > 10_000, "only {total} textures in the base game?");
    eprintln!("{total} base-game textures listed");
}

/// Sweep the real setup. Every texture reported missing is cross-checked
/// by brute force — a stat in every enabled mod, overwrite and Data — and
/// every missing plugin against plugins.txt directly, so a false alarm
/// fails here rather than on Heath's screen.
#[test]
fn real_sweep_reports_no_false_alarms() {
    use lineage_lib::readiness::{self, Status};
    let tmp = sandbox("readiness");
    let Some(mut settings) = real_mo2_settings(&tmp) else { return };
    settings.skyrim_folder = r"D:\Nordic Souls\Game Root".into();
    let Some(appdata) = std::env::var_os("APPDATA") else { return };
    let library = lineage_lib::library::load_from_dir(&PathBuf::from(appdata).join("com.coldsun.lineage"));

    let started = std::time::Instant::now();
    let setup = readiness::Setup::load(&settings).unwrap();
    let indexed = started.elapsed();
    let paths: Vec<String> = scan::scan_settings(&settings).files.into_iter().map(|f| f.path).collect();
    let report = readiness::sweep(&setup, &library, &paths, &mut |_| {});
    eprintln!(
        "indexed in {indexed:?}, swept in {:?}: {} presets, {} ready, {} missing, {} unconfirmed, {} unreadable",
        started.elapsed(),
        report.total,
        report.ready,
        report.missing,
        report.unconfirmed,
        report.unreadable.len()
    );
    for c in report.causes.iter().take(12) {
        eprintln!(
            "  {:?} {} ({} presets, {} refs, e.g. {:?}): {}",
            c.status,
            c.title,
            c.presets.len(),
            c.references.len(),
            c.references.first(),
            c.detail
        );
    }
    assert_eq!(report.total + report.unreadable.len(), paths.len());
    assert_eq!(report.ready + report.missing, report.total);

    let modlist = std::fs::read_to_string(Path::new(PROFILE).join("modlist.txt")).unwrap();
    let mut enabled: Vec<PathBuf> = modlist
        .lines()
        .filter_map(|l| l.strip_prefix('+'))
        .map(|m| Path::new(MODS).join(m.trim()))
        .collect();
    enabled.push(PathBuf::from(r"D:\Nordic Souls\overwrite"));
    enabled.push(PathBuf::from(r"D:\Nordic Souls\Game Root\Data"));
    let plugins_txt = std::fs::read_to_string(Path::new(PROFILE).join("plugins.txt")).unwrap().to_lowercase();

    let mut checked = 0;
    for cause in report.causes.iter().filter(|c| c.status == Status::Missing) {
        for r in &cause.references {
            let lower = r.to_lowercase();
            if lower.ends_with(".esp") || lower.ends_with(".esm") || lower.ends_with(".esl") {
                let installed = enabled.iter().any(|m| m.join(r.trim()).is_file());
                let active = plugins_txt.lines().any(|l| l.trim() == format!("*{}", lower.trim()));
                assert!(!(installed && active), "{r} was reported missing but is installed and active");
            } else {
                let norm = lower.trim().replace('/', "\\");
                let norm = norm.trim_start_matches('\\');
                let norm = norm.strip_prefix("data\\").unwrap_or(norm);
                let norm = norm.strip_prefix("textures\\").unwrap_or(norm);
                let loose = enabled.iter().find(|m| m.join("textures").join(norm).is_file());
                assert!(loose.is_none(), "{r} was reported missing but is loose in {:?}", loose);
            }
            checked += 1;
        }
    }
    eprintln!("{checked} missing references cross-checked");
    let _ = std::fs::remove_dir_all(&tmp);
}
