//! Readiness: will a preset load correctly on *this* setup?
//!
//! Find Assets answers what a preset needs; this answers whether the setup
//! the game will actually load has it. Plugins and textures are checked
//! against real files — a plugin must be installed in an enabled location
//! *and* active in plugins.txt; a texture must exist loose in an enabled
//! location or inside a BSA that loads — so "Missing" is precise. Slider
//! families have no file to find: they're matched through the library to a
//! Nexus mod and looked for among enabled MO2 mods by `meta.ini` modid. Any
//! mod can provide the same sliders, so not finding that one page proves
//! nothing, and a slider family is never worse than "Unconfirmed".
//!
//! Lineage never edits the profile. It reports.

use crate::assets::{self, AssetRef};
use crate::bsa;
use crate::jslot;
use crate::library::{self, Library};
use crate::scan;
use crate::settings::{self, AppSettings, ModManagerKind};
use crate::snapshot::FailedFile;
use serde::Serialize;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

const OFFICIAL_MASTERS: [&str; 5] = [
    "skyrim.esm",
    "update.esm",
    "dawnguard.esm",
    "hearthfires.esm",
    "dragonborn.esm",
];
const PLUGIN_EXTENSIONS: [&str; 3] = ["esp", "esm", "esl"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Missing,
    Unconfirmed,
    Ready,
}

/// One reference, checked.
#[derive(Debug, Clone, Serialize)]
pub struct Check {
    /// "plugin" | "texture" | "morph"
    pub kind: String,
    pub value: String,
    pub status: Status,
    /// Where it was found, or why not: "KS Hairdos", "in disabled mod KS
    /// Hairdos", "installed (KS Hairdos) but not active in plugins.txt".
    pub detail: String,
    /// Where to get it, when the library knows.
    pub source_name: Option<String>,
    pub source_url: Option<String>,
    /// What one fix covers, for grouping a sweep's problems.
    #[serde(skip)]
    cause: Option<CauseKey>,
}

#[derive(Debug, Clone)]
struct CauseKey {
    key: String,
    title: String,
    detail: String,
}

struct Location {
    label: String,
    root: PathBuf,
}

/// Where a texture turned up, in the order they're searched.
enum Found {
    Loaded(String),
    UnloadedBsa(String),
    Disabled(String),
}

/// What the game will load, indexed once.
pub struct Setup {
    /// The profile name, or "your Skyrim Data folder".
    pub label: String,
    mo2: bool,
    enabled: Vec<Location>,
    disabled: Vec<Location>,
    /// Active plugin file names and their stems, lowercased.
    active: HashSet<String>,
    active_stems: HashSet<String>,
    /// BSA file names Skyrim.ini loads by name, lowercased.
    ini_archives: HashSet<String>,
    /// Plugin file name (lowercased) → index into `enabled` / `disabled`.
    plugin_files: HashMap<String, usize>,
    disabled_plugin_files: HashMap<String, usize>,
    /// (in an enabled location?, location index, archive path)
    archives: Vec<(bool, usize, PathBuf)>,
}

fn lower_name(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default()
}

fn extension_is(path: &Path, exts: &[&str]) -> bool {
    path.extension()
        .map(|e| exts.iter().any(|x| e.eq_ignore_ascii_case(x)))
        .unwrap_or(false)
}

/// plugins.txt: `*Name.esp` is active; unstarred lines are installed but off.
fn active_plugins(text: &str) -> HashSet<String> {
    text.lines()
        .filter_map(|l| l.trim().strip_prefix('*'))
        .map(|n| n.trim().to_lowercase())
        .filter(|n| !n.is_empty())
        .collect()
}

/// `sResourceArchiveList` / `sResourceArchiveList2` from a Skyrim.ini.
fn ini_archives(text: &str) -> HashSet<String> {
    text.lines()
        .filter_map(|l| l.split_once('='))
        .filter(|(k, _)| {
            let k = k.trim();
            k.eq_ignore_ascii_case("sResourceArchiveList") || k.eq_ignore_ascii_case("sResourceArchiveList2")
        })
        .flat_map(|(_, v)| v.split(','))
        .map(|n| n.trim().to_lowercase())
        .filter(|n| !n.is_empty())
        .collect()
}

fn my_games_ini() -> Option<PathBuf> {
    let home = std::env::var_os("USERPROFILE")?;
    Some(PathBuf::from(home).join(r"Documents\My Games\Skyrim Special Edition\Skyrim.ini"))
}

fn nexus_mod_id(url: &str) -> Option<u32> {
    let lower = url.to_ascii_lowercase();
    if !lower.contains("nexusmods.com") {
        return None;
    }
    let after = lower.split("/mods/").nth(1)?;
    after
        .split(|c: char| !c.is_ascii_digit())
        .next()?
        .parse()
        .ok()
        .filter(|&id| id > 0)
}

