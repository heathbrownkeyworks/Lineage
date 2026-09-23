//! The release packager: a chosen set of presets → the zip a player installs.
//!
//! Each preset goes in as it will ship — cleaned in memory with the chosen
//! categories, byte-for-byte when there's nothing to clean — under
//! `SKSE/Plugins/CharGen/Presets/`, keeping whatever follows `CharGen\Presets`
//! in its source path (so `Female\` / `Male\` survive). Its RaceMenu head
//! export — the `.nif` and `.dds` that Export Head writes — goes to
//! `SKSE/Plugins/CharGen/` under the preset's own name, from whichever files
//! were found for it or chosen by hand.
//!
//! Source files are only ever read. The zip is written to a temp file beside
//! the destination, reopened and checked against the plan, and only then
//! renamed into place — a failed pack never leaves a half-written zip, and
//! never replaces a good one.

use crate::clean::{self, CleanCategory};
use crate::jslot;
use crate::rawjson;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

const CHARGEN: &str = "SKSE/Plugins/CharGen";
const HEAD_EXPORT_EXTENSIONS: [&str; 2] = ["nif", "dds"];
/// Enough of a list to act on without burying the message.
const LIST_LIMIT: usize = 12;
/// Find in a folder gives up past this many entries: a folder that big isn't
/// where a pack's exports are, and walking it would take minutes.
const FOLDER_SEARCH_LIMIT: usize = 200_000;

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

/// One file of a head export.
#[derive(Debug, Clone, Serialize)]
pub struct ExportFile {
    pub path: String,
    pub size: u64,
}

/// A preset's RaceMenu head export. Export Head writes the head mesh
/// (`<name>.nif`) and its tint (`<name>.dds`) together, under whatever name was
/// typed in its dialog, so the name often isn't the preset's.
#[derive(Debug, Clone, Serialize)]
pub struct HeadExport {
    pub preset: String,
    pub nif: Option<ExportFile>,
    pub dds: Option<ExportFile>,
    /// Where to start looking by hand: the CharGen folder holding the preset's
    /// Presets folder, or the preset's own folder.
    pub look_in: String,
}

/// The files chosen for one preset, as the page sends them to be packed.
#[derive(Debug, Clone, Deserialize)]
pub struct HeadExportChoice {
    pub preset: String,
    pub nif: Option<String>,
    pub dds: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct FolderMatch {
    pub found: Vec<HeadExport>,
    /// Presets whose name turned up in more than one folder, left to be chosen
    /// by hand.
    pub ambiguous: Vec<String>,
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

/// A folder's files by lowercased name, with sizes — both from the listing
/// itself, no file opened. Unreadable folders have none.
fn files_by_name(dir: &Path) -> HashMap<String, ExportFile> {
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let meta = e.metadata().ok().filter(|m| m.is_file())?;
            Some((
                e.file_name().to_string_lossy().to_lowercase(),
                ExportFile {
                    path: e.path().display().to_string(),
                    size: meta.len(),
                },
            ))
        })
        .collect()
}

fn listed(lines: &[String]) -> String {
    let mut out = lines.iter().take(LIST_LIMIT).cloned().collect::<Vec<_>>().join("\n");
    if lines.len() > LIST_LIMIT {
        out.push_str(&format!("\n…and {} more", lines.len() - LIST_LIMIT));
    }
    out
}

/// The CharGen folder holding the preset's `CharGen\Presets` folder: where
/// RaceMenu writes head exports, and where preset mods ship them.
fn chargen_of(preset: &Path) -> Option<PathBuf> {
    let parts = parts(preset);
    let i = presets_index(&parts)?;
    preset.ancestors().nth(parts.len() - i).map(Path::to_path_buf)
}

fn look_in(preset: &Path) -> PathBuf {
    chargen_of(preset)
        .or_else(|| preset.parent().map(Path::to_path_buf))
        .unwrap_or_default()
}

