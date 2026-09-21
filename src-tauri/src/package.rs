//! The release packager: a chosen set of presets → the zip a player installs.
//!
//! Each preset goes in as it will ship — cleaned in memory with the chosen
//! categories, byte-for-byte when there's nothing to clean — under
//! `SKSE/Plugins/CharGen/Presets/`, keeping whatever follows `CharGen\Presets`
//! in its source path (so `Female\` / `Male\` survive). Its head export,
//! `<name>.nif` / `<name>.dds` in that same CharGen folder, goes beside it.
//!
//! Source files are only ever read. The zip is written to a temp file beside
//! the destination, reopened and checked against the plan, and only then
//! renamed into place — a failed pack never leaves a half-written zip, and
//! never replaces a good one.

use crate::clean::{self, CleanCategory};
use crate::jslot;
use crate::rawjson;
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

const CHARGEN: &str = "SKSE/Plugins/CharGen";
const HEAD_EXPORT_EXTENSIONS: [&str; 2] = ["nif", "dds"];
/// Enough of a list to act on without burying the message.
const LIST_LIMIT: usize = 12;

#[derive(Debug, Clone, Serialize)]
pub struct PackProgress {
    pub current: usize,
    pub total: usize,
    pub name: String,
}

#[derive(Debug, Serialize)]
pub struct PackOutcome {
    pub path: String,
    pub presets: usize,
    /// Presets that had something cleaned out of them.
    pub cleaned: usize,
    pub head_exports: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EntryKind {
    Preset,
    HeadExport,
}

#[derive(Debug)]
struct Entry {
    /// Path inside the zip, `/`-separated.
    name: String,
    source: PathBuf,
    kind: EntryKind,
}

fn parts(path: &Path) -> Vec<String> {
    path.components()
        .filter_map(|c| match c {
            Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect()
}

/// Index of the `Presets` in the last `CharGen\Presets` of `parts`, when a
/// file follows it.
fn presets_index(parts: &[String]) -> Option<usize> {
    (1..parts.len().saturating_sub(1))
        .rev()
        .find(|&i| parts[i].eq_ignore_ascii_case("presets") && parts[i - 1].eq_ignore_ascii_case("chargen"))
}

/// The optional folder inside `Presets/`: one name, valid on Windows.
fn folder_name(subfolder: Option<&str>) -> Result<Option<String>, String> {
    let Some(name) = subfolder.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    let bad = name == "."
        || name == ".."
        || name.ends_with('.')
        || name.chars().any(|c| c.is_control() || r#"<>:"/\|?*"#.contains(c));
    if bad {
        return Err(format!(
            "\"{name}\" can't be the folder inside Presets. Use a single folder name without \\ / : * ? \" < > |."
        ));
    }
    Ok(Some(name.to_string()))
}

/// A folder's files by lowercased name. Unreadable folders have none.
fn files_by_name(dir: &Path) -> HashMap<String, PathBuf> {
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
        .map(|e| (e.file_name().to_string_lossy().to_lowercase(), e.path()))
        .collect()
}

fn listed(lines: &[String]) -> String {
    let mut out = lines.iter().take(LIST_LIMIT).cloned().collect::<Vec<_>>().join("\n");
    if lines.len() > LIST_LIMIT {
        out.push_str(&format!("\n…and {} more", lines.len() - LIST_LIMIT));
    }
    out
}

/// Where every file goes. Two different sources on the same zip path
/// (compared without case, as Windows would) fail the plan; the same head
/// export reached from two presets is packed once.
fn plan(paths: &[String], subfolder: Option<&str>, head_exports: bool) -> Result<Vec<Entry>, String> {
    let folder = folder_name(subfolder)?;
    let mut entries: Vec<Entry> = Vec::new();
    let mut taken: HashMap<String, usize> = HashMap::new();
    let mut collisions: Vec<String> = Vec::new();
    let mut listings: HashMap<PathBuf, HashMap<String, PathBuf>> = HashMap::new();

    let mut push = |entry: Entry, entries: &mut Vec<Entry>| match taken.get(&entry.name.to_lowercase()) {
        Some(&i) if entries[i].source == entry.source => {}
        Some(&i) => collisions.push(format!(
            "{}\n    {}\n    {}",
            entry.name,
            entries[i].source.display(),
            entry.source.display()
        )),
        None => {
            taken.insert(entry.name.to_lowercase(), entries.len());
            entries.push(entry);
        }
    };

    for p in paths {
        let path = Path::new(p);
        let parts = parts(path);
        let (rel, chargen) = match presets_index(&parts) {
            Some(i) => (
                parts[i + 1..].to_vec(),
                path.ancestors().nth(parts.len() - i).map(Path::to_path_buf),
            ),
            None => (parts.last().cloned().into_iter().collect(), None),
        };
        let mut name = format!("{CHARGEN}/Presets");
        for part in folder.iter().chain(rel.iter()) {
            name.push('/');
            name.push_str(part);
        }
        push(
            Entry {
                name,
                source: path.to_path_buf(),
                kind: EntryKind::Preset,
            },
            &mut entries,
        );

        let Some(dir) = chargen.filter(|_| head_exports) else { continue };
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        let listing = listings.entry(dir.clone()).or_insert_with(|| files_by_name(&dir));
        for ext in HEAD_EXPORT_EXTENSIONS {
            let Some(source) = listing.get(&format!("{stem}.{ext}")) else { continue };
            let file_name = source
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            push(
                Entry {
                    name: format!("{CHARGEN}/{file_name}"),
                    source: source.clone(),
                    kind: EntryKind::HeadExport,
                },
                &mut entries,
            );
        }
    }

    if !collisions.is_empty() {
        return Err(format!(
            "{} file{} would land on the same path in the zip, so nothing was packed. Rename one of each pair, or leave one out:\n{}",
            collisions.len(),
            if collisions.len() == 1 { "" } else { "s" },
            listed(&collisions)
        ));
    }
    Ok(entries)
}

/// A preset's bytes as it will ship, and whether cleaning changed it.
fn shipped_bytes(path: &Path, categories: &[CleanCategory]) -> Result<(Vec<u8>, bool), String> {
    let bytes = std::fs::read(path).map_err(|e| format!("couldn't be read ({e})"))?;
    let (bom, text) = jslot::read_text(&bytes);
    serde_json::from_str::<Value>(&text).map_err(|e| format!("isn't valid preset JSON ({e})"))?;
    if categories.is_empty() {
        return Ok((bytes, false));
    }
    let mut raw = rawjson::parse(&text).map_err(|e| format!("isn't valid preset JSON ({e})"))?;
    if clean::clean(&mut raw, categories).is_empty() {
        return Ok((bytes, false));
    }
    let cleaned = jslot::to_formatted_string(&raw, jslot::detect_format(&text, bom));
    Ok((cleaned.into_bytes(), true))
}

fn zip_error(e: impl std::fmt::Display) -> String {
    format!("Couldn't write the pack ({e})")
}

/// Write every entry to `tmp`. Returns how many presets were cleaned. Keeps
/// reading after an unreadable preset so the error lists all of them.
fn write_pack(
    entries: &[Entry],
    categories: &[CleanCategory],
    tmp: &Path,
    progress: &mut dyn FnMut(PackProgress),
) -> Result<usize, String> {
    let file = std::fs::File::create(tmp).map_err(zip_error)?;
    let mut zip = zip::ZipWriter::new(file);
    let options =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    let mut cleaned = 0usize;
    let mut unreadable: Vec<String> = Vec::new();
    for (i, entry) in entries.iter().enumerate() {
        progress(PackProgress {
            current: i + 1,
            total: entries.len(),
            name: entry
                .source
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
        });
        match entry.kind {
            EntryKind::Preset => {
                let bytes = match shipped_bytes(&entry.source, categories) {
                    Ok((bytes, was_cleaned)) => {
                        cleaned += usize::from(was_cleaned);
                        bytes
                    }
                    Err(reason) => {
                        unreadable.push(format!("{} {reason}", entry.source.display()));
                        continue;
                    }
                };
                if unreadable.is_empty() {
                    zip.start_file(entry.name.as_str(), options).map_err(zip_error)?;
                    zip.write_all(&bytes).map_err(zip_error)?;
                }
            }
            // Tens of MB each (uncompressed tint masks): streamed, not loaded.
            EntryKind::HeadExport if unreadable.is_empty() => {
                let mut source = std::fs::File::open(&entry.source)
                    .map_err(|e| format!("Couldn't read {} ({e})", entry.source.display()))?;
                zip.start_file(entry.name.as_str(), options).map_err(zip_error)?;
                std::io::copy(&mut source, &mut zip)
                    .map_err(|e| format!("Couldn't pack {} ({e})", entry.source.display()))?;
            }
            EntryKind::HeadExport => {}
        }
    }
    if !unreadable.is_empty() {
        return Err(format!(
            "{} preset{} couldn't be packed, so nothing was. Fix or leave out:\n{}",
            unreadable.len(),
            if unreadable.len() == 1 { "" } else { "s" },
            listed(&unreadable)
        ));
    }
    zip.finish().map_err(zip_error)?.sync_all().map_err(zip_error)?;
    Ok(cleaned)
}

/// Reopen the written zip and hold it to the plan: every planned file and
/// nothing else, every preset parses, none of the chosen categories remain.
fn verify(tmp: &Path, entries: &[Entry], categories: &[CleanCategory]) -> Result<(), String> {
    let file = std::fs::File::open(tmp).map_err(|e| format!("Couldn't reopen the pack to check it ({e})"))?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| format!("The pack didn't reopen as a zip ({e})"))?;
    if zip.len() != entries.len() {
        return Err(format!(
            "The pack holds {} files instead of the {} planned.",
            zip.len(),
            entries.len()
        ));
    }
    for entry in entries {
        let mut file = zip
            .by_name(&entry.name)
            .map_err(|_| format!("{} is missing from the pack.", entry.name))?;
        if entry.kind != EntryKind::Preset {
            continue;
        }
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)
            .map_err(|e| format!("{} didn't read back from the pack ({e})", entry.name))?;
        let (_, text) = jslot::read_text(&bytes);
        serde_json::from_str::<Value>(&text)
            .map_err(|e| format!("{} doesn't parse in the pack ({e})", entry.name))?;
        let raw = rawjson::parse(&text).map_err(|e| format!("{} doesn't parse in the pack ({e})", entry.name))?;
        if clean::findings(&raw, None).iter().any(|f| categories.contains(&f.category)) {
            return Err(format!("{} still holds data that should have been cleaned.", entry.name));
        }
    }
    Ok(())
}

pub fn build_pack(
    paths: &[String],
    categories: &[CleanCategory],
    subfolder: Option<&str>,
    head_exports: bool,
    dest: &Path,
    progress: &mut dyn FnMut(PackProgress),
) -> Result<PackOutcome, String> {
    if paths.is_empty() {
        return Err("No presets chosen.".to_string());
    }
    let entries = plan(paths, subfolder, head_exports)?;
    let (Some(dir), Some(file_name)) = (dest.parent(), dest.file_name()) else {
        return Err(format!("{} isn't somewhere a file can be saved.", dest.display()));
    };
    let tmp = dir.join(format!(".{}.lineage-partial", file_name.to_string_lossy()));
    let result = write_pack(&entries, categories, &tmp, progress).and_then(|cleaned| {
        verify(&tmp, &entries, categories)?;
        std::fs::rename(&tmp, dest).map_err(|e| format!("Couldn't save {} ({e})", dest.display()))?;
        Ok(cleaned)
    });
    let cleaned = result.inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })?;
    let count = |kind| entries.iter().filter(|e| e.kind == kind).count();
    Ok(PackOutcome {
        path: dest.display().to_string(),
        presets: count(EntryKind::Preset),
        cleaned,
        head_exports: count(EntryKind::HeadExport),
    })
}