/// The files wanted under a location, and every folder on the way to them,
/// so a walk only descends where a wanted file could be.
struct Wanted {
    dirs: HashSet<String>,
    files: HashSet<String>,
}

impl Wanted {
    fn new<'a>(files: impl Iterator<Item = &'a String>) -> Wanted {
        let mut wanted = Wanted {
            dirs: HashSet::new(),
            files: HashSet::new(),
        };
        for f in files {
            let mut end = f.len();
            while let Some(i) = f[..end].rfind('\\') {
                wanted.dirs.insert(f[..i].to_string());
                end = i;
            }
            wanted.files.insert(f.clone());
        }
        wanted
    }
}

fn walk(dir: &Path, rel: &str, wanted: &Wanted, hit: &mut dyn FnMut(&str)) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_lowercase();
        let child = if rel.is_empty() { name } else { format!("{rel}\\{name}") };
        let Ok(kind) = entry.file_type() else { continue };
        if kind.is_dir() {
            if wanted.dirs.contains(&child) {
                walk(&entry.path(), &child, wanted, hit);
            }
        } else if wanted.files.contains(&child) {
            hit(&child);
        }
    }
}

impl Setup {
    pub fn load(settings: &AppSettings) -> Result<Setup, String> {
        let data = settings.data_dir().filter(|d| d.is_dir());
        let mut enabled = Vec::new();
        let mut disabled = Vec::new();
        let (label, plugins_txt, ini) = match settings.mod_manager {
            ModManagerKind::Mo2 => {
                let profile = settings.mo2_profile_dir.trim();
                if profile.is_empty() {
                    return Err("Set your MO2 profile in Settings first.".to_string());
                }
                let on = scan::enabled_mo2_mods(profile)
                    .ok_or_else(|| format!("Couldn't read modlist.txt in {profile}."))?;
                let mods = std::fs::read_dir(settings.mo2_mods_folder.trim())
                    .map_err(|e| format!("Couldn't read the MO2 mods folder ({e})."))?;
                let mut folders: Vec<(String, PathBuf)> = mods
                    .flatten()
                    .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
                    .map(|e| (e.file_name().to_string_lossy().into_owned(), e.path()))
                    .filter(|(name, _)| !name.to_ascii_lowercase().ends_with("_separator"))
                    .collect();
                folders.sort();
                for (name, root) in folders {
                    let location = Location { label: name.clone(), root };
                    if on.contains(&name.to_ascii_lowercase()) {
                        enabled.push(location);
                    } else {
                        disabled.push(location);
                    }
                }
                let overwrite = Path::new(settings.mo2_instance.trim()).join("overwrite");
                if !settings.mo2_instance.trim().is_empty() && overwrite.is_dir() {
                    enabled.push(Location {
                        label: "Overwrite".to_string(),
                        root: overwrite,
                    });
                }
                let profile = Path::new(profile);
                let profile_ini = profile.join("Skyrim.ini");
                let label = profile
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| profile.display().to_string());
                let ini = if profile_ini.is_file() { Some(profile_ini) } else { my_games_ini() };
                (label, profile.join("plugins.txt"), ini)
            }
            ModManagerKind::Vortex | ModManagerKind::Manual => {
                if data.is_none() {
                    return Err("Set your Skyrim folder in Settings first.".to_string());
                }
                let local = std::env::var_os("LOCALAPPDATA")
                    .ok_or_else(|| "Couldn't find %LOCALAPPDATA%.".to_string())?;
                (
                    "your Skyrim Data folder".to_string(),
                    PathBuf::from(local).join(r"Skyrim Special Edition\plugins.txt"),
                    my_games_ini(),
                )
            }
        };
        if let Some(data) = &data {
            enabled.push(Location {
                label: "Skyrim Data folder".to_string(),
                root: data.clone(),
            });
        }

        let text = std::fs::read_to_string(&plugins_txt)
            .map_err(|e| format!("Couldn't read {} ({e}).", plugins_txt.display()))?;
        let mut active = active_plugins(&text);
        active.extend(OFFICIAL_MASTERS.iter().map(|m| m.to_string()));
        // Creation Club content the game loads on its own.
        if let Some(ccc) = data.as_ref().and_then(|d| d.parent()).map(|g| g.join("Skyrim.ccc")) {
            if let Ok(text) = std::fs::read_to_string(ccc) {
                active.extend(text.lines().map(|l| l.trim().to_lowercase()).filter(|l| !l.is_empty()));
            }
        }
        let active_stems = active
            .iter()
            .map(|p| p.rsplit_once('.').map(|(s, _)| s.to_string()).unwrap_or_else(|| p.clone()))
            .collect();
        let ini_archives = ini
            .and_then(|p| std::fs::read_to_string(p).ok())
            .map(|t| ini_archives(&t))
            .unwrap_or_default();