fn lower_stem(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

/// A head export file, checked by its first bytes to really be what its
/// extension says: a NIF (`Gamebryo File Format…`, `NetImmerse File Format…`)
/// or a DDS (`DDS `).
fn checked(path: &Path, ext: &str) -> Result<ExportFile, String> {
    let read = || -> std::io::Result<(u64, Vec<u8>)> {
        let file = std::fs::File::open(path)?;
        let size = file.metadata()?.len();
        let mut head = Vec::new();
        file.take(22).read_to_end(&mut head)?;
        Ok((size, head))
    };
    let (size, head) = read().map_err(|e| format!("{} couldn't be read ({e})", path.display()))?;
    let genuine = match ext {
        "nif" => head.starts_with(b"Gamebryo File Format") || head.starts_with(b"NetImmerse File Format"),
        "dds" => head.starts_with(b"DDS "),
        _ => false,
    };
    if !genuine {
        let what = if ext == "nif" { "a NIF mesh" } else { "a DDS texture" };
        return Err(format!("{} isn't {what}.", path.display()));
    }
    Ok(ExportFile {
        path: path.display().to_string(),
        size,
    })
}

/// Each preset's head export, found by exact name (ignoring case): first in
/// the CharGen folder holding its Presets folder, then beside the preset
/// itself. The first folder with either file supplies both — an export is a
/// pair written together, so two folders' files are never mixed. Names that
/// only look alike are never matched: close names are often other faces.
pub fn find_head_exports(paths: &[String]) -> Vec<HeadExport> {
    let mut listings: HashMap<PathBuf, HashMap<String, ExportFile>> = HashMap::new();
    paths
        .iter()
        .map(|p| {
            let path = Path::new(p);
            let stem = lower_stem(path);
            let mut export = HeadExport {
                preset: p.clone(),
                nif: None,
                dds: None,
                look_in: look_in(path).display().to_string(),
            };
            for dir in chargen_of(path).into_iter().chain(path.parent().map(Path::to_path_buf)) {
                let listing = listings.entry(dir.clone()).or_insert_with(|| files_by_name(&dir));
                let nif = listing.get(&format!("{stem}.nif")).cloned();
                let dds = listing.get(&format!("{stem}.dds")).cloned();
                if nif.is_some() || dds.is_some() {
                    export.nif = nif;
                    export.dds = dds;
                    break;
                }
            }
            export
        })
        .collect()
}

/// A head export chosen by hand: one `.nif`, one `.dds`, or both. A lone pick
/// brings its partner of the same name from the same folder, since Export
/// Head writes the two together.
pub fn head_export_from_picked(preset: &str, picked: &[String]) -> Result<HeadExport, String> {
    let mut nif: Option<PathBuf> = None;
    let mut dds: Option<PathBuf> = None;
    for p in picked {
        let path = PathBuf::from(p);
        let ext = path
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        let slot = match ext.as_str() {
            "nif" => &mut nif,
            "dds" => &mut dds,
            _ => return Err(format!("{} isn't a .nif or .dds file.", path.display())),
        };
        if slot.is_some() {
            return Err("Pick one .nif and one .dds at most: the head export of one preset.".to_string());
        }
        *slot = Some(path);
    }
    let lone = match (&nif, &dds) {
        (None, None) => return Err("Nothing was picked.".to_string()),
        (Some(one), None) => Some((one.clone(), "dds")),
        (None, Some(one)) => Some((one.clone(), "nif")),
        _ => None,
    };
    if let Some((one, partner_ext)) = lone {
        let partner = one
            .parent()
            .and_then(|dir| files_by_name(dir).remove(&format!("{}.{partner_ext}", lower_stem(&one))))
            .map(|f| PathBuf::from(f.path))
            // Only a genuine one: a bad partner doesn't spoil a good pick.
            .filter(|p| checked(p, partner_ext).is_ok());
        if partner_ext == "nif" {
            nif = partner;
        } else {
            dds = partner;
        }
    }
    Ok(HeadExport {
        preset: preset.to_string(),
        nif: nif.map(|p| checked(&p, "nif")).transpose()?,
        dds: dds.map(|p| checked(&p, "dds")).transpose()?,
        look_in: look_in(Path::new(preset)).display().to_string(),
    })
}

/// Head exports for `presets` found anywhere under `folder`, by exact name. A
/// name in more than one folder isn't guessed at: those presets come back as
/// ambiguous, to be chosen by hand.
pub fn find_head_exports_in(folder: &Path, presets: &[String]) -> Result<FolderMatch, String> {
    if !folder.is_dir() {
        return Err(format!("{} isn't a folder.", folder.display()));
    }
    let wanted: HashSet<String> = presets.iter().map(|p| lower_stem(Path::new(p))).collect();
    /// A folder's `.nif` and `.dds` for one name.
    type Pair = (Option<PathBuf>, Option<PathBuf>);
    // Name → the folders holding it → its pair there.
    let mut by_stem: HashMap<String, BTreeMap<PathBuf, Pair>> = HashMap::new();
    for (seen, entry) in walkdir::WalkDir::new(folder)
        .follow_links(false)
        .into_iter()
        .flatten()
        .enumerate()
    {
        if seen >= FOLDER_SEARCH_LIMIT {
            return Err(format!(
                "{} holds too many files to search. Pick the folder your head exports are in.",
                folder.display()
            ));
        }
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        let ext = path
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        let stem = lower_stem(path);
        if !HEAD_EXPORT_EXTENSIONS.contains(&ext.as_str()) || !wanted.contains(&stem) {
            continue;
        }
        let Some(dir) = path.parent() else { continue };
        let slot = by_stem.entry(stem).or_default().entry(dir.to_path_buf()).or_default();
        if ext == "nif" {
            slot.0 = Some(path.to_path_buf());
        } else {
            slot.1 = Some(path.to_path_buf());
        }
    }
    let mut out = FolderMatch {
        found: Vec::new(),
        ambiguous: Vec::new(),
    };
    for p in presets {
        let Some(folders) = by_stem.get(&lower_stem(Path::new(p))) else { continue };
        if folders.len() > 1 {
            out.ambiguous.push(p.clone());
            continue;
        }
        let Some((nif, dds)) = folders.values().next() else { continue };
        let nif = nif.as_deref().and_then(|n| checked(n, "nif").ok());
        let dds = dds.as_deref().and_then(|d| checked(d, "dds").ok());
        if nif.is_some() || dds.is_some() {
            out.found.push(HeadExport {
                preset: p.clone(),
                nif,
                dds,
                look_in: look_in(Path::new(p)).display().to_string(),
            });
        }
    }
    Ok(out)
}

/// Two choices naming the same files (ignoring case, as Windows does).
fn same_export(a: &HeadExportChoice, b: &HeadExportChoice) -> bool {
    let key = |c: &HeadExportChoice| {
        (
            c.nif.as_deref().map(str::to_lowercase),
            c.dds.as_deref().map(str::to_lowercase),
        )
    };
    key(a) == key(b)
}

fn export_text(c: &HeadExportChoice) -> String {
    let files: Vec<&str> = [c.nif.as_deref(), c.dds.as_deref()].into_iter().flatten().collect();
    if files.is_empty() {
        "nothing".to_string()
    } else {
        files.join(" + ")
    }
}

/// Where every file goes. Two different sources on the same zip path
/// (compared without case, as Windows would) fail the plan; the same head
/// export chosen for two presets is packed once. Head exports go to
/// `CharGen/` under the preset's own name, whatever the files are called, and
/// each is checked to really be a NIF or DDS first.
fn plan(paths: &[String], subfolder: Option<&str>, choices: &[HeadExportChoice]) -> Result<Vec<Entry>, String> {
    let folder = folder_name(subfolder)?;
    let chosen: HashMap<String, &HeadExportChoice> =
        choices.iter().map(|c| (c.preset.to_lowercase(), c)).collect();
    let mut entries: Vec<Entry> = Vec::new();
    let mut taken: HashMap<String, usize> = HashMap::new();
    let mut collisions: Vec<String> = Vec::new();
    let mut not_exports: Vec<String> = Vec::new();
    // One name in the zip holds one head export. Two presets of one name may
    // share theirs, but one export's head must never ship with another's tint.
    let mut claimed: HashMap<String, (&str, &HeadExportChoice)> = HashMap::new();
    let mut mixed: Vec<String> = Vec::new();

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
        let rel: Vec<String> = match presets_index(&parts) {
            Some(i) => parts[i + 1..].to_vec(),
            None => parts.last().cloned().into_iter().collect(),
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

        let Some(&choice) = chosen.get(&p.to_lowercase()) else { continue };
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        if let Some(&(first, earlier)) = claimed.get(&stem.to_lowercase()) {
            if !same_export(earlier, choice) {
                mixed.push(format!(
                    "{CHARGEN}/{stem}.nif and .dds, one name for two head exports\n    {first}: {}\n    {p}: {}",
                    export_text(earlier),
                    export_text(choice)
                ));
            }
            // Either way it's packed once, from the first.
            continue;
        }
        claimed.insert(stem.to_lowercase(), (p.as_str(), choice));
        for (ext, source) in [("nif", &choice.nif), ("dds", &choice.dds)] {
            let Some(source) = source else { continue };
            let source = PathBuf::from(source);
            match checked(&source, ext) {
                Ok(_) => push(
                    Entry {
                        name: format!("{CHARGEN}/{stem}.{ext}"),
                        source,
                        kind: EntryKind::HeadExport,
                    },
                    &mut entries,
                ),
                Err(reason) => not_exports.push(reason),
            }
        }
    }

    if !not_exports.is_empty() {
        return Err(format!(
            "{} head export file{} can't be packed, so nothing was. Choose {} again:\n{}",
            not_exports.len(),
            if not_exports.len() == 1 { "" } else { "s" },
            if not_exports.len() == 1 { "it" } else { "them" },
            listed(&not_exports)
        ));
    }
    collisions.extend(mixed);
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
        if entry.kind == EntryKind::HeadExport {
            // Streamed from disk, tens of MB: it must have gone in whole.
            let expected = std::fs::metadata(&entry.source)
                .map(|m| m.len())
                .map_err(|e| format!("Couldn't check {} ({e})", entry.source.display()))?;
            if file.size() != expected {
                return Err(format!(
                    "{} went into the pack incomplete ({} of {} bytes).",
                    entry.name,
                    file.size(),
                    expected
                ));
            }
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

/// The half-written zip. Removed when dropped — on an error, and on a panic
/// mid-pack — unless the finished zip was renamed into place.
struct Partial {
    path: PathBuf,
    kept: bool,
}

impl Drop for Partial {
    fn drop(&mut self) {
        if !self.kept {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

pub fn build_pack(
    paths: &[String],
    categories: &[CleanCategory],
    subfolder: Option<&str>,
    head_exports: &[HeadExportChoice],
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
    let mut tmp = Partial {
        path: dir.join(format!(".{}.lineage-partial", file_name.to_string_lossy())),
        kept: false,
    };
    let cleaned = write_pack(&entries, categories, &tmp.path, progress)?;
    verify(&tmp.path, &entries, categories)?;
    std::fs::rename(&tmp.path, dest).map_err(|e| format!("Couldn't save {} ({e})", dest.display()))?;
    tmp.kept = true;
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
    head_exports: Vec<HeadExportChoice>,
    dest: String,
    on_progress: tauri::ipc::Channel<PackProgress>,
) -> Result<PackOutcome, String> {
    tauri::async_runtime::spawn_blocking(move || {
        build_pack(
            &paths,
            &categories,
            subfolder.as_deref(),
            &head_exports,
            Path::new(&dest),
            &mut |p| {
                let _ = on_progress.send(p);
            },
        )
    })
    .await
    .map_err(|e| format!("packing task failed: {e}"))?
}

#[tauri::command]
pub async fn head_exports_for(paths: Vec<String>) -> Result<Vec<HeadExport>, String> {
    tauri::async_runtime::spawn_blocking(move || find_head_exports(&paths))
        .await
        .map_err(|e| format!("head export search failed: {e}"))
}

#[tauri::command]
pub async fn choose_head_export(preset: String, picked: Vec<String>) -> Result<HeadExport, String> {
    tauri::async_runtime::spawn_blocking(move || head_export_from_picked(&preset, &picked))
        .await
        .map_err(|e| format!("head export check failed: {e}"))?
}

#[tauri::command]
pub async fn head_exports_in_folder(folder: String, presets: Vec<String>) -> Result<FolderMatch, String> {
    tauri::async_runtime::spawn_blocking(move || find_head_exports_in(Path::new(&folder), &presets))
        .await
        .map_err(|e| format!("head export search failed: {e}"))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use CleanCategory::*;

    const DEFAULTS: [CleanCategory; 4] = [BodyMorphs, BodyOverlays, Skeleton, WeaponCamera];
    const TATTOOED: &str = r#"{"modNames":["KS Hairdo's.esp"],"overrides":[{"node":"Body [Ovl0]","values":[{"data":"Actors\\Character\\Overlays\\Tattoos\\t.dds","index":0,"key":9,"type":2}]},{"node":"Face [Ovl0]","values":[{"data":"Actors\\Character\\Overlays\\Freckles\\f.dds","index":0,"key":9,"type":2}]}]}"#;
    const PLAIN: &str = "{\r\n    \"modNames\" : [ \"KS Hairdo's.esp\" ]\r\n}\r\n";
    /// Just enough of each for the header check.
    const NIF: &str = "Gamebryo File Format, Version 20.2.0.7\nhead mesh";
    const DDS: &str = "DDS |tint";

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

    fn pack(
        sb: &Sandbox,
        paths: &[String],
        categories: &[CleanCategory],
        sub: Option<&str>,
        heads: &[HeadExportChoice],
    ) -> Result<PackOutcome, String> {
        std::fs::create_dir_all(sb.dest().parent().unwrap()).unwrap();
        build_pack(paths, categories, sub, heads, &sb.dest(), &mut |_| {})
    }

    /// What the page sends for what it shows.
    fn choices(found: &[HeadExport]) -> Vec<HeadExportChoice> {
        found
            .iter()
            .map(|h| HeadExportChoice {
                preset: h.preset.clone(),
                nif: h.nif.as_ref().map(|f| f.path.clone()),
                dds: h.dds.as_ref().map(|f| f.path.clone()),
            })
            .collect()
    }

    fn name_of(file: &Option<ExportFile>) -> Option<&str> {
        file.as_ref().map(|f| f.path.rsplit(['\\', '/']).next().unwrap_or(&f.path))
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
        pack(&sb, &paths, &[], None, &[]).unwrap();
        assert_eq!(
            zip_names(&sb.dest()),
            [
                "SKSE/Plugins/CharGen/Presets/Brynja.jslot",
                "SKSE/Plugins/CharGen/Presets/Cyra.jslot",
                "SKSE/Plugins/CharGen/Presets/Female/Agnes.jslot",
            ]
        );
        pack(&sb, &paths, &[], Some(" ColdSun "), &[]).unwrap();
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
        let outcome = pack(&sb, &[tattooed.clone(), plain.clone()], &DEFAULTS, None, &[]).unwrap();
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
    fn head_exports_are_found_by_exact_name_in_chargen_or_beside_the_preset() {
        let sb = Sandbox::new("find");
        let chargen = r"M\SKSE\Plugins\CharGen";
        let presets = [
            sb.file(&format!(r"{chargen}\Presets\Female\Agnes.jslot"), PLAIN),
            sb.file(&format!(r"{chargen}\Presets\Brynja.jslot"), PLAIN),
            sb.file(r"Loose\Cyra.jslot", PLAIN),
            sb.file(&format!(r"{chargen}\Presets\Dagny.jslot"), PLAIN),
            sb.file(&format!(r"{chargen}\Presets\Eir.jslot"), PLAIN),
        ];
        sb.file(&format!(r"{chargen}\AGNES.NIF"), NIF);
        sb.file(&format!(r"{chargen}\agnes.dds"), DDS);
        // Typed with ".nif" in Export Head's dialog, and a near miss: neither is Brynja.
        sb.file(&format!(r"{chargen}\Brynja.nif.nif"), NIF);
        sb.file(&format!(r"{chargen}\Brynj.nif"), NIF);
        sb.file(r"Loose\Cyra.nif", NIF);
        sb.file(r"Loose\Cyra.dds", DDS);
        sb.file(&format!(r"{chargen}\Dagny.nif"), NIF);
        // CharGen comes first and supplies both, so the stray .dds beside
        // Eir's preset is never paired with Eir's CharGen .nif.
        sb.file(&format!(r"{chargen}\Eir.nif"), NIF);
        sb.file(&format!(r"{chargen}\Presets\Eir.dds"), DDS);

        let found = find_head_exports(&presets);
        let got: Vec<(Option<&str>, Option<&str>)> =
            found.iter().map(|h| (name_of(&h.nif), name_of(&h.dds))).collect();
        assert_eq!(
            got,
            [
                (Some("AGNES.NIF"), Some("agnes.dds")),
                (None, None),
                (Some("Cyra.nif"), Some("Cyra.dds")),
                (Some("Dagny.nif"), None),
                (Some("Eir.nif"), None),
            ]
        );
        assert_eq!(found[0].nif.as_ref().unwrap().size, NIF.len() as u64);
        assert_eq!(found[0].look_in, sb.0.join(chargen).display().to_string());
        assert_eq!(found[2].look_in, sb.0.join("Loose").display().to_string());
    }

    #[test]
    fn a_chosen_export_ships_under_the_presets_name() {
        let sb = Sandbox::new("chosen");
        let redguard = sb.file(r"M\SKSE\Plugins\CharGen\Presets\1-Redguard.jslot", PLAIN);
        // Export Head's dialog was given another name: nothing finds it, so it's chosen.
        let nif = sb.file(r"M\SKSE\Plugins\CharGen\1-Reguard.nif", NIF);
        sb.file(r"M\SKSE\Plugins\CharGen\1-reguard.DDS", DDS);
        assert!(find_head_exports(std::slice::from_ref(&redguard))[0].nif.is_none());

        let chosen = head_export_from_picked(&redguard, std::slice::from_ref(&nif)).unwrap();
        assert_eq!(name_of(&chosen.dds), Some("1-reguard.DDS"), "the .dds of the same name comes along");
        let outcome = pack(&sb, std::slice::from_ref(&redguard), &[], None, &choices(&[chosen])).unwrap();
        assert_eq!(outcome.head_exports, 2);
        assert_eq!(
            zip_names(&sb.dest()),
            [
                "SKSE/Plugins/CharGen/1-Redguard.dds",
                "SKSE/Plugins/CharGen/1-Redguard.nif",
                "SKSE/Plugins/CharGen/Presets/1-Redguard.jslot",
            ]
        );
        assert_eq!(zip_bytes(&sb.dest(), "SKSE/Plugins/CharGen/1-Redguard.nif"), NIF.as_bytes());

        pack(&sb, std::slice::from_ref(&redguard), &[], None, &[]).unwrap();
        assert_eq!(zip_names(&sb.dest()).len(), 1, "none chosen: the preset only");
    }

    #[test]
    fn files_that_arent_a_nif_or_dds_are_refused() {
        let sb = Sandbox::new("notnif");
        let aela = sb.file(r"M\SKSE\Plugins\CharGen\Presets\Aela.jslot", PLAIN);
        let fake = sb.file(r"M\SKSE\Plugins\CharGen\Aela.nif", "not a mesh");
        let dds = sb.file(r"M\SKSE\Plugins\CharGen\Aela.dds", DDS);
        let notes = sb.file(r"M\SKSE\Plugins\CharGen\Aela.txt", "notes");
        let other = sb.file(r"M\SKSE\Plugins\CharGen\Other.dds", DDS);

        assert!(head_export_from_picked(&aela, std::slice::from_ref(&fake)).is_err());
        assert!(head_export_from_picked(&aela, std::slice::from_ref(&notes)).is_err());
        assert!(head_export_from_picked(&aela, &[dds.clone(), other]).is_err(), "two .dds for one preset");
        assert!(head_export_from_picked(&aela, &[]).is_err());
        // A good .dds alone is fine; its fake partner stays behind.
        let alone = head_export_from_picked(&aela, std::slice::from_ref(&dds)).unwrap();
        assert_eq!((name_of(&alone.nif), name_of(&alone.dds)), (None, Some("Aela.dds")));

        // Whatever the page sends is checked again when packing.
        let sent = HeadExportChoice {
            preset: aela.clone(),
            nif: Some(fake.clone()),
            dds: Some(dds),
        };
        let err = pack(&sb, std::slice::from_ref(&aela), &[], None, &[sent]).unwrap_err();
        assert!(err.contains(&fake), "{err}");
        assert!(!sb.dest().exists());
    }

    #[test]
    fn finding_in_a_folder_matches_exact_names_and_leaves_repeats() {
        let sb = Sandbox::new("infolder");
        let preset = |name: &str| sb.file(&format!(r"M\SKSE\Plugins\CharGen\Presets\Female\Nord\{name}.jslot"), PLAIN);
        let (agnes, lily, signe) = (preset("Agnes"), preset("Lily"), preset("Signe"));
        let sculpts = r"M\SKSE\Plugins\CharGen\Head Sculpts\Female";
        sb.file(&format!(r"{sculpts}\Nord\Agnes.nif"), NIF);
        sb.file(&format!(r"{sculpts}\Nord\Agnes.dds"), DDS);
        // Two folders hold a Lily: not guessed at.
        sb.file(&format!(r"{sculpts}\Nord\Lily.nif"), NIF);
        sb.file(&format!(r"{sculpts}\Breton\Lily.nif"), NIF);

        let root = sb.0.join(r"M\SKSE\Plugins\CharGen\Head Sculpts");
        let matched = find_head_exports_in(&root, &[agnes.clone(), lily.clone(), signe]).unwrap();
        assert_eq!(matched.found.len(), 1);
        assert_eq!(matched.found[0].preset, agnes);
        assert_eq!(
            (name_of(&matched.found[0].nif), name_of(&matched.found[0].dds)),
            (Some("Agnes.nif"), Some("Agnes.dds"))
        );
        assert_eq!(matched.ambiguous, [lily]);
        assert!(find_head_exports_in(&sb.0.join("nowhere"), &[]).is_err());
    }

    #[test]
    fn a_shared_head_export_is_packed_once_but_a_clash_stops_the_pack() {
        let sb = Sandbox::new("clash");
        // Same name in two subfolders of one mod: one head export, packed once.
        let female = sb.file(r"M\SKSE\Plugins\CharGen\Presets\Female\Ria.jslot", PLAIN);
        let male = sb.file(r"M\SKSE\Plugins\CharGen\Presets\Male\Ria.jslot", PLAIN);
        sb.file(r"M\SKSE\Plugins\CharGen\Ria.nif", NIF);
        let found = choices(&find_head_exports(&[female.clone(), male.clone()]));
        assert_eq!(pack(&sb, &[female.clone(), male.clone()], &[], None, &found).unwrap().head_exports, 1);

        // Different exports for the two: both would be CharGen/Ria.nif.
        std::fs::remove_file(sb.dest()).unwrap();
        let other = sb.file(r"Elsewhere\Ria.nif", NIF);
        let clash = [
            found[0].clone(),
            HeadExportChoice {
                preset: male.clone(),
                nif: Some(other.clone()),
                dds: None,
            },
        ];
        let err = pack(&sb, &[female.clone(), male.clone()], &[], None, &clash).unwrap_err();
        assert!(err.contains(&other), "{err}");
        assert!(!sb.dest().exists());

        // One preset's lone head and the other's lone tint land on different
        // zip paths, but they're two exports: never shipped as one pair.
        let x = sb.file(r"X\Ria.nif", NIF);
        let y = sb.file(r"Y\Ria.dds", DDS);
        let halves = [
            HeadExportChoice {
                preset: female.clone(),
                nif: Some(x),
                dds: None,
            },
            HeadExportChoice {
                preset: male.clone(),
                nif: None,
                dds: Some(y.clone()),
            },
        ];
        let err = pack(&sb, &[female, male], &[], None, &halves).unwrap_err();
        assert!(err.contains("two head exports") && err.contains(&y), "{err}");
        assert!(!sb.dest().exists());

        // Same file name from two mods: refused, both named, nothing written.
        let a = sb.file(r"A\SKSE\Plugins\CharGen\Presets\Lydia.jslot", PLAIN);
        let b = sb.file(r"B\SKSE\Plugins\CharGen\Presets\LYDIA.jslot", PLAIN);
        let err = pack(&sb, &[a.clone(), b.clone()], &[], None, &[]).unwrap_err();
        assert!(err.contains(&a) && err.contains(&b), "{err}");
        assert!(!sb.dest().exists());
        assert_eq!(std::fs::read_dir(sb.dest().parent().unwrap()).unwrap().count(), 0, "no temp file left");
    }

    #[test]
    fn an_unreadable_preset_or_bad_folder_name_packs_nothing() {
        let sb = Sandbox::new("refuse");
        let good = sb.file(r"M\SKSE\Plugins\CharGen\Presets\Good.jslot", PLAIN);
        let bad = sb.file(r"M\SKSE\Plugins\CharGen\Presets\Bad.jslot", "{ not json");
        let err = pack(&sb, &[good.clone(), bad.clone()], &[], None, &[]).unwrap_err();
        assert!(err.contains(&bad) && !err.contains(&good), "{err}");
        assert_eq!(std::fs::read_dir(sb.dest().parent().unwrap()).unwrap().count(), 0, "no temp file left");

        for name in ["a/b", r"a\b", "..", "Pack.", "what?"] {
            assert!(pack(&sb, std::slice::from_ref(&good), &[], Some(name), &[]).is_err(), "{name}");
        }
    }

    #[test]
    fn a_failed_pack_leaves_an_existing_zip_alone() {
        let sb = Sandbox::new("keep");
        let good = sb.file(r"M\SKSE\Plugins\CharGen\Presets\Good.jslot", PLAIN);
        let bad = sb.file(r"M\SKSE\Plugins\CharGen\Presets\Bad.jslot", "{ not json");
        pack(&sb, std::slice::from_ref(&good), &[], None, &[]).unwrap();
        let before = std::fs::read(sb.dest()).unwrap();
        assert!(pack(&sb, &[good, bad], &[], None, &[]).is_err());
        assert_eq!(std::fs::read(sb.dest()).unwrap(), before);
    }
}