#[tauri::command]
pub async fn pack_release(
    paths: Vec<String>,
    categories: Vec<CleanCategory>,
    subfolder: Option<String>,
    include_head_exports: bool,
    dest: String,
    on_progress: tauri::ipc::Channel<PackProgress>,
) -> Result<PackOutcome, String> {
    tauri::async_runtime::spawn_blocking(move || {
        build_pack(
            &paths,
            &categories,
            subfolder.as_deref(),
            include_head_exports,
            Path::new(&dest),
            &mut |p| {
                let _ = on_progress.send(p);
            },
        )
    })
    .await
    .map_err(|e| format!("packing task failed: {e}"))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use CleanCategory::*;

    const DEFAULTS: [CleanCategory; 4] = [BodyMorphs, BodyOverlays, Skeleton, WeaponCamera];
    const TATTOOED: &str = r#"{"modNames":["KS Hairdo's.esp"],"overrides":[{"node":"Body [Ovl0]","values":[{"data":"Actors\\Character\\Overlays\\Tattoos\\t.dds","index":0,"key":9,"type":2}]},{"node":"Face [Ovl0]","values":[{"data":"Actors\\Character\\Overlays\\Freckles\\f.dds","index":0,"key":9,"type":2}]}]}"#;
    const PLAIN: &str = "{\r\n    \"modNames\" : [ \"KS Hairdo's.esp\" ]\r\n}\r\n";

    struct Sandbox(PathBuf);
    impl Sandbox {
        fn new(tag: &str) -> Sandbox {
            let dir = std::env::temp_dir().join(format!("lineage-pack-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Sandbox(dir)
        }
        fn file(&self, rel: &str, contents: &str) -> String {
            let p = self.0.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, contents).unwrap();
            p.display().to_string()
        }
        fn dest(&self) -> PathBuf {
            self.0.join("out").join("Pack.zip")
        }
    }
    impl Drop for Sandbox {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn pack(sb: &Sandbox, paths: &[String], categories: &[CleanCategory], sub: Option<&str>, heads: bool) -> Result<PackOutcome, String> {
        std::fs::create_dir_all(sb.dest().parent().unwrap()).unwrap();
        build_pack(paths, categories, sub, heads, &sb.dest(), &mut |_| {})
    }

    fn zip_names(path: &Path) -> Vec<String> {
        let mut zip = zip::ZipArchive::new(std::fs::File::open(path).unwrap()).unwrap();
        let mut names: Vec<String> = (0..zip.len()).map(|i| zip.by_index(i).unwrap().name().to_string()).collect();
        names.sort();
        names
    }

    fn zip_bytes(path: &Path, name: &str) -> Vec<u8> {
        let mut zip = zip::ZipArchive::new(std::fs::File::open(path).unwrap()).unwrap();
        let mut out = Vec::new();
        zip.by_name(name).unwrap().read_to_end(&mut out).unwrap();
        out
    }

    #[test]
    fn presets_land_under_chargen_presets_keeping_their_subfolders() {
        let sb = Sandbox::new("layout");
        let paths = vec![
            sb.file(r"ModA\SKSE\Plugins\CharGen\Presets\Female\Agnes.jslot", PLAIN),
            sb.file(r"ModA\skse\plugins\chargen\presets\Brynja.jslot", PLAIN),
            sb.file(r"Loose\Female\Cyra.jslot", PLAIN),
        ];
        pack(&sb, &paths, &[], None, false).unwrap();
        assert_eq!(
            zip_names(&sb.dest()),
            [
                "SKSE/Plugins/CharGen/Presets/Brynja.jslot",
                "SKSE/Plugins/CharGen/Presets/Cyra.jslot",
                "SKSE/Plugins/CharGen/Presets/Female/Agnes.jslot",
            ]
        );
        pack(&sb, &paths, &[], Some(" ColdSun "), false).unwrap();
        assert_eq!(
            zip_names(&sb.dest()),
            [
                "SKSE/Plugins/CharGen/Presets/ColdSun/Brynja.jslot",
                "SKSE/Plugins/CharGen/Presets/ColdSun/Cyra.jslot",
                "SKSE/Plugins/CharGen/Presets/ColdSun/Female/Agnes.jslot",
            ]
        );
    }

    #[test]
    fn cleaned_in_the_zip_and_never_on_disk() {
        let sb = Sandbox::new("clean");
        let tattooed = sb.file(r"M\SKSE\Plugins\CharGen\Presets\Inked.jslot", TATTOOED);
        let plain = sb.file(r"M\SKSE\Plugins\CharGen\Presets\Plain.jslot", PLAIN);
        let outcome = pack(&sb, &[tattooed.clone(), plain.clone()], &DEFAULTS, None, false).unwrap();
        assert_eq!((outcome.presets, outcome.cleaned, outcome.head_exports), (2, 1, 0));

        let inked = String::from_utf8(zip_bytes(&sb.dest(), "SKSE/Plugins/CharGen/Presets/Inked.jslot")).unwrap();
        assert!(!inked.contains("Body [Ovl0]"), "{inked}");
        assert!(inked.contains("Face [Ovl0]"), "face overlays are never touched: {inked}");
        assert_eq!(std::fs::read_to_string(&tattooed).unwrap(), TATTOOED, "the source is unchanged");
        assert_eq!(
            zip_bytes(&sb.dest(), "SKSE/Plugins/CharGen/Presets/Plain.jslot"),
            PLAIN.as_bytes(),
            "nothing to clean goes in byte-for-byte"
        );
    }

    #[test]
    fn head_exports_match_by_name_ignoring_case() {
        let sb = Sandbox::new("heads");
        let agnes = sb.file(r"M\SKSE\Plugins\CharGen\Presets\Female\Agnes.jslot", PLAIN);
        let brynja = sb.file(r"M\SKSE\Plugins\CharGen\Presets\Brynja.jslot", PLAIN);
        sb.file(r"M\SKSE\Plugins\CharGen\AGNES.NIF", "nif");
        sb.file(r"M\SKSE\Plugins\CharGen\agnes.dds", "dds");
        sb.file(r"M\SKSE\Plugins\CharGen\Brynja.nif.nif", "stray");
        sb.file(r"M\SKSE\Plugins\CharGen\Presets\Brynja.nif", "not beside Presets");

        let outcome = pack(&sb, &[agnes.clone(), brynja.clone()], &[], None, true).unwrap();
        assert_eq!(outcome.head_exports, 2);
        assert_eq!(
            zip_names(&sb.dest()),
            [
                "SKSE/Plugins/CharGen/AGNES.NIF",
                "SKSE/Plugins/CharGen/Presets/Brynja.jslot",
                "SKSE/Plugins/CharGen/Presets/Female/Agnes.jslot",
                "SKSE/Plugins/CharGen/agnes.dds",
            ]
        );
        assert_eq!(zip_bytes(&sb.dest(), "SKSE/Plugins/CharGen/AGNES.NIF"), b"nif");

        pack(&sb, &[agnes, brynja], &[], None, false).unwrap();
        assert_eq!(zip_names(&sb.dest()).len(), 2, "unticked: presets only");
    }

    #[test]
    fn a_shared_head_export_is_packed_once_but_a_clash_stops_the_pack() {
        let sb = Sandbox::new("clash");
        // Same stem in two subfolders of one mod: one head export, packed once.
        let female = sb.file(r"M\SKSE\Plugins\CharGen\Presets\Female\Ria.jslot", PLAIN);
        let male = sb.file(r"M\SKSE\Plugins\CharGen\Presets\Male\Ria.jslot", PLAIN);
        sb.file(r"M\SKSE\Plugins\CharGen\Ria.nif", "nif");
        assert_eq!(pack(&sb, &[female, male], &[], None, true).unwrap().head_exports, 1);

        // Same file name from two mods: refused, both named, nothing written.
        std::fs::remove_file(sb.dest()).unwrap();
        let a = sb.file(r"A\SKSE\Plugins\CharGen\Presets\Lydia.jslot", PLAIN);
        let b = sb.file(r"B\SKSE\Plugins\CharGen\Presets\LYDIA.jslot", PLAIN);
        let err = pack(&sb, &[a.clone(), b.clone()], &[], None, false).unwrap_err();
        assert!(err.contains(&a) && err.contains(&b), "{err}");
        assert!(!sb.dest().exists());
        assert_eq!(std::fs::read_dir(sb.dest().parent().unwrap()).unwrap().count(), 0, "no temp file left");
    }

    #[test]
    fn an_unreadable_preset_or_bad_folder_name_packs_nothing() {
        let sb = Sandbox::new("refuse");
        let good = sb.file(r"M\SKSE\Plugins\CharGen\Presets\Good.jslot", PLAIN);
        let bad = sb.file(r"M\SKSE\Plugins\CharGen\Presets\Bad.jslot", "{ not json");
        let err = pack(&sb, &[good.clone(), bad.clone()], &[], None, false).unwrap_err();
        assert!(err.contains(&bad) && !err.contains(&good), "{err}");
        assert_eq!(std::fs::read_dir(sb.dest().parent().unwrap()).unwrap().count(), 0, "no temp file left");

        for name in ["a/b", r"a\b", "..", "Pack.", "what?"] {
            assert!(pack(&sb, std::slice::from_ref(&good), &[], Some(name), false).is_err(), "{name}");
        }
    }

    #[test]
    fn a_failed_pack_leaves_an_existing_zip_alone() {
        let sb = Sandbox::new("keep");
        let good = sb.file(r"M\SKSE\Plugins\CharGen\Presets\Good.jslot", PLAIN);
        let bad = sb.file(r"M\SKSE\Plugins\CharGen\Presets\Bad.jslot", "{ not json");
        pack(&sb, std::slice::from_ref(&good), &[], None, false).unwrap();
        let before = std::fs::read(sb.dest()).unwrap();
        assert!(pack(&sb, &[good, bad], &[], None, false).is_err());
        assert_eq!(std::fs::read(sb.dest()).unwrap(), before);
    }
}