        let mut setup = Setup {
            label,
            mo2: settings.mod_manager == ModManagerKind::Mo2,
            enabled,
            disabled,
            active,
            active_stems,
            ini_archives,
            plugin_files: HashMap::new(),
            disabled_plugin_files: HashMap::new(),
            archives: Vec::new(),
        };
        for is_enabled in [true, false] {
            let locations = if is_enabled { &setup.enabled } else { &setup.disabled };
            for (i, location) in locations.iter().enumerate() {
                let Ok(entries) = std::fs::read_dir(&location.root) else { continue };
                for entry in entries.flatten() {
                    if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                        continue;
                    }
                    let path = entry.path();
                    if extension_is(&path, &PLUGIN_EXTENSIONS) {
                        let files = if is_enabled {
                            &mut setup.plugin_files
                        } else {
                            &mut setup.disabled_plugin_files
                        };
                        files.entry(lower_name(&path)).or_insert(i);
                    } else if extension_is(&path, &["bsa"]) {
                        setup.archives.push((is_enabled, i, path));
                    }
                }
            }
        }
        Ok(setup)
    }

    /// Whether an archive in an enabled location loads: Skyrim.ini names it,
    /// or an active plugin claims it (`<plugin>.bsa`, `<plugin> - *.bsa`).
    fn loads(&self, archive: &Path) -> bool {
        let name = lower_name(archive);
        if self.ini_archives.contains(&name) {
            return true;
        }
        let stem = name.strip_suffix(".bsa").unwrap_or(&name);
        self.active_stems.contains(stem)
            || stem
                .match_indices(" - ")
                .any(|(i, _)| self.active_stems.contains(&stem[..i]))
    }

    fn location(&self, is_enabled: bool, i: usize) -> &Location {
        if is_enabled { &self.enabled[i] } else { &self.disabled[i] }
    }

    /// Search order: loose in enabled locations, archives that load,
    /// archives that don't, then disabled mods (loose, then archives).
    fn find_textures(&self, wanted: &HashSet<String>) -> HashMap<String, Found> {
        let mut found: HashMap<String, Found> = HashMap::new();
        let remaining = |found: &HashMap<String, Found>| -> HashSet<String> {
            wanted.iter().filter(|w| !found.contains_key(*w)).cloned().collect()
        };
        let search_loose = |locations: &[Location], found: &mut HashMap<String, Found>, disabled: bool| {
            let left = remaining(found);
            if left.is_empty() {
                return;
            }
            let tree = Wanted::new(left.iter());
            for location in locations {
                walk(&location.root, "", &tree, &mut |hit| {
                    let label = location.label.clone();
                    found
                        .entry(hit.to_string())
                        .or_insert_with(|| if disabled { Found::Disabled(label) } else { Found::Loaded(label) });
                });
            }
        };
        search_loose(&self.enabled, &mut found, false);

        let search_archives = |pick: &dyn Fn(bool, &Path) -> bool,
                               make: &dyn Fn(&Location, String) -> Found,
                               found: &mut HashMap<String, Found>| {
            for (is_enabled, i, path) in &self.archives {
                if !pick(*is_enabled, path) {
                    continue;
                }
                let left = remaining(found);
                if left.is_empty() {
                    return;
                }
                let archive = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let location = self.location(*is_enabled, *i);
                // A damaged archive can't hold what we're looking for.
                let _ = bsa::for_each_path(path, &mut |p| {
                    if left.contains(p) && !found.contains_key(p) {
                        found.insert(p.to_string(), make(location, archive.clone()));
                    }
                });
            }
        };
        search_archives(&|on, p| on && self.loads(p), &|_, a| Found::Loaded(a), &mut found);
        search_archives(&|on, p| on && !self.loads(p), &|_, a| Found::UnloadedBsa(a), &mut found);
        search_loose(&self.disabled, &mut found, true);
        search_archives(&|on, _| !on, &|l, _| Found::Disabled(l.label.clone()), &mut found);
        found
    }

    /// Nexus mod id → labels of the enabled and the disabled MO2 mods from it.
    fn mod_ids(&self) -> HashMap<u32, (Vec<String>, Vec<String>)> {
        let mut out: HashMap<u32, (Vec<String>, Vec<String>)> = HashMap::new();
        for (is_enabled, locations) in [(true, &self.enabled), (false, &self.disabled)] {
            for location in locations.iter() {
                if let Some(id) = assets::meta_ini_mod_id(&location.root) {
                    let slot = out.entry(id).or_default();
                    (if is_enabled { &mut slot.0 } else { &mut slot.1 }).push(location.label.clone());
                }
            }
        }
        out
    }

    /// Check each reference once. `refs` may mix kinds; order is kept.
    pub fn check(&self, library: &Library, refs: &[AssetRef]) -> Vec<Check> {
        let source = |kind: &str, value: &str| {
            library
                .match_entry(kind, value, false)
                .map(|m| (m.entry.name.clone(), Some(m.entry.url.clone()).filter(|u| !u.trim().is_empty())))
        };
        let full_texture = |value: &str| format!("textures\\{}", library::normalize_ref(value));
        let wanted: HashSet<String> = refs
            .iter()
            .filter(|r| r.kind == "texture")
            .map(|r| full_texture(&r.value))
            .collect();
        let textures = if wanted.is_empty() { HashMap::new() } else { self.find_textures(&wanted) };
        let mod_ids = if self.mo2 && refs.iter().any(|r| r.kind == "morph") {
            self.mod_ids()
        } else {
            HashMap::new()
        };

        refs.iter()
            .map(|r| {
                let src = source(&r.kind, &r.value);
                let not_found = |key: String, title: String| CauseKey {
                    key: src.as_ref().map(|(n, _)| format!("source|{}", n.to_lowercase())).unwrap_or(key),
                    title: src.as_ref().map(|(n, _)| n.clone()).unwrap_or(title),
                    detail: if src.is_some() {
                        "Not found on this setup.".to_string()
                    } else {
                        "Not found on this setup, and the library doesn't know where it's from.".to_string()
                    },
                };
                let disabled = |label: &str| CauseKey {
                    key: format!("disabled|{}", label.to_lowercase()),
                    title: label.to_string(),
                    detail: "Disabled in MO2.".to_string(),
                };
                let (status, detail, cause) = match r.kind.as_str() {
                    "plugin" => {
                        let name = r.value.trim().to_lowercase();
                        match (self.plugin_files.get(&name), self.active.contains(&name)) {
                            (Some(&i), true) => (Status::Ready, self.enabled[i].label.clone(), None),
                            (Some(&i), false) => (
                                Status::Missing,
                                format!("installed ({}) but not active in plugins.txt", self.enabled[i].label),
                                Some(CauseKey {
                                    key: format!("inactive|{name}"),
                                    title: r.value.trim().to_string(),
                                    detail: "Installed but not active in plugins.txt.".to_string(),
                                }),
                            ),
                            (None, _) => match self.disabled_plugin_files.get(&name) {
                                Some(&i) => {
                                    let label = &self.disabled[i].label;
                                    (Status::Missing, format!("in disabled mod {label}"), Some(disabled(label)))
                                }
                                None => (
                                    Status::Missing,
                                    "not installed".to_string(),
                                    Some(not_found(format!("plugin|{name}"), r.value.trim().to_string())),
                                ),
                            },
                        }
                    }
                    "texture" => match textures.get(&full_texture(&r.value)) {
                        Some(Found::Loaded(from)) => (Status::Ready, from.clone(), None),
                        Some(Found::UnloadedBsa(archive)) => (
                            Status::Missing,
                            format!("inside {archive}, which doesn't load because no active plugin claims it"),
                            Some(CauseKey {
                                key: format!("bsa|{}", archive.to_lowercase()),
                                title: archive.clone(),
                                detail: "An archive that doesn't load: no active plugin claims it.".to_string(),
                            }),
                        ),
                        Some(Found::Disabled(label)) => {
                            (Status::Missing, format!("in disabled mod {label}"), Some(disabled(label)))
                        }
                        None => {
                            let norm = library::normalize_ref(&r.value);
                            let (pattern, _) = assets::texture_group_pattern(&norm);
                            (
                                Status::Missing,
                                "not found".to_string(),
                                Some(not_found(
                                    format!("folder|{pattern}"),
                                    assets::display_pattern(&pattern, &r.value),
                                )),
                            )
                        }
                    },
                    _ => {
                        let family = |detail: &str| {
                            let (key, title) = match &src {
                                Some((name, _)) => (format!("morph|{}", name.to_lowercase()), name.clone()),
                                None => {
                                    let prefix = assets::leading_token_prefix(r.value.trim())
                                        .unwrap_or_else(|| r.value.trim().to_string());
                                    (format!("morph-prefix|{}", prefix.to_lowercase()), prefix)
                                }
                            };
                            Some(CauseKey {
                                key,
                                title,
                                detail: detail.to_string(),
                            })
                        };
                        match &src {
                            _ if !self.mo2 => (
                                Status::Unconfirmed,
                                "slider families can only be checked with MO2".to_string(),
                                family("Slider families can only be checked with MO2."),
                            ),
                            None => (
                                Status::Unconfirmed,
                                "slider family not identified".to_string(),
                                family("Slider family not identified in the library."),
                            ),
                            Some((name, url)) => match url.as_deref().and_then(nexus_mod_id) {
                                None => (
                                    Status::Unconfirmed,
                                    format!("{name} has no Nexus page to match against"),
                                    family("No Nexus page to match against your mods."),
                                ),
                                Some(id) => match mod_ids.get(&id) {
                                    Some((on, _)) if !on.is_empty() => (Status::Ready, on[0].clone(), None),
                                    Some((_, off)) if !off.is_empty() => (
                                        Status::Unconfirmed,
                                        format!("{name} is only in disabled mod {}", off[0]),
                                        family(&format!("Only in disabled mod {}.", off[0])),
                                    ),
                                    _ => (
                                        Status::Unconfirmed,
                                        format!("no enabled mod is from {name}'s page"),
                                        family("No enabled mod is from this page; another mod may provide the sliders."),
                                    ),
                                },
                            },
                        }
                    }
                };
                let (source_name, source_url) = match &src {
                    Some((n, u)) if status != Status::Ready => (Some(n.clone()), u.clone()),
                    _ => (None, None),
                };
                Check {
                    kind: r.kind.clone(),
                    value: r.value.clone(),
                    status,
                    detail,
                    source_name,
                    source_url,
                    cause,
                }
            })
            .collect()
    }
}

#[derive(Debug, Serialize)]
pub struct PresetReadiness {
    pub preset_path: String,
    pub profile: String,
    /// Missing first, then Unconfirmed, then Ready.
    pub checks: Vec<Check>,
    pub parse_error: Option<String>,
}

pub fn check_preset(setup: &Setup, library: &Library, path: &Path) -> PresetReadiness {
    let mut out = PresetReadiness {
        preset_path: path.display().to_string(),
        profile: setup.label.clone(),
        checks: Vec::new(),
        parse_error: None,
    };
    match jslot::parse_file(path) {
        Err(e) => out.parse_error = Some(e),
        Ok(value) => {
            let x = assets::extract(&value);
            let refs: Vec<AssetRef> = x.plugins.into_iter().chain(x.textures).chain(x.morphs).collect();
            out.checks = setup.check(library, &refs);
            out.checks.sort_by_key(|c| c.status);
        }
    }
    out
}

/// Everything one fix covers, and the presets it affects.
#[derive(Debug, Serialize)]
pub struct Cause {
    pub status: Status,
    pub title: String,
    pub detail: String,
    /// The references behind it, as presets spell them.
    pub references: Vec<String>,
    pub presets: Vec<String>,
    pub source_url: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct SweepReport {
    pub profile: String,
    /// Presets read (the unreadable ones are in `unreadable`).
    pub total: usize,
    /// Presets with nothing Missing: every plugin and texture is there.
    pub ready: usize,
    /// Presets with at least one Missing reference. `ready + missing == total`.
    pub missing: usize,
    /// Presets with a slider family that couldn't be confirmed — in either
    /// group above. Counted apart because a family no page confirms is
    /// common and usually harmless; folding it in would bury real problems.
    pub unconfirmed: usize,
    /// The broken-preset check: presets that don't parse at all.
    pub unreadable: Vec<FailedFile>,
    /// Missing causes first, then by how many presets each affects.
    pub causes: Vec<Cause>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReadinessProgress {
    /// "indexing" | "parsing" | "checking"
    pub stage: String,
    pub current: usize,
    pub total: usize,
    pub detail: String,
}

pub fn sweep(
    setup: &Setup,
    library: &Library,
    paths: &[String],
    progress: &mut dyn FnMut(ReadinessProgress),
) -> SweepReport {
    let mut unreadable = Vec::new();
    let mut refs: Vec<AssetRef> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    let mut users: Vec<BTreeSet<usize>> = Vec::new();
    let mut read = vec![false; paths.len()];
    for (i, path) in paths.iter().enumerate() {
        progress(ReadinessProgress {
            stage: "parsing".into(),
            current: i + 1,
            total: paths.len(),
            detail: Path::new(path)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
        });
        let value = match jslot::parse_file(Path::new(path)) {
            Ok(value) => value,
            Err(reason) => {
                unreadable.push(FailedFile {
                    path: path.clone(),
                    reason,
                });
                continue;
            }
        };
        read[i] = true;
        let x = assets::extract(&value);
        for r in x.plugins.into_iter().chain(x.textures).chain(x.morphs) {
            let key = format!("{}|{}", r.kind, r.value.trim().to_lowercase());
            let slot = *index.entry(key).or_insert_with(|| {
                refs.push(r);
                users.push(BTreeSet::new());
                refs.len() - 1
            });
            users[slot].insert(i);
        }
    }

    progress(ReadinessProgress {
        stage: "checking".into(),
        current: 0,
        total: refs.len(),
        detail: String::new(),
    });
    let checks = setup.check(library, &refs);

    let mut missing = vec![false; paths.len()];
    let mut unconfirmed = vec![false; paths.len()];
    let mut causes: Vec<(Cause, BTreeSet<usize>)> = Vec::new();
    let mut cause_index: HashMap<(Status, String), usize> = HashMap::new();
    for (check, presets) in checks.iter().zip(&users) {
        for &p in presets {
            match check.status {
                Status::Missing => missing[p] = true,
                Status::Unconfirmed => unconfirmed[p] = true,
                Status::Ready => {}
            }
        }
        let Some(key) = &check.cause else { continue };
        let slot = *cause_index.entry((check.status, key.key.clone())).or_insert_with(|| {
            causes.push((
                Cause {
                    status: check.status,
                    title: key.title.clone(),
                    detail: key.detail.clone(),
                    references: Vec::new(),
                    presets: Vec::new(),
                    source_url: check.source_url.clone(),
                },
                BTreeSet::new(),
            ));
            causes.len() - 1
        });
        let (cause, affected) = &mut causes[slot];
        cause.references.push(check.value.clone());
        affected.extend(presets.iter().copied());
    }
    let mut causes: Vec<Cause> = causes
        .into_iter()
        .map(|(mut cause, affected)| {
            cause.presets = affected.into_iter().map(|i| paths[i].clone()).collect();
            cause.references.sort_by_key(|r| r.to_lowercase());
            cause
        })
        .collect();
    causes.sort_by(|a, b| {
        a.status
            .cmp(&b.status)
            .then(b.presets.len().cmp(&a.presets.len()))
            .then_with(|| a.title.to_lowercase().cmp(&b.title.to_lowercase()))
    });

    let total = read.iter().filter(|r| **r).count();
    let flagged = |flags: &[bool]| (0..paths.len()).filter(|&i| read[i] && flags[i]).count();
    SweepReport {
        profile: setup.label.clone(),
        total,
        ready: total - flagged(&missing),
        missing: flagged(&missing),
        unconfirmed: flagged(&unconfirmed),
        unreadable,
        causes,
    }
}

#[tauri::command]
pub async fn readiness_for(app: tauri::AppHandle, path: String) -> Result<PresetReadiness, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let settings = settings::load_from_app(&app)?;
        let library = assets::library_for(&app)?;
        let setup = Setup::load(&settings)?;
        Ok(check_preset(&setup, &library, Path::new(&path)))
    })
    .await
    .map_err(|e| format!("readiness task failed: {e}"))?
}

#[tauri::command]
pub async fn readiness_sweep(
    app: tauri::AppHandle,
    on_progress: tauri::ipc::Channel<ReadinessProgress>,
) -> Result<SweepReport, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let settings = settings::load_from_app(&app)?;
        let library = assets::library_for(&app)?;
        let _ = on_progress.send(ReadinessProgress {
            stage: "indexing".into(),
            current: 0,
            total: 0,
            detail: String::new(),
        });
        let setup = Setup::load(&settings)?;
        let paths: Vec<String> = scan::scan_settings(&settings).files.into_iter().map(|f| f.path).collect();
        if paths.is_empty() {
            return Err("No presets were found in your JSLOT locations.".to_string());
        }
        Ok(sweep(&setup, &library, &paths, &mut |p| {
            let _ = on_progress.send(p);
        }))
    })
    .await
    .map_err(|e| format!("readiness task failed: {e}"))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::{LibraryEntry, MergedEntry};

    struct Sandbox(PathBuf);
    impl Sandbox {
        fn file(&self, rel: &str, contents: &[u8]) -> PathBuf {
            let p = self.0.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, contents).unwrap();
            p
        }
    }
    impl Drop for Sandbox {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// An MO2 instance with every case the checker distinguishes.
    fn fake_mo2(tag: &str) -> (Sandbox, AppSettings) {
        let root = std::env::temp_dir().join(format!("lineage-ready-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let sb = Sandbox(root.clone());
        sb.file(r"mo2\mods\Hair Mod\KS.esp", b"");
        sb.file(r"mo2\mods\Hair Mod\textures\actors\character\overlays\hair\h.dds", b"");
        sb.file(r"mo2\mods\Hair Mod\meta.ini", b"[General]\nmodid=12302\n");
        sb.file(r"mo2\mods\Inactive Mod\Inactive.esp", b"");
        sb.file(r"mo2\mods\Off Mod\Off.esp", b"");
        sb.file(r"mo2\mods\Off Mod\textures\off\o.dds", b"");
        sb.file(r"mo2\mods\Off Mod\meta.ini", b"[General]\nmodid=999\n");
        sb.file(r"mo2\mods\Archived\Arch.esp", b"");
        sb.file(r"mo2\mods\Archived\Arch.bsa", &bsa::tests::build(105, &[("textures\\arch", &["A.dds"])]));
        sb.file(
            r"mo2\mods\Archived\Arch - Textures.bsa",
            &bsa::tests::build(105, &[("textures\\arch2", &["b.dds"])]),
        );
        sb.file(
            r"mo2\mods\Unclaimed\Loose.bsa",
            &bsa::tests::build(105, &[("textures\\unclaimed", &["u.dds"])]),
        );
        sb.file(r"mo2\mods\Looks_separator\readme.txt", b"");
        sb.file(r"mo2\overwrite\textures\ow\w.dds", b"");
        sb.file(
            r"mo2\profiles\P\modlist.txt",
            b"+Hair Mod\n+Inactive Mod\n-Off Mod\n+Archived\n+Unclaimed\n-Looks_separator\n",
        );
        sb.file(r"mo2\profiles\P\plugins.txt", b"# comment\n*KS.esp\nInactive.esp\n*Arch.esp\n");
        sb.file(r"mo2\profiles\P\Skyrim.ini", b"[Archive]\nsResourceArchiveList2=Listed.bsa, Other.bsa\n");
        sb.file(r"game\Data\Listed.bsa", &bsa::tests::build(104, &[("textures\\listed", &["l.dds"])]));
        let s = |rel: &str| root.join(rel).display().to_string();
        let settings = AppSettings {
            mod_manager: ModManagerKind::Mo2,
            mo2_instance: s("mo2"),
            mo2_mods_folder: s(r"mo2\mods"),
            mo2_profile_dir: s(r"mo2\profiles\P"),
            skyrim_folder: s("game"),
            ..AppSettings::default()
        };
        (sb, settings)
    }

    fn entry(kind: &str, pattern: &str, name: &str, url: &str) -> MergedEntry {
        MergedEntry {
            entry: LibraryEntry {
                id: format!("seed-{name}"),
                kind: kind.into(),
                pattern: pattern.into(),
                match_type: "prefix".into(),
                name: name.into(),
                url: url.into(),
            },
            source: "seed".into(),
            enabled: true,
        }
    }

    fn library() -> Library {
        Library {
            entries: vec![
                entry("morph", "ECE_", "ECE", "https://www.nexusmods.com/skyrimspecialedition/mods/12302"),
                entry("morph", "OFF_", "Off Sliders", "https://www.nexusmods.com/skyrimspecialedition/mods/999"),
                entry("morph", "PAT_", "Patreon Sliders", "https://www.patreon.com/x"),
                entry("morph", "GONE_", "Gone Sliders", "https://www.nexusmods.com/skyrimspecialedition/mods/5"),
                entry("texture", r"gone\", "Gone Pack", "https://www.nexusmods.com/skyrimspecialedition/mods/6"),
            ],
            warning: None,
        }
    }

    fn r(kind: &str, value: &str) -> AssetRef {
        AssetRef {
            kind: kind.into(),
            value: value.into(),
            appeared_in: Vec::new(),
        }
    }

    #[test]
    fn plugins_textures_and_sliders_are_checked_against_the_profile() {
        let (_sb, settings) = fake_mo2("check");
        let setup = Setup::load(&settings).unwrap();
        assert_eq!(setup.label, "P");
        let refs = [
            r("plugin", "KS.esp"),
            r("plugin", "Inactive.esp"),
            r("plugin", "Off.esp"),
            r("plugin", "Nowhere.esp"),
            r("texture", r"Actors\Character\Overlays\Hair\H.dds"),
            r("texture", r"Data\Textures\ow\w.dds"),
            r("texture", r"arch\a.dds"),
            r("texture", r"arch2\b.dds"),
            r("texture", r"listed\l.dds"),
            r("texture", r"unclaimed\u.dds"),
            r("texture", r"off\o.dds"),
            r("texture", r"gone\g.dds"),
            r("morph", "ECE_Nose"),
            r("morph", "OFF_Jaw"),
            r("morph", "PAT_Lip"),
            r("morph", "GONE_Brow"),
            r("morph", "Mystery"),
        ];
        let checks = setup.check(&library(), &refs);
        let got: Vec<(&str, Status, &str)> =
            checks.iter().map(|c| (c.value.as_str(), c.status, c.detail.as_str())).collect();
        use Status::*;
        assert_eq!(
            got,
            [
                ("KS.esp", Ready, "Hair Mod"),
                ("Inactive.esp", Missing, "installed (Inactive Mod) but not active in plugins.txt"),
                ("Off.esp", Missing, "in disabled mod Off Mod"),
                ("Nowhere.esp", Missing, "not installed"),
                (r"Actors\Character\Overlays\Hair\H.dds", Ready, "Hair Mod"),
                (r"Data\Textures\ow\w.dds", Ready, "Overwrite"),
                (r"arch\a.dds", Ready, "Arch.bsa"),
                (r"arch2\b.dds", Ready, "Arch - Textures.bsa"),
                (r"listed\l.dds", Ready, "Listed.bsa"),
                (
                    r"unclaimed\u.dds",
                    Missing,
                    "inside Loose.bsa, which doesn't load because no active plugin claims it"
                ),
                (r"off\o.dds", Missing, "in disabled mod Off Mod"),
                (r"gone\g.dds", Missing, "not found"),
                ("ECE_Nose", Ready, "Hair Mod"),
                ("OFF_Jaw", Unconfirmed, "Off Sliders is only in disabled mod Off Mod"),
                ("PAT_Lip", Unconfirmed, "Patreon Sliders has no Nexus page to match against"),
                ("GONE_Brow", Unconfirmed, "no enabled mod is from Gone Sliders's page"),
                ("Mystery", Unconfirmed, "slider family not identified"),
            ]
        );
        let gone = checks.iter().find(|c| c.value == r"gone\g.dds").unwrap();
        assert_eq!(gone.source_name.as_deref(), Some("Gone Pack"));
        assert!(checks.iter().filter(|c| c.status == Ready).all(|c| c.source_url.is_none()));
    }

    #[test]
    fn a_sweep_groups_problems_by_cause_and_counts_presets() {
        let (sb, settings) = fake_mo2("sweep");
        let preset = |name: &str, json: &str| {
            sb.file(&format!(r"presets\{name}"), json.as_bytes()).display().to_string()
        };
        let paths = vec![
            preset("ready.jslot", r#"{"modNames":["KS.esp"],"morphs":{"custom":[{"name":"ECE_Nose","value":1}]}}"#),
            preset(
                "off-a.jslot",
                r#"{"modNames":["Off.esp"],"overrides":[{"node":"Face [Ovl0]","values":[{"data":"off\\o.dds","index":0,"key":9,"type":2}]}]}"#,
            ),
            preset("off-b.jslot", r#"{"modNames":["Off.esp","KS.esp"]}"#),
            preset("sliders.jslot", r#"{"morphs":{"custom":[{"name":"PAT_Lip","value":1}]}}"#),
            preset("broken.jslot", "{ not json"),
        ];
        let setup = Setup::load(&settings).unwrap();
        let report = sweep(&setup, &library(), &paths, &mut |_| {});
        // sliders.jslot is ready (nothing missing) and also unconfirmed.
        assert_eq!((report.total, report.ready, report.missing, report.unconfirmed), (4, 2, 2, 1));
        assert_eq!(report.unreadable.len(), 1);
        assert!(report.unreadable[0].path.ends_with("broken.jslot"));

        // Off.esp and its texture are one fix: enable Off Mod.
        let first = &report.causes[0];
        assert_eq!((first.status, first.title.as_str()), (Status::Missing, "Off Mod"));
        assert_eq!(first.references, ["Off.esp", r"off\o.dds"]);
        assert_eq!(first.presets.len(), 2, "a union of presets, never a sum");
        let last = report.causes.last().unwrap();
        assert_eq!((last.status, last.title.as_str()), (Status::Unconfirmed, "Patreon Sliders"));
    }

    #[test]
    fn profile_files_parse() {
        let set = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<HashSet<String>>();
        assert_eq!(active_plugins("# c\n*A.esp\nB.esp\n *C.esm \n*\n"), set(&["a.esp", "c.esm"]));
        assert_eq!(
            ini_archives("[Archive]\nsResourceArchiveList=Skyrim - Misc.bsa, X.bsa\nsResourceArchiveList2 = Y.bsa\nOther=Z.bsa\n"),
            set(&["skyrim - misc.bsa", "x.bsa", "y.bsa"])
        );
        assert_eq!(nexus_mod_id("https://www.nexusmods.com/skyrimspecialedition/mods/12302?tab=files"), Some(12302));
        assert_eq!(nexus_mod_id("https://www.nexusmods.com/skyrim/mods/72955"), Some(72955));
        assert_eq!(nexus_mod_id("https://www.patreon.com/mods/12"), None);
    }
}
