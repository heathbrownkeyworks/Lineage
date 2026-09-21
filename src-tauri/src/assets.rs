//! Find Assets — trace a preset's references back to their source mods.
//!
//! Extraction reads the confirmed JSLOT sections (see NOTES.md): plugin names
//! from `mods` / `modNames` / `headParts[].formIdentifier` / `actor.headTexture`,
//! texture paths from `tintInfo` / `faceTextures` and any `.dds`/`.nif`/`.tri`
//! strings inside `overrides` / `skinOverrides`, and non-vanilla morph names
//! from `morphs.custom`.
//!
//! Resolution order (per spec):
//!   1. Local file + MO2 `meta.ini` mod id (cheapest, no network)
//!   2. Local file + Nexus MD5 lookup
//!   3. Vortex deployment manifest mapping (deployed file → staging mod)
//!   4. Plugin-name heuristics against already-identified mods
//! Anything left goes in the Unknown list — the public Nexus v1 API has no
//! search-by-name endpoint to guess with.

use crate::detect::VORTEX_MANIFEST_NAME;
use crate::jslot;
use crate::nexus::{self, NexusModInfo, RateLimitInfo};
use crate::scan;
use crate::settings::{self, AppSettings, ModManagerKind};
use md5::{Digest, Md5};
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};

const MAX_HASH_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Debug, Clone, Serialize)]
pub struct AssetRef {
    /// "plugin" | "texture" | "morph"
    pub kind: String,
    pub value: String,
    /// Which preset sections referenced it.
    pub appeared_in: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct IdentifiedGroup {
    /// Unique group identity ("nexus:6817" / "folder:ks hairdos" /
    /// "library:seed-cme-morphs"), assigned by `Groups::add`. The visible
    /// fields are NOT unique — two library entries can share a mod id, a name
    /// and a URL (the ECE/CME slider pair are both mod 12302) — so this is the
    /// only safe key for a keyed list in the UI.
    pub key: String,
    pub mod_id: Option<u32>,
    pub nexus: Option<NexusModInfo>,
    /// Local mod folder the assets live in, when known.
    pub mod_folder: Option<String>,
    /// "meta.ini" | "md5" | "vortex-manifest" | "heuristic" | "local-folder"
    pub resolved_by: String,
    pub page_url: Option<String>,
    pub name: Option<String>,
    pub library_source: Option<String>,
    pub assets: Vec<AssetRef>,
}

#[derive(Debug, Serialize)]
pub struct FindAssetsReport {
    pub preset_path: String,
    pub identified: Vec<IdentifiedGroup>,
    pub unknown: Vec<AssetRef>,
    /// Base-game references (vanilla masters, unresolved vanilla-path
    /// textures) — listed so "nothing external" is a visible, honest result.
    pub vanilla: Vec<String>,
    pub api_key_present: bool,
    /// Set when Nexus enrichment stopped early (rate limit, network) —
    /// local identification still ran.
    pub nexus_error: Option<String>,
    pub rate_limit: Option<RateLimitInfo>,
    /// Set when the user's library.json was unreadable — the app ran with
    /// seed entries only for this scan.
    pub library_warning: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FindProgress {
    /// "parsing" | "resolving" | "nexus"
    pub stage: String,
    pub current: usize,
    pub total: usize,
    pub detail: String,
}

// ---------------------------------------------------------------------------
// Extraction
// ---------------------------------------------------------------------------

fn is_vanilla_plugin(name: &str) -> bool {
    let l = name.to_ascii_lowercase();
    matches!(
        l.as_str(),
        "skyrim.esm" | "update.esm" | "dawnguard.esm" | "hearthfires.esm" | "dragonborn.esm"
    ) || (l.starts_with("cc") && (l.ends_with(".esm") || l.ends_with(".esl")))
}

/// Vanilla `Actors\Character\<folder>\` head/body skin folders. A texture
/// sitting directly inside one of these is a base-game skin slot: every skin
/// mod (Fair Skin, Demoniac, Bijin, Tempered Skins, ...) replaces those files
/// *in place* at the same path, so the path identifies nothing. Resolving one
/// would just name whichever skin the scanning machine happens to have
/// installed -- which is why the same preset used to yield different
/// "requirements" on different setups.
const VANILLA_SKIN_FOLDERS: &[&str] = &[
    "female",
    "male",
    "femaleorc",
    "orcmale",
    "argonian",
    "argonianfemale",
    "argonianmale",
    "khajiit",
    "khajiitfemale",
    "khajiitmale",
    "bretonfemale",
    "bretonmale",
    "darkelffemale",
    "darkelfmale",
    "highelffemale",
    "highelfmale",
    "imperialfemale",
    "imperialmale",
    "nordfemale",
    "nordmale",
    "redguardfemale",
    "redguardmale",
    "woodelffemale",
    "woodelfmale",
    "elderfemale",
    "eldermale",
];

/// Vanilla tint-mask folder -- the warpaint / dirt / makeup masks that ship
/// with the game. Mods in here are replacers at the same paths, so like the
/// skin slots they are taste, not a requirement.
const VANILLA_TINTMASK_DIR: &str = r"actors\character\character assets\tintmasks\";

/// True for texture paths every install already satisfies: base-game skin
/// slots and vanilla tint masks. Those are never a requirement worth listing.
///
/// Deliberately *not* a plain `Actors\Character\` prefix test -- mods ship
/// their own subfolders inside that tree (Racial Skin Variance under
/// `Actors\Character\RSV\`, overlay packs under `Character Assets\Overlays\`,
/// pubic-hair mods, complexion packs under `Female\FaceDetails\`) and those
/// are real dependencies. The skin-slot rule therefore demands a known
/// vanilla race folder with the file sitting *directly* inside it.
fn is_vanilla_skin_texture(path: &str) -> bool {
    let p = crate::library::normalize_ref(path);
    if p.starts_with(VANILLA_TINTMASK_DIR) {
        return true;
    }
    let seg: Vec<&str> = p.split('\\').collect();
    seg.len() == 4
        && seg[0] == "actors"
        && seg[1] == "character"
        && VANILLA_SKIN_FOLDERS.contains(&seg[2])
}

/// Record a texture reference -- unless it is a vanilla skin slot or tint
/// mask, which is routed to `vanilla` instead. The preset's reference stays
/// visible there (the same honesty the vanilla-plugin list provides) without
/// ever being resolved to a mod or presented as a requirement.
fn note_texture(
    map: &mut HashMap<String, AssetRef>,
    vanilla: &mut Vec<String>,
    value: &str,
    section: &str,
) {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return;
    }
    if is_vanilla_skin_texture(trimmed) {
        if !vanilla.iter().any(|v| v.eq_ignore_ascii_case(trimmed)) {
            vanilla.push(trimmed.to_string());
        }
        return;
    }
    note_ref(map, "texture", trimmed, section);
}

pub(crate) struct Extracted {
    pub plugins: Vec<AssetRef>,
    pub textures: Vec<AssetRef>,
    pub morphs: Vec<AssetRef>,
    pub vanilla: Vec<String>,
}

fn note_ref(map: &mut HashMap<String, AssetRef>, kind: &str, value: &str, section: &str) {
    let key = value.trim().to_ascii_lowercase();
    if key.is_empty() {
        return;
    }
    let entry = map.entry(key).or_insert_with(|| AssetRef {
        kind: kind.into(),
        value: value.trim().to_string(),
        appeared_in: Vec::new(),
    });
    if !entry.appeared_in.iter().any(|s| s == section) {
        entry.appeared_in.push(section.into());
    }
}

/// Walk `overrides`-style sections for embedded asset paths (string values
/// ending in .dds/.nif/.tri).
fn collect_path_strings(
    value: &Value,
    section: &str,
    out: &mut HashMap<String, AssetRef>,
    vanilla: &mut Vec<String>,
) {
    match value {
        Value::String(s) => {
            let l = s.to_ascii_lowercase();
            if l.ends_with(".dds") || l.ends_with(".nif") || l.ends_with(".tri") {
                note_texture(out, vanilla, s, section);
            }
        }
        Value::Array(items) => {
            for v in items {
                collect_path_strings(v, section, out, vanilla);
            }
        }
        Value::Object(obj) => {
            for v in obj.values() {
                collect_path_strings(v, section, out, vanilla);
            }
        }
        _ => {}
    }
}

pub(crate) fn extract(preset: &Value) -> Extracted {
    let mut plugins: HashMap<String, AssetRef> = HashMap::new();
    let mut textures: HashMap<String, AssetRef> = HashMap::new();
    let mut morphs: HashMap<String, AssetRef> = HashMap::new();
    let mut vanilla: Vec<String> = Vec::new();
    let note_plugin = |map: &mut HashMap<String, AssetRef>,
                           vanilla: &mut Vec<String>,
                           name: &str,
                           section: &str| {
        let name = name.trim();
        if name.is_empty() {
            return;
        }
        if is_vanilla_plugin(name) {
            if !vanilla.iter().any(|v| v.eq_ignore_ascii_case(name)) {
                vanilla.push(name.to_string());
            }
        } else {
            note_ref(map, "plugin", name, section);
        }
    };

    if let Some(mods) = preset.get("mods").and_then(Value::as_array) {
        for m in mods {
            if let Some(name) = m.get("name").and_then(Value::as_str) {
                note_plugin(&mut plugins, &mut vanilla, name, "mods");
            }
        }
    }
    if let Some(names) = preset.get("modNames").and_then(Value::as_array) {
        for n in names {
            if let Some(name) = n.as_str() {
                note_plugin(&mut plugins, &mut vanilla, name, "modNames");
            }
        }
    }
    if let Some(parts) = preset.get("headParts").and_then(Value::as_array) {
        for p in parts {
            if let Some(ident) = p.get("formIdentifier").and_then(Value::as_str) {
                if let Some((plugin, _)) = ident.split_once('|') {
                    note_plugin(&mut plugins, &mut vanilla, plugin, "headParts");
                }
            }
        }
    }
    if let Some(ident) = preset
        .get("actor")
        .and_then(|a| a.get("headTexture"))
        .and_then(Value::as_str)
    {
        if let Some((plugin, _)) = ident.split_once('|') {
            note_plugin(&mut plugins, &mut vanilla, plugin, "actor.headTexture");
        }
    }
    for section in ["tintInfo", "faceTextures"] {
        if let Some(items) = preset.get(section).and_then(Value::as_array) {
            for item in items {
                if let Some(tex) = item.get("texture").and_then(Value::as_str) {
                    note_texture(&mut textures, &mut vanilla, tex, section);
                }
            }
        }
    }
    for section in ["overrides", "skinOverrides"] {
        if let Some(v) = preset.get(section) {
            collect_path_strings(v, section, &mut textures, &mut vanilla);
        }
    }
    if let Some(customs) = preset
        .get("morphs")
        .and_then(|m| m.get("custom"))
        .and_then(Value::as_array)
    {
        for c in customs {
            if let Some(name) = c.get("name").and_then(Value::as_str) {
                note_ref(&mut morphs, "morph", name, "morphs.custom");
            }
        }
    }

    Extracted {
        plugins: sort_refs(plugins),
        textures: sort_refs(textures),
        morphs: sort_refs(morphs),
        vanilla,
    }
}

/// Sort a dedup map into a stable Vec, case-insensitive by value — shared by
/// `extract` and `collect_refs` so both sort identically.
pub(crate) fn sort_refs(map: HashMap<String, AssetRef>) -> Vec<AssetRef> {
    let mut v: Vec<AssetRef> = map.into_values().collect();
    v.sort_by(|a, b| a.value.to_ascii_lowercase().cmp(&b.value.to_ascii_lowercase()));
    v
}

// ---------------------------------------------------------------------------
// Local context: mod folders, meta.ini, Vortex manifest
// ---------------------------------------------------------------------------

pub(crate) struct LocalContext {
    /// Top-level mod folders (MO2 mods dir / Vortex staging), name → path.
    mod_folders: Vec<(String, PathBuf)>,
    data_dir: Option<PathBuf>,
    /// Deployed rel path (lowercase, backslashes) → source mod folder name.
    vortex_map: HashMap<String, String>,
}

pub(crate) fn build_context(settings: &AppSettings) -> LocalContext {
    let mut mod_folders = Vec::new();
    let mods_dir = match settings.mod_manager {
        ModManagerKind::Mo2 => settings.mo2_mods_folder.trim(),
        ModManagerKind::Vortex => settings.vortex_staging_folder.trim(),
        ModManagerKind::Manual => "",
    };
    if !mods_dir.is_empty() {
        if let Ok(entries) = std::fs::read_dir(mods_dir) {
            for entry in entries.flatten() {
                if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                    mod_folders.push((entry.file_name().to_string_lossy().into_owned(), entry.path()));
                }
            }
        }
    }
    let data_dir = settings.data_dir().filter(|d| d.is_dir());

    // Vortex deployment manifest: Data-relative path → source mod folder.
    let mut vortex_map = HashMap::new();
    if settings.mod_manager == ModManagerKind::Vortex {
        if let Some(data) = &data_dir {
            if let Ok(text) = std::fs::read_to_string(data.join(VORTEX_MANIFEST_NAME)) {
                if let Ok(manifest) = serde_json::from_str::<Value>(&text) {
                    if let Some(files) = manifest.get("files").and_then(Value::as_array) {
                        for f in files {
                            let rel = f.get("relPath").and_then(Value::as_str);
                            let source = f.get("source").and_then(Value::as_str);
                            if let (Some(rel), Some(source)) = (rel, source) {
                                vortex_map.insert(
                                    rel.replace('/', "\\").to_ascii_lowercase(),
                                    source.to_string(),
                                );
                            }
                        }
                    }
                }
            }
        }
    }
    LocalContext {
        mod_folders,
        data_dir,
        vortex_map,
    }
}

/// The Nexus mod id from an MO2 meta.ini ([General] modid=), if positive.
pub(crate) fn meta_ini_mod_id(mod_folder: &Path) -> Option<u32> {
    let text = std::fs::read_to_string(mod_folder.join("meta.ini")).ok()?;
    let value = text.lines().find_map(|line| {
        line.trim()
            .strip_prefix("modid")
            .map(str::trim_start)
            .and_then(|rest| rest.strip_prefix('='))
    })?;
    value.trim().parse::<i64>().ok().filter(|id| *id > 0).map(|id| id as u32)
}

/// Vortex staging folders are usually named "<Mod Name>-<modid>-<version>…"
/// (from the Nexus download filename). First standalone number ≥ 2 digits.
fn vortex_folder_mod_id(folder: &str) -> Option<u32> {
    folder
        .split('-')
        .map(str::trim)
        .filter(|part| part.len() >= 2 && part.chars().all(|c| c.is_ascii_digit()))
        .find_map(|part| part.parse::<u32>().ok())
}

fn md5_of_file(path: &Path) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    if meta.len() > MAX_HASH_BYTES {
        return None;
    }
    let mut file = std::fs::File::open(path).ok()?;
    let mut hasher = Md5::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Some(
        hasher
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>(),
    )
}

/// Where one reference resolved locally.
struct LocalHit {
    file: PathBuf,
    mod_folder: Option<String>,
    mod_folder_path: Option<PathBuf>,
    /// True when found under the game Data dir (Vortex-deployed or loose).
    in_data: bool,
    data_rel: Option<String>,
}

fn find_plugin(ctx: &LocalContext, name: &str) -> Option<LocalHit> {
    for (folder, path) in &ctx.mod_folders {
        let candidate = path.join(name);
        if candidate.is_file() {
            return Some(LocalHit {
                file: candidate,
                mod_folder: Some(folder.clone()),
                mod_folder_path: Some(path.clone()),
                in_data: false,
                data_rel: None,
            });
        }
    }
    if let Some(data) = &ctx.data_dir {
        let candidate = data.join(name);
        if candidate.is_file() {
            return Some(LocalHit {
                file: candidate,
                mod_folder: None,
                mod_folder_path: None,
                in_data: true,
                data_rel: Some(name.to_string()),
            });
        }
    }
    None
}

fn find_texture(ctx: &LocalContext, rel: &str) -> Option<LocalHit> {
    let rel_path = Path::new(rel);
    for (folder, path) in &ctx.mod_folders {
        let candidate = path.join("textures").join(rel_path);
        if candidate.is_file() {
            return Some(LocalHit {
                file: candidate,
                mod_folder: Some(folder.clone()),
                mod_folder_path: Some(path.clone()),
                in_data: false,
                data_rel: None,
            });
        }
    }
    if let Some(data) = &ctx.data_dir {
        let candidate = data.join("textures").join(rel_path);
        if candidate.is_file() {
            return Some(LocalHit {
                file: candidate,
                mod_folder: None,
                mod_folder_path: None,
                in_data: true,
                data_rel: Some(format!("textures\\{rel}")),
            });
        }
    }
    None
}

// ---------------------------------------------------------------------------
// The command
// ---------------------------------------------------------------------------

/// One shared accumulator for identified groups, keyed by nexus id or folder.
#[derive(Default)]
struct Groups {
    order: Vec<String>,
    map: HashMap<String, IdentifiedGroup>,
}

impl Groups {
    fn add(&mut self, key: String, make: impl FnOnce() -> IdentifiedGroup, asset: &AssetRef) {
        if !self.map.contains_key(&key) {
            self.order.push(key.clone());
            let mut group = make();
            key.clone_into(&mut group.key);
            self.map.insert(key.clone(), group);
        }
        let group = self.map.get_mut(&key).expect("just inserted");
        if !group
            .assets
            .iter()
            .any(|a| a.kind == asset.kind && a.value.eq_ignore_ascii_case(&asset.value))
        {
            group.assets.push(asset.clone());
        }
    }

    fn into_vec(mut self) -> Vec<IdentifiedGroup> {
        self.order
            .iter()
            .filter_map(|k| self.map.remove(k))
            .collect()
    }
}

fn nexus_url(mod_id: u32) -> String {
    format!("https://www.nexusmods.com/{}/mods/{mod_id}", nexus::GAME_DOMAIN)
}

/// Build (or add to) a `resolved_by: "library"` group for a library hit.
/// When the entry's URL is a Nexus mod page and Nexus is reachable, enrich
/// with `nexus::mod_info` — failures degrade silently, same pattern as
/// `lookup_nexus` in `resolve_refs`. The entry's own `name` always wins over
/// whatever Nexus reports.
fn add_library_group(
    groups: &mut Groups,
    hit: &crate::library::MergedEntry,
    asset: &AssetRef,
    app: Option<&tauri::AppHandle>,
    api_key: &str,
    nexus_down: &mut bool,
    nexus_error: &mut Option<String>,
) {
    let mod_id = crate::library::nexus_mod_id_from_url(&hit.entry.url);
    let info = match (mod_id, *nexus_down, app) {
        (Some(id), false, Some(app)) => match nexus::mod_info(app, api_key, id) {
            Ok(info) => info,
            Err(f) => {
                if f.rate_limited {
                    *nexus_down = true;
                }
                if nexus_error.is_none() {
                    *nexus_error = Some(f.message);
                }
                None
            }
        },
        _ => None,
    };
    groups.add(
        format!("library:{}", hit.entry.id),
        || IdentifiedGroup {
            key: String::new(), // Groups::add fills this in.
            mod_id,
            nexus: info,
            mod_folder: None,
            resolved_by: "library".into(),
            page_url: Some(hit.entry.url.clone()),
            name: Some(hit.entry.name.clone()),
            library_source: Some(hit.source.clone()),
            assets: Vec::new(),
        },
        asset,
    );
}

/// Push `asset` into `unknown` unless a library entry (seed or user; the
/// user-only precedence pass already ran earlier) claims it first.
#[allow(clippy::too_many_arguments)]
fn unknown_or_library(
    groups: &mut Groups,
    unknown: &mut Vec<AssetRef>,
    library: &crate::library::Library,
    asset: &AssetRef,
    app: Option<&tauri::AppHandle>,
    api_key: &str,
    nexus_down: &mut bool,
    nexus_error: &mut Option<String>,
) {
    match library.match_entry(&asset.kind, &asset.value, false) {
        Some(hit) => add_library_group(groups, hit, asset, app, api_key, nexus_down, nexus_error),
        None => unknown.push(asset.clone()),
    }
}

/// Output of the resolution pass: identified groups plus everything left over.
pub(crate) struct ResolveOutput {
    pub identified: Vec<IdentifiedGroup>,
    pub unknown: Vec<AssetRef>,
    pub extra_vanilla: Vec<String>,
    pub api_key_present: bool,
    pub nexus_error: Option<String>,
}

/// Resolve every plugin/texture/morph reference to a source mod. `app` is
/// `None` for offline/aggregate scans — Nexus lookups are skipped in that
/// case but local identification still runs.
///
/// Precedence: (1) a user library entry beats everything, checked before the
/// automatic pipeline even runs; (2) the automatic pipeline (meta.ini, MD5,
/// Vortex manifest, local-folder, plugin-name heuristics) runs unchanged;
/// (3) a seed/any library entry claims refs that would otherwise land in
/// Unknown; (4) after every ref is resolved, any group still missing a
/// `page_url` borrows one from a library entry matching one of its assets.
pub(crate) fn resolve_refs(
    app: Option<&tauri::AppHandle>,
    settings: &AppSettings,
    ctx: &LocalContext,
    library: &crate::library::Library,
    plugins: &[AssetRef],
    textures: &[AssetRef],
    morphs: &[AssetRef],
    progress: &mut dyn FnMut(FindProgress),
) -> ResolveOutput {
    let api_key = settings.nexus_api_key.trim().to_string();
    let api_key_present = !api_key.is_empty();

    let mut groups = Groups::default();
    let mut unknown: Vec<AssetRef> = Vec::new();
    let mut extra_vanilla: Vec<String> = Vec::new();
    let mut nexus_error: Option<String> = None;
    // Once Nexus fails hard (rate limit / network), stop calling it but
    // keep resolving locally. No AppHandle (offline/aggregate scans) means
    // Nexus is never reachable either.
    let mut nexus_down = !api_key_present || app.is_none();

    let lookup_nexus = |app: Option<&tauri::AppHandle>,
                            nexus_down: &mut bool,
                            nexus_error: &mut Option<String>,
                            mod_id: u32|
     -> Option<NexusModInfo> {
        if *nexus_down {
            return None;
        }
        let Some(app) = app else { return None };
        match nexus::mod_info(app, &api_key, mod_id) {
            Ok(info) => info,
            Err(f) => {
                if f.rate_limited {
                    *nexus_down = true;
                }
                if nexus_error.is_none() {
                    *nexus_error = Some(f.message);
                }
                None
            }
        }
    };

    let total = plugins.len() + textures.len();
    let mut current = 0usize;

    // --- plugins + textures share the same resolution pipeline ---------
    let mut unresolved_plugins: Vec<AssetRef> = Vec::new();
    let file_refs = plugins
        .iter()
        .map(|r| (r, true))
        .chain(textures.iter().map(|r| (r, false)));
    for (asset, is_plugin) in file_refs {
        current += 1;
        progress(FindProgress {
            stage: "resolving".into(),
            current,
            total,
            detail: asset.value.clone(),
        });

        // 0. User library override — beats the automatic pipeline entirely.
        if let Some(hit) = library.match_entry(&asset.kind, &asset.value, true) {
            add_library_group(&mut groups, hit, asset, app, &api_key, &mut nexus_down, &mut nexus_error);
            continue;
        }

        let hit = if is_plugin {
            find_plugin(ctx, &asset.value)
        } else {
            find_texture(ctx, &asset.value)
        };
        let Some(hit) = hit else {
            if is_plugin {
                unresolved_plugins.push(asset.clone());
            } else if let Some(hit) = library.match_entry(&asset.kind, &asset.value, false) {
                // A named library entry outranks the vanilla-shape
                // heuristic below. Mods do ship inside the vanilla tree
                // (Racial Skin Variance lives under Actors\Character\RSV\),
                // and an entry means someone decided that path names a
                // real mod.
                add_library_group(
                    &mut groups,
                    hit,
                    asset,
                    app,
                    &api_key,
                    &mut nexus_down,
                    &mut nexus_error,
                );
            } else {
                let normalized_texture = asset.value.to_ascii_lowercase().replace('/', "\\");
                let vanilla_shaped = normalized_texture.starts_with("actors\\character\\")
                    && !normalized_texture.starts_with("actors\\character\\overlays\\");
                if vanilla_shaped {
                    // Not loose anywhere + a base-game character path: almost
                    // certainly ships in the vanilla BSAs. RaceMenu overlays
                    // (Actors\Character\Overlays\...) are excluded — they
                    // never ship in vanilla BSAs, so an unfound one must
                    // still be reported rather than called vanilla.
                    if !extra_vanilla.iter().any(|v| v.eq_ignore_ascii_case(&asset.value)) {
                        extra_vanilla.push(asset.value.clone());
                    }
                } else {
                    unknown.push(asset.clone());
                }
            }
            continue;
        };

        // 1. MO2 meta.ini — cheapest, no network.
        let meta_id = hit
            .mod_folder_path
            .as_deref()
            .filter(|_| settings.mod_manager == ModManagerKind::Mo2)
            .and_then(meta_ini_mod_id);
        if let Some(mod_id) = meta_id {
            let info = lookup_nexus(app, &mut nexus_down, &mut nexus_error, mod_id);
            groups.add(
                format!("nexus:{mod_id}"),
                || IdentifiedGroup {
                    key: String::new(),
                    mod_id: Some(mod_id),
                    nexus: info,
                    mod_folder: hit.mod_folder.clone(),
                    resolved_by: "meta.ini".into(),
                    page_url: Some(nexus_url(mod_id)),
                    name: None,
                    library_source: None,
                    assets: Vec::new(),
                },
                asset,
            );
            continue;
        }

        // 3. Vortex deployment manifest (before hashing: cheaper).
        if hit.in_data && !ctx.vortex_map.is_empty() {
            let rel = hit.data_rel.clone().unwrap_or_default().to_ascii_lowercase();
            if let Some(source) = ctx.vortex_map.get(&rel) {
                let mod_id = vortex_folder_mod_id(source);
                let info = mod_id
                    .and_then(|id| lookup_nexus(app, &mut nexus_down, &mut nexus_error, id));
                let key = mod_id
                    .map(|id| format!("nexus:{id}"))
                    .unwrap_or_else(|| format!("folder:{}", source.to_ascii_lowercase()));
                groups.add(
                    key,
                    || IdentifiedGroup {
                        key: String::new(),
                        mod_id,
                        nexus: info,
                        mod_folder: Some(source.clone()),
                        resolved_by: "vortex-manifest".into(),
                        page_url: mod_id.map(nexus_url),
                        name: None,
                        library_source: None,
                        assets: Vec::new(),
                    },
                    asset,
                );
                continue;
            }
        }

        // 2. MD5 hash + Nexus lookup — the most reliable network path.
        let mut resolved = false;
        if !nexus_down {
            if let Some(app) = app {
                progress(FindProgress {
                    stage: "nexus".into(),
                    current,
                    total,
                    detail: format!("Checking Nexus for {}", asset.value),
                });
                if let Some(hash) = md5_of_file(&hit.file) {
                    match nexus::md5_lookup(app, &api_key, &hash) {
                        Ok(Some(info)) => {
                            let mod_id = info.mod_id;
                            groups.add(
                                format!("nexus:{mod_id}"),
                                || IdentifiedGroup {
                                    key: String::new(),
                                    mod_id: Some(mod_id),
                                    nexus: Some(info),
                                    mod_folder: hit.mod_folder.clone(),
                                    resolved_by: "md5".into(),
                                    page_url: Some(nexus_url(mod_id)),
                                    name: None,
                                    library_source: None,
                                    assets: Vec::new(),
                                },
                                asset,
                            );
                            resolved = true;
                        }
                        Ok(None) => {}
                        Err(f) => {
                            if f.rate_limited {
                                nexus_down = true;
                            }
                            if nexus_error.is_none() {
                                nexus_error = Some(f.message);
                            }
                        }
                    }
                }
            }
        }
        if resolved {
            continue;
        }

        // Local attribution without Nexus: still valuable.
        match &hit.mod_folder {
            Some(folder) => {
                groups.add(
                    format!("folder:{}", folder.to_ascii_lowercase()),
                    || IdentifiedGroup {
                        key: String::new(),
                        mod_id: None,
                        nexus: None,
                        mod_folder: Some(folder.clone()),
                        resolved_by: "local-folder".into(),
                        page_url: None,
                        name: None,
                        library_source: None,
                        assets: Vec::new(),
                    },
                    asset,
                );
            }
            None => unknown_or_library(
                &mut groups,
                &mut unknown,
                library,
                asset,
                app,
                &api_key,
                &mut nexus_down,
                &mut nexus_error,
            ),
        }
    }

    // 4. Plugin-name heuristics: match unfound plugins against
    // already-identified mod folders by name.
    for asset in unresolved_plugins {
        let stem = asset
            .value
            .rsplit_once('.')
            .map(|(s, _)| s)
            .unwrap_or(&asset.value)
            .to_ascii_lowercase();
        let matched_key = groups
            .map
            .iter()
            .find(|(_, g)| {
                g.mod_folder
                    .as_deref()
                    .map(|f| {
                        let f = f.to_ascii_lowercase();
                        f.contains(&stem) || stem.contains(&f)
                    })
                    .unwrap_or(false)
            })
            .map(|(k, _)| k.clone());
        match matched_key {
            Some(key) => groups.add(key, || unreachable!("existing key"), &asset),
            None => unknown_or_library(
                &mut groups,
                &mut unknown,
                library,
                &asset,
                app,
                &api_key,
                &mut nexus_down,
                &mut nexus_error,
            ),
        }
    }

    // Morphs resolve to sliders, not files — a user override, then a
    // seed/any library entry, else Unknown.
    for asset in morphs {
        if let Some(hit) = library.match_entry(&asset.kind, &asset.value, true) {
            add_library_group(&mut groups, hit, asset, app, &api_key, &mut nexus_down, &mut nexus_error);
            continue;
        }
        unknown_or_library(
            &mut groups,
            &mut unknown,
            library,
            asset,
            app,
            &api_key,
            &mut nexus_down,
            &mut nexus_error,
        );
    }

    // Link attachment: any group the automatic pipeline identified but
    // couldn't give a page link (e.g. local-folder) borrows one from a
    // library entry matching one of its assets. resolved_by/mod_id/nexus
    // stay untouched — only the link fields change.
    for key in groups.order.clone() {
        let group = groups.map.get_mut(&key).expect("key from order");
        if group.page_url.is_some() {
            continue;
        }
        if let Some(hit) = group
            .assets
            .iter()
            .find_map(|a| library.match_entry(&a.kind, &a.value, false))
        {
            group.page_url = Some(hit.entry.url.clone());
            group.name = Some(hit.entry.name.clone());
            group.library_source = Some(hit.source.clone());
        }
    }

    ResolveOutput {
        identified: groups.into_vec(),
        unknown,
        extra_vanilla,
        api_key_present,
        nexus_error,
    }
}

/// Test entry point: the resolver with Nexus disabled (no AppHandle needed).
#[cfg(test)]
pub(crate) fn resolve_refs_offline(
    settings: &AppSettings,
    ctx: &LocalContext,
    library: &crate::library::Library,
    plugins: &[AssetRef],
    textures: &[AssetRef],
    morphs: &[AssetRef],
) -> ResolveOutput {
    resolve_refs(None, settings, ctx, library, plugins, textures, morphs, &mut |_| {})
}

/// Analyze a preset and resolve every reference to a source mod.
#[tauri::command]
pub async fn find_assets(
    app: tauri::AppHandle,
    path: String,
    on_progress: tauri::ipc::Channel<FindProgress>,
) -> Result<FindAssetsReport, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let settings = settings::load_from_app(&app)?;

        let _ = on_progress.send(FindProgress {
            stage: "parsing".into(),
            current: 0,
            total: 0,
            detail: "Reading preset".into(),
        });
        let preset = jslot::parse_file(Path::new(&path))?;
        let extracted = extract(&preset);
        let ctx = build_context(&settings);

        let config_dir = {
            use tauri::Manager;
            app.path()
                .app_config_dir()
                .map_err(|e| format!("failed to resolve app config dir: {e}"))?
        };
        let library = crate::library::load_from_dir(&config_dir);

        let mut progress = |p: FindProgress| {
            let _ = on_progress.send(p);
        };
        let output = resolve_refs(
            Some(&app),
            &settings,
            &ctx,
            &library,
            &extracted.plugins,
            &extracted.textures,
            &extracted.morphs,
            &mut progress,
        );

        let mut vanilla = extracted.vanilla;
        vanilla.extend(output.extra_vanilla);

        Ok(FindAssetsReport {
            preset_path: path,
            identified: output.identified,
            unknown: output.unknown,
            vanilla,
            api_key_present: output.api_key_present,
            nexus_error: output.nexus_error,
            rate_limit: nexus::get_rate_limit(),
            library_warning: library.warning,
        })
    })
    .await
    .map_err(|e| format!("find assets task failed: {e}"))?
}

// ---------------------------------------------------------------------------
// Collection Review — every preset across the configured roots, at once
// ---------------------------------------------------------------------------

/// Everything the parse phase produces; cached for `review_refresh`.
#[derive(Clone)]
pub(crate) struct CollectedRefs {
    pub total_presets: usize,
    pub parse_failures: usize,
    pub plugins: Vec<AssetRef>,
    pub textures: Vec<AssetRef>,
    pub morphs: Vec<AssetRef>,
    pub vanilla: Vec<String>,
    /// "kind|lowercased value" → number of distinct presets referencing it.
    pub preset_counts: HashMap<String, usize>,
}

/// Fold one preset's refs (already deduped within that preset by `extract`)
/// into the shared accumulator map — same lowercase-key merge as `note_ref`,
/// unioning `appeared_in` — and bump `preset_counts` once per ref, which is
/// exactly once per (preset, ref) since `refs` never repeats a value.
fn fold_preset_refs(
    map: &mut HashMap<String, AssetRef>,
    refs: &[AssetRef],
    preset_counts: &mut HashMap<String, usize>,
) {
    for r in refs {
        let key = r.value.trim().to_ascii_lowercase();
        if key.is_empty() {
            continue;
        }
        let entry = map.entry(key.clone()).or_insert_with(|| AssetRef {
            kind: r.kind.clone(),
            value: r.value.clone(),
            appeared_in: Vec::new(),
        });
        for section in &r.appeared_in {
            if !entry.appeared_in.iter().any(|s| s == section) {
                entry.appeared_in.push(section.clone());
            }
        }
        *preset_counts.entry(format!("{}|{key}", r.kind)).or_insert(0) += 1;
    }
}

/// Parse every configured preset (via `scan::scan_settings`), folding all
/// references into one deduped union with a per-preset reference count for
/// the review UI. Parse failures are counted, not fatal.
pub(crate) fn collect_refs(
    settings: &AppSettings,
    progress: &mut dyn FnMut(FindProgress),
) -> CollectedRefs {
    let scanned = scan::scan_settings(settings);
    let total_presets = scanned.files.len();
    let mut parse_failures = 0usize;

    let mut plugins: HashMap<String, AssetRef> = HashMap::new();
    let mut textures: HashMap<String, AssetRef> = HashMap::new();
    let mut morphs: HashMap<String, AssetRef> = HashMap::new();
    let mut vanilla: Vec<String> = Vec::new();
    let mut preset_counts: HashMap<String, usize> = HashMap::new();

    for (i, file) in scanned.files.iter().enumerate() {
        progress(FindProgress {
            stage: "parsing".into(),
            current: i + 1,
            total: total_presets,
            detail: file.file_name.clone(),
        });
        let preset = match jslot::parse_file(Path::new(&file.path)) {
            Ok(v) => v,
            Err(_) => {
                parse_failures += 1;
                continue;
            }
        };
        let extracted = extract(&preset);
        fold_preset_refs(&mut plugins, &extracted.plugins, &mut preset_counts);
        fold_preset_refs(&mut textures, &extracted.textures, &mut preset_counts);
        fold_preset_refs(&mut morphs, &extracted.morphs, &mut preset_counts);
        for v in extracted.vanilla {
            if !vanilla.iter().any(|x| x.eq_ignore_ascii_case(&v)) {
                vanilla.push(v);
            }
        }
    }

    CollectedRefs {
        total_presets,
        parse_failures,
        plugins: sort_refs(plugins),
        textures: sort_refs(textures),
        morphs: sort_refs(morphs),
        vanilla,
        preset_counts,
    }
}

/// Cache of the last `review_collection` run's parsed refs, so
/// `review_refresh` can re-resolve (e.g. after a library edit) without
/// re-parsing every preset on disk.
static REVIEW_CACHE: std::sync::OnceLock<std::sync::Mutex<Option<CollectedRefs>>> =
    std::sync::OnceLock::new();

fn review_cache() -> &'static std::sync::Mutex<Option<CollectedRefs>> {
    REVIEW_CACHE.get_or_init(|| std::sync::Mutex::new(None))
}

// ---------------------------------------------------------------------------
// Unknown grouping
// ---------------------------------------------------------------------------

/// Path segments every mod shares — walked past when deciding where a
/// texture's own folder begins. `Data\` and `Textures\` are here because some
/// presets spell a path with them and some without, for the same mod.
const GENERIC_PATH_SEGMENTS: &[&str] = &[
    "actors",
    "character",
    "character assets",
    "overlays",
    "data",
    "textures",
];

/// One library entry's worth of unknown references.
///
/// The Unknown list is flat — one row per path — and a mod that ships forty
/// overlay textures is forty rows, each needing its own name and URL. Almost
/// all of them share a folder (or, for loose files, a filename prefix), and
/// one `prefix` entry claims the lot, so grouping turns the queue from
/// roughly a thousand decisions into a couple of hundred.
#[derive(Debug, Clone, Serialize)]
pub struct UnknownGroup {
    /// Stable identity for a keyed list in the UI. `pattern` is not unique on
    /// its own — a texture and a morph could in principle derive the same one.
    pub key: String,
    /// "texture" | "plugin" | "morph"
    pub kind: String,
    /// The library pattern this group would create, in the author's own
    /// capitalization.
    pub pattern: String,
    /// "prefix" | "exact"
    pub match_type: String,
    pub assets: Vec<AssetRef>,
}


/// The leading token of a name, separator included — `empyreancs_` from
/// `empyreancs_f_10_a`, `EXPR_` from `EXPR_BrowWidth`. Mods that drop files
/// loose into a shared folder still prefix every one of them, and morph names
/// carry their slider family the same way, so this is the only attribution
/// handle those two cases offer.
pub(crate) fn leading_token_prefix(name: &str) -> Option<String> {
    let idx = name.find(['_', '-'])?;
    let token = &name[..idx];
    let usable = token.len() >= 3 && token.chars().all(|c| c.is_ascii_alphanumeric());
    usable.then(|| name[..=idx].to_string())
}

/// Where a texture's group starts: the first segment that is a mod's own
/// folder rather than a container. When every folder is generic the file is
/// sitting loose and its filename prefix is used instead.
pub(crate) fn texture_group_pattern(norm: &str) -> (String, &'static str) {
    let seg: Vec<&str> = norm.split('\\').collect();
    let (folders, file) = seg.split_at(seg.len() - 1);
    for (i, s) in folders.iter().enumerate() {
        if !GENERIC_PATH_SEGMENTS.contains(s) {
            return (format!("{}\\", folders[..=i].join("\\")), "prefix");
        }
    }
    let stem = file[0].split('.').next().unwrap_or(file[0]);
    match leading_token_prefix(stem) {
        Some(prefix) if folders.is_empty() => (prefix, "prefix"),
        Some(prefix) => (format!("{}\\{prefix}", folders.join("\\")), "prefix"),
        None => (norm.to_string(), "exact"),
    }
}

fn strip_prefix_ci<'a>(s: &'a str, prefix: &str) -> &'a str {
    if s.len() >= prefix.len() && s[..prefix.len()].eq_ignore_ascii_case(prefix) {
        &s[prefix.len()..]
    } else {
        s
    }
}

/// Recover the author's capitalization for a pattern derived from a
/// lowercased path, so a grouped entry reads like the hand-written ones
/// (`Actors\Character\Overlays\Koralina_Male\`, not all lowercase). Matching
/// is case-insensitive either way; this is purely how it reads.
pub(crate) fn display_pattern(lower_pattern: &str, sample: &str) -> String {
    let cased = sample.trim().replace('/', "\\");
    let cased = cased.trim_start_matches('\\');
    let cased = strip_prefix_ci(cased, "data\\");
    let cased = strip_prefix_ci(cased, "textures\\");
    let n = lower_pattern.len();
    if cased.len() >= n && cased.is_char_boundary(n) && cased[..n].eq_ignore_ascii_case(lower_pattern)
    {
        cased[..n].to_string()
    } else {
        lower_pattern.to_string()
    }
}

/// Collapse the flat Unknown list into the smallest set of library entries
/// that would cover it. Plugin names have no shared structure to exploit, so
/// each stays its own single-asset group.
pub(crate) fn group_unknown(unknown: &[AssetRef]) -> Vec<UnknownGroup> {
    let mut groups: Vec<UnknownGroup> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    for asset in unknown {
        let (pattern, match_type) = match asset.kind.as_str() {
            "texture" => texture_group_pattern(&crate::library::normalize_ref(&asset.value)),
            "morph" => match leading_token_prefix(asset.value.trim()) {
                Some(prefix) => (prefix.to_ascii_lowercase(), "prefix"),
                None => (asset.value.trim().to_ascii_lowercase(), "exact"),
            },
            _ => (asset.value.trim().to_ascii_lowercase(), "exact"),
        };
        let key = format!("{}|{pattern}", asset.kind);
        let slot = *index.entry(key.clone()).or_insert_with(|| {
            groups.push(UnknownGroup {
                key,
                kind: asset.kind.clone(),
                pattern: display_pattern(&pattern, &asset.value),
                match_type: match_type.into(),
                assets: Vec::new(),
            });
            groups.len() - 1
        });
        groups[slot].assets.push(asset.clone());
    }
    // Biggest wins first; ties broken by pattern so the order is stable.
    groups.sort_by(|a, b| {
        b.assets
            .len()
            .cmp(&a.assets.len())
            .then_with(|| a.pattern.to_ascii_lowercase().cmp(&b.pattern.to_ascii_lowercase()))
    });
    for g in &mut groups {
        g.assets
            .sort_by(|a, b| a.value.to_ascii_lowercase().cmp(&b.value.to_ascii_lowercase()));
    }
    groups
}

#[derive(Debug, Serialize)]
pub struct CollectionReport {
    pub total_presets: usize,
    pub parse_failures: usize,
    pub identified: Vec<IdentifiedGroup>,
    pub unknown: Vec<AssetRef>,
    /// `unknown`, collapsed into the library entries that would cover it.
    pub unknown_groups: Vec<UnknownGroup>,
    pub vanilla: Vec<String>,
    /// "kind|lowercased value" → number of presets referencing it.
    pub preset_counts: HashMap<String, usize>,
    pub api_key_present: bool,
    pub nexus_error: Option<String>,
    pub library_warning: Option<String>,
}

/// Build the response both `review_collection` and `review_refresh` return —
/// shared so the two response shapes can't drift apart.
fn assemble(
    collected: &CollectedRefs,
    output: ResolveOutput,
    library_warning: Option<String>,
) -> CollectionReport {
    let mut vanilla = collected.vanilla.clone();
    vanilla.extend(output.extra_vanilla);
    CollectionReport {
        total_presets: collected.total_presets,
        parse_failures: collected.parse_failures,
        identified: output.identified,
        unknown_groups: group_unknown(&output.unknown),
        unknown: output.unknown,
        vanilla,
        preset_counts: collected.preset_counts.clone(),
        api_key_present: output.api_key_present,
        nexus_error: output.nexus_error,
        library_warning,
    }
}

/// Load the Asset Library from the app config dir — shared by both commands.
pub(crate) fn library_for(app: &tauri::AppHandle) -> Result<crate::library::Library, String> {
    use tauri::Manager;
    let config_dir = app
        .path()
        .app_config_dir()
        .map_err(|e| format!("failed to resolve app config dir: {e}"))?;
    Ok(crate::library::load_from_dir(&config_dir))
}

/// Parse every configured preset and resolve every reference — the full
/// Collection Review run. Caches the extracted refs so `review_refresh` can
/// re-resolve without re-parsing every preset.
#[tauri::command]
pub async fn review_collection(
    app: tauri::AppHandle,
    on_progress: tauri::ipc::Channel<FindProgress>,
) -> Result<CollectionReport, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let settings = settings::load_from_app(&app)?;
        let mut progress = |p: FindProgress| {
            let _ = on_progress.send(p);
        };
        let collected = collect_refs(&settings, &mut progress);
        *review_cache().lock().unwrap() = Some(collected.clone());

        let ctx = build_context(&settings);
        let library = library_for(&app)?;
        let output = resolve_refs(
            Some(&app),
            &settings,
            &ctx,
            &library,
            &collected.plugins,
            &collected.textures,
            &collected.morphs,
            &mut progress,
        );
        Ok(assemble(&collected, output, library.warning))
    })
    .await
    .map_err(|e| format!("collection review task failed: {e}"))?
}

/// Re-resolve the refs cached by the last `review_collection` run — no disk
/// re-scan, just resolution (e.g. after the user edits the Asset Library).
#[tauri::command]
pub async fn review_refresh(
    app: tauri::AppHandle,
    on_progress: tauri::ipc::Channel<FindProgress>,
) -> Result<CollectionReport, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let settings = settings::load_from_app(&app)?;
        let collected = review_cache()
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| "Run a collection review first.".to_string())?;

        let ctx = build_context(&settings);
        let library = library_for(&app)?;
        let mut progress = |p: FindProgress| {
            let _ = on_progress.send(p);
        };
        let output = resolve_refs(
            Some(&app),
            &settings,
            &ctx,
            &library,
            &collected.plugins,
            &collected.textures,
            &collected.morphs,
            &mut progress,
        );
        Ok(assemble(&collected, output, library.warning))
    })
    .await
    .map_err(|e| format!("collection refresh task failed: {e}"))?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collection_aggregation_dedupes_and_counts() {
        let dir = std::env::temp_dir().join(format!("lineage-collect-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let presets = dir.join("presets");
        std::fs::create_dir_all(&presets).unwrap();
        let preset = |plugins: &[&str]| {
            let mods: Vec<String> = plugins.iter().enumerate()
                .map(|(i, p)| format!(r#"{{"index": {i}, "name": "{p}"}}"#)).collect();
            format!(r#"{{"mods": [{}]}}"#, mods.join(","))
        };
        std::fs::write(presets.join("a.jslot"), preset(&["Shared.esp", "OnlyA.esp"])).unwrap();
        std::fs::write(presets.join("b.jslot"), preset(&["Shared.esp"])).unwrap();
        std::fs::write(presets.join("broken.jslot"), "{ nope").unwrap();

        let settings = crate::settings::AppSettings {
            jslot_roots: vec![crate::settings::JslotRoot {
                id: "t".into(), label: "T".into(),
                path: presets.display().to_string(),
                kind: crate::settings::RootKind::Plain,
            }],
            ..Default::default()
        };
        let collected = collect_refs(&settings, &mut |_| {});
        assert_eq!(collected.total_presets, 3);
        assert_eq!(collected.parse_failures, 1);
        // Dedup: Shared.esp appears once with preset_count 2.
        assert_eq!(collected.preset_counts.get("plugin|shared.esp"), Some(&2));
        assert_eq!(collected.preset_counts.get("plugin|onlya.esp"), Some(&1));
        let shared = collected.plugins.iter().filter(|r| r.value.eq_ignore_ascii_case("Shared.esp")).count();
        assert_eq!(shared, 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn extraction_reads_confirmed_sections() {
        let preset: Value = serde_json::from_str(
            r#"{
                "actor": {"headTexture": "SomeSkin.esp|00A123"},
                "mods": [{"index": 0, "name": "Skyrim.esm"}, {"index": 1, "name": "KS Hairdo's.esp"}],
                "modNames": ["High Poly Head.esm"],
                "headParts": [{"formId": 1, "formIdentifier": "High Poly Head.esm|001234", "type": 3}],
                "tintInfo": [{"color": 1, "index": 0, "texture": "Actors\\Character\\Custom\\tint.dds"}],
                "overrides": [{"node": "Body", "values": [{"data": "custom\\thing.dds", "index": -1, "key": 9, "type": 9}]}],
                "morphs": {"custom": [{"name": "EFM_Brow_Width", "value": 1.0}]}
            }"#,
        )
        .unwrap();
        let e = extract(&preset);
        let plugin_names: Vec<&str> = e.plugins.iter().map(|p| p.value.as_str()).collect();
        assert!(plugin_names.contains(&"KS Hairdo's.esp"));
        assert!(plugin_names.contains(&"High Poly Head.esm"));
        assert!(plugin_names.contains(&"SomeSkin.esp"));
        assert!(!plugin_names.iter().any(|p| p.eq_ignore_ascii_case("Skyrim.esm")));
        assert_eq!(e.vanilla, vec!["Skyrim.esm"]);
        let tex: Vec<&str> = e.textures.iter().map(|t| t.value.as_str()).collect();
        assert!(tex.contains(&"Actors\\Character\\Custom\\tint.dds"));
        assert!(tex.contains(&"custom\\thing.dds"));
        assert_eq!(e.morphs.len(), 1);
        // headParts + modNames both list High Poly Head.esm — merged, both sections noted.
        let hph = e
            .plugins
            .iter()
            .find(|p| p.value == "High Poly Head.esm")
            .unwrap();
        assert!(hph.appeared_in.contains(&"modNames".to_string()));
        assert!(hph.appeared_in.contains(&"headParts".to_string()));
    }

    #[test]
    fn vortex_folder_names_yield_mod_ids() {
        assert_eq!(vortex_folder_mod_id("The Pure-20583-3-1-1612345678"), Some(20583));
        assert_eq!(vortex_folder_mod_id("KS Hairdos SSE-6817-1-9"), Some(6817));
        assert_eq!(vortex_folder_mod_id("Plain Folder"), None);
    }

    #[test]
    fn meta_ini_ids_parse_and_reject_non_nexus() {
        let dir = std::env::temp_dir().join(format!("lineage-meta-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("meta.ini"), "[General]\ngameName=SkyrimSE\nmodid=47828\n").unwrap();
        assert_eq!(meta_ini_mod_id(&dir), Some(47828));
        std::fs::write(dir.join("meta.ini"), "[General]\nmodid=-1\n").unwrap();
        assert_eq!(meta_ini_mod_id(&dir), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn library_precedence_user_overrides_seed_fills_and_attaches() {
        use crate::library::{Library, LibraryEntry, MergedEntry};
        use crate::settings::AppSettings;

        let dir = std::env::temp_dir().join(format!("lineage-resolve-lib-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // A mod folder providing one plugin → automatic local-folder attribution.
        let mods = dir.join("mods");
        std::fs::create_dir_all(mods.join("Some Hair Mod")).unwrap();
        std::fs::write(mods.join("Some Hair Mod").join("Hair.esp"), b"x").unwrap();

        let settings = AppSettings {
            mod_manager: crate::settings::ModManagerKind::Mo2,
            mo2_mods_folder: mods.display().to_string(),
            ..Default::default()
        };
        let ctx = build_context(&settings);

        let mk = |id: &str, source: &str, kind: &str, pattern: &str, match_type: &str, name: &str| MergedEntry {
            entry: LibraryEntry {
                id: id.into(), kind: kind.into(), pattern: pattern.into(),
                match_type: match_type.into(), name: name.into(),
                url: format!("https://example.com/{id}"),
            },
            source: source.into(),
            enabled: true,
        };
        let library = Library {
            entries: vec![
                // User override for the plugin the pipeline WOULD identify locally.
                mk("user-hair", "user", "plugin", "Hair.esp", "exact", "Corrected Hair Mod"),
                // Seed for an otherwise-unknown morph family.
                mk("seed-efm", "seed", "morph", "EFM_", "prefix", "Expressive Facegen Morphs"),
                // Seed matching the SAME plugin — must NOT override the user entry.
                mk("seed-hair", "seed", "plugin", "Hair.esp", "exact", "Wrong Seed Name"),
                // Seed that attaches a link to a URL-less local-folder group.
                mk("seed-other", "seed", "plugin", "Other.esp", "exact", "Other Mod"),
                // Seed that claims a plugin only after heuristics fail to place it.
                mk("seed-only-plugin", "seed", "plugin", "SeedOnly.esp", "exact", "Seed Only Mod"),
            ],
            warning: None,
        };

        std::fs::write(mods.join("Some Hair Mod").join("Other.esp"), b"y").unwrap();
        let refs = |kind: &str, value: &str| AssetRef {
            kind: kind.into(), value: value.into(), appeared_in: vec!["test".into()],
        };
        let plugins = vec![
            refs("plugin", "Hair.esp"),
            refs("plugin", "Other.esp"),
            refs("plugin", "Missing.esp"),
            refs("plugin", "SeedOnly.esp"),
        ];
        let morphs = vec![refs("morph", "EFM_Brow_Width"), refs("morph", "Totally_Custom")];

        // No API key: app handle unused on the no-network path — pass via the
        // test-only entry point resolve_refs_offline.
        let out = resolve_refs_offline(&settings, &ctx, &library, &plugins, &[], &morphs);

        // 1. User entry wins for Hair.esp even though local attribution existed.
        let hair = out.identified.iter().find(|g| g.assets.iter().any(|a| a.value == "Hair.esp")).unwrap();
        assert_eq!(hair.resolved_by, "library");
        assert_eq!(hair.name.as_deref(), Some("Corrected Hair Mod"));
        assert_eq!(hair.library_source.as_deref(), Some("user"));
        assert_eq!(hair.page_url.as_deref(), Some("https://example.com/user-hair"));

        // 2. Seed claims the otherwise-unknown morph; the custom morph stays unknown.
        let efm = out.identified.iter().find(|g| g.name.as_deref() == Some("Expressive Facegen Morphs")).unwrap();
        assert_eq!(efm.library_source.as_deref(), Some("seed"));
        assert!(out.unknown.iter().any(|a| a.value == "Totally_Custom"));
        assert!(out.unknown.iter().all(|a| a.value != "EFM_Brow_Width"));

        // 3. Seed attaches its link to the URL-less local-folder group for Other.esp
        //    WITHOUT changing resolved_by.
        let other = out.identified.iter().find(|g| g.assets.iter().any(|a| a.value == "Other.esp")).unwrap();
        assert_eq!(other.resolved_by, "local-folder");
        assert_eq!(other.page_url.as_deref(), Some("https://example.com/seed-other"));
        assert_eq!(other.name.as_deref(), Some("Other Mod"));

        // 4. A plugin nothing matches stays unknown.
        assert!(out.unknown.iter().any(|a| a.value == "Missing.esp"));

        // 5. A plugin found nowhere and unmatched by heuristics is still
        //    claimed by a seed entry (the positive post-heuristics case).
        let seed_only = out.identified.iter().find(|g| g.name.as_deref() == Some("Seed Only Mod")).unwrap();
        assert_eq!(seed_only.resolved_by, "library");
        assert_eq!(seed_only.library_source.as_deref(), Some("seed"));
        assert!(out.unknown.iter().all(|a| a.value != "SeedOnly.esp"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn library_precedence_texture_kind_covers_all_legs() {
        use crate::library::{Library, LibraryEntry, MergedEntry};
        use crate::settings::AppSettings;

        let dir = std::env::temp_dir().join(format!("lineage-resolve-lib-tex-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // A mod folder providing one texture → automatic local-folder attribution
        // would claim it if the user override didn't intercept first.
        let mods = dir.join("mods");
        std::fs::create_dir_all(mods.join("Tex Mod").join("textures").join("hair")).unwrap();
        std::fs::write(
            mods.join("Tex Mod").join("textures").join("hair").join("uservalue.dds"),
            b"x",
        )
        .unwrap();

        let settings = AppSettings {
            mod_manager: crate::settings::ModManagerKind::Mo2,
            mo2_mods_folder: mods.display().to_string(),
            ..Default::default()
        };
        let ctx = build_context(&settings);

        let mk = |id: &str, source: &str, kind: &str, pattern: &str, match_type: &str, name: &str| MergedEntry {
            entry: LibraryEntry {
                id: id.into(), kind: kind.into(), pattern: pattern.into(),
                match_type: match_type.into(), name: name.into(),
                url: format!("https://example.com/{id}"),
            },
            source: source.into(),
            enabled: true,
        };
        let library = Library {
            entries: vec![
                // (a) User entry for a texture that ALSO exists on disk under a
                // tracked mod folder — must resolve via the library before
                // find_texture attribution ever runs.
                mk("user-tex", "user", "texture", "hair\\uservalue.dds", "exact", "User Texture Mod"),
                // (b) Seed entry for a mod that ships INSIDE the vanilla tree.
                // Racial Skin Variance lives at Actors\Character\RSV\, so the
                // old "anything under Actors\Character\ is vanilla" gate used
                // to swallow it. A named entry now outranks that heuristic.
                mk(
                    "seed-rsv-tex",
                    "seed",
                    "texture",
                    "Actors\\Character\\RSV\\",
                    "prefix",
                    "Racial Skin Variance",
                ),
                // (c) Seed entry for a non-vanilla texture resolved nowhere else.
                mk("seed-tex", "seed", "texture", "custom\\seedtex.dds", "exact", "Seed Texture Mod"),
                // (d) Seed entry for a RaceMenu overlay texture — despite
                // starting with Actors\Character\, overlays never ship in
                // vanilla BSAs and must gap-fill from the seed instead of
                // being swallowed by the vanilla-shape gate.
                mk(
                    "seed-overlay-tex",
                    "seed",
                    "texture",
                    "Actors\\Character\\Overlays\\FooOverlay\\",
                    "prefix",
                    "Overlay Seed Mod",
                ),
            ],
            warning: None,
        };

        let refs = |kind: &str, value: &str| AssetRef {
            kind: kind.into(), value: value.into(), appeared_in: vec!["test".into()],
        };
        let textures = vec![
            refs("texture", "hair\\uservalue.dds"),
            refs("texture", "Actors\\Character\\FooBar.dds"),
            refs("texture", "Actors\\Character\\RSV\\DarkElfMale\\MaleHead.dds"),
            refs("texture", "custom\\seedtex.dds"),
            refs("texture", "Actors\\Character\\Overlays\\FooOverlay\\tex.dds"),
        ];

        let out = resolve_refs_offline(&settings, &ctx, &library, &[], &textures, &[]);

        // (a) User override wins over the on-disk local-folder attribution.
        let user_tex = out
            .identified
            .iter()
            .find(|g| g.assets.iter().any(|a| a.value == "hair\\uservalue.dds"))
            .unwrap();
        assert_eq!(user_tex.resolved_by, "library");
        assert_eq!(user_tex.name.as_deref(), Some("User Texture Mod"));
        assert_eq!(user_tex.library_source.as_deref(), Some("user"));

        // (b) A vanilla-shaped path that NO library entry names still falls to
        // the vanilla-shape gate rather than the unknown list.
        assert!(out.extra_vanilla.iter().any(|v| v.eq_ignore_ascii_case("Actors\\Character\\FooBar.dds")));
        assert!(out.unknown.iter().all(|a| a.value != "Actors\\Character\\FooBar.dds"));

        // (b2) ...but a named library entry outranks that gate. The library is
        // explicit human knowledge; the gate is a guess, and a guess that
        // overrides a correct entry leaves the user no way to fix it from the
        // UI. Genuine base-game skin slots and tint masks cannot reach here at
        // all — `is_vanilla_skin_texture` drops them during extraction, before
        // any library lookup — so this precedence cannot mislabel vanilla.
        let rsv = out
            .identified
            .iter()
            .find(|g| g.name.as_deref() == Some("Racial Skin Variance"))
            .unwrap();
        assert_eq!(rsv.resolved_by, "library");
        assert_eq!(rsv.library_source.as_deref(), Some("seed"));
        assert!(out
            .extra_vanilla
            .iter()
            .all(|v| !v.to_ascii_lowercase().contains("rsv")));

        // (c) Non-vanilla texture resolved nowhere else is claimed by the seed entry.
        let seed_tex = out.identified.iter().find(|g| g.name.as_deref() == Some("Seed Texture Mod")).unwrap();
        assert_eq!(seed_tex.resolved_by, "library");
        assert_eq!(seed_tex.library_source.as_deref(), Some("seed"));
        assert!(seed_tex.assets.iter().any(|a| a.value == "custom\\seedtex.dds"));

        // (d) An overlay texture is excluded from the vanilla-shape gate —
        // it resolves via the seed entry instead of being misclassified as
        // "ships in vanilla BSAs".
        let overlay_tex = out.identified.iter().find(|g| g.name.as_deref() == Some("Overlay Seed Mod")).unwrap();
        assert_eq!(overlay_tex.resolved_by, "library");
        assert_eq!(overlay_tex.library_source.as_deref(), Some("seed"));
        assert!(overlay_tex.assets.iter().any(|a| a.value == "Actors\\Character\\Overlays\\FooOverlay\\tex.dds"));
        assert!(out.extra_vanilla.iter().all(|v| !v.to_ascii_lowercase().contains("overlays")));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Every identified group carries a distinct `key`, even when several
    /// groups are indistinguishable on their visible fields. The shipped seed
    /// library really does contain such pairs — the ECE_/CME_ slider entries
    /// and the two Kyoe brow plugin spellings each point at one Nexus mod —
    /// and the UI lists groups in a keyed `{#each}`, which throws on a
    /// duplicate key and aborts the whole render.
    #[test]
    fn identified_groups_carry_unique_keys() {
        let dir = std::env::temp_dir().join(format!("lineage-group-keys-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let library = crate::library::load_from_dir(&dir);
        let settings = AppSettings::default();
        let ctx = build_context(&settings);

        let refs = |kind: &str, value: &str| AssetRef {
            kind: kind.into(), value: value.into(), appeared_in: vec!["test".into()],
        };
        let plugins = vec![
            refs("plugin", "Kyoe_BanginBrows.esp"),
            refs("plugin", "Kyoe BanginBrows.esp"),
        ];
        let morphs = vec![refs("morph", "ECE_Widen"), refs("morph", "CME_Widen")];
        let out = resolve_refs_offline(&settings, &ctx, &library, &plugins, &[], &morphs);

        assert_eq!(out.identified.len(), 4, "one group per matched seed entry");
        let keys: std::collections::HashSet<&str> =
            out.identified.iter().map(|g| g.key.as_str()).collect();
        assert_eq!(keys.len(), out.identified.len(), "group keys are unique");
        assert!(out.identified.iter().all(|g| !g.key.is_empty()));
        // The collision the key field exists to survive: distinct groups, same mod id.
        let mod_ids: Vec<Option<u32>> = out.identified.iter().map(|g| g.mod_id).collect();
        assert_eq!(mod_ids.iter().filter(|id| **id == Some(12302)).count(), 2);
        assert_eq!(mod_ids.iter().filter(|id| **id == Some(13630)).count(), 2);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn vanilla_skin_slots_and_tintmasks_are_not_requirements() {
        let preset: Value = serde_json::from_str(
            r#"{
                "faceTextures": [
                    {"index": 0, "texture": "Actors\\Character\\Female\\FemaleHead.dds"},
                    {"index": 1, "texture": "Actors\\Character\\BretonFemale\\FemaleHead_msn.dds"},
                    {"index": 3, "texture": "Actors\\Character\\Male\\BlankDetailmap.dds"},
                    {"index": 4, "texture": "Actors\\Character\\RSV\\DarkElfMale\\MaleHead.dds"},
                    {"index": 5, "texture": "ColdSun\\Visions\\Body\\FemaleHead_sk.dds"},
                    {"index": 6, "texture": "Actors\\Character\\Female\\FaceDetails\\FaceFemale_0.dds"}
                ],
                "tintInfo": [
                    {"color": 1, "index": 0, "texture": "Actors\\Character\\Character Assets\\TintMasks\\SkinTone.dds"},
                    {"color": 2, "index": 1, "texture": "Actors\\Character\\Overlays\\FMS\\Eyeliner\\Wing 2.dds"},
                    {"color": 3, "index": 2, "texture": "Actors\\Character\\Character Assets\\Overlays\\Frecklemania2\\Female\\Face\\HeFace3.dds"}
                ]
            }"#,
        )
        .unwrap();
        let e = extract(&preset);
        let tex: Vec<&str> = e.textures.iter().map(|t| t.value.as_str()).collect();

        // Base-game skin slots + vanilla tint masks: every install already has
        // them, so they are recorded as vanilla and never offered as a
        // requirement — whichever skin/warpaint replacer happens to be installed.
        for dropped in [
            r"Actors\Character\Female\FemaleHead.dds",
            r"Actors\Character\BretonFemale\FemaleHead_msn.dds",
            r"Actors\Character\Male\BlankDetailmap.dds",
            r"Actors\Character\Character Assets\TintMasks\SkinTone.dds",
        ] {
            assert!(!tex.contains(&dropped), "{dropped} should not be a requirement");
            assert!(
                e.vanilla.iter().any(|v| v == dropped),
                "{dropped} should be listed as vanilla"
            );
        }

        // Mods shipping inside (or alongside) the vanilla tree are real
        // dependencies and must survive.
        for kept in [
            r"Actors\Character\RSV\DarkElfMale\MaleHead.dds",
            r"ColdSun\Visions\Body\FemaleHead_sk.dds",
            r"Actors\Character\Female\FaceDetails\FaceFemale_0.dds",
            r"Actors\Character\Overlays\FMS\Eyeliner\Wing 2.dds",
            r"Actors\Character\Character Assets\Overlays\Frecklemania2\Female\Face\HeFace3.dds",
        ] {
            assert!(tex.contains(&kept), "{kept} should still be a requirement");
        }
    }

    #[test]
    fn is_vanilla_skin_texture_boundaries() {
        // A vanilla skin slot, however the path happens to be spelled.
        assert!(is_vanilla_skin_texture(
            r"Actors\Character\Female\FemaleHead.dds"
        ));
        assert!(is_vanilla_skin_texture(
            "actors/character/male/malehead_sk.dds"
        ));
        assert!(is_vanilla_skin_texture(
            r"Data\Textures\Actors\Character\FemaleOrc\FemaleHeadOrc_msn.dds"
        ));
        assert!(is_vanilla_skin_texture(
            r"Actors\Character\Character Assets\TintMasks\FemaleHead_Cheeks.dds"
        ));

        // A mod's own folder inside the vanilla tree is not a vanilla slot.
        assert!(!is_vanilla_skin_texture(
            r"Actors\Character\RSV\DarkElfMale\MaleHead.dds"
        ));
        // Neither is a mod subfolder nested under a vanilla race folder —
        // the file must sit directly in the race folder to count.
        assert!(!is_vanilla_skin_texture(
            r"Actors\Character\Female\FaceDetails\FaceFemale_0.dds"
        ));
        // Overlays and non-vanilla roots are untouched.
        assert!(!is_vanilla_skin_texture(
            r"Actors\Character\Overlays\FMS\Eyeliner\Wing 2.dds"
        ));
        assert!(!is_vanilla_skin_texture(
            r"Actors\Character\Character Assets\Overlays\Frecklemania2\Female\Face\HeFace3.dds"
        ));
        assert!(!is_vanilla_skin_texture(
            r"ColdSun\Visions\Body\FemaleHead_sk.dds"
        ));
        assert!(!is_vanilla_skin_texture(r"!COR\Head\FemaleHead.dds"));
    }

    /// The skin-slot / tint-mask rule, checked against the real preset
    /// collection rather than a synthetic preset — an over-reaching rule that
    /// swallowed a real mod's textures would still pass the unit tests above.
    #[test]
    fn vanilla_skin_rule_against_real_presets() {
        let corpus = std::path::Path::new(r"D:\Nordic Souls\mods");
        if !corpus.is_dir() {
            return;
        }
        let mut textures: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut vanilla: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut presets = 0usize;
        for entry in walkdir::WalkDir::new(corpus).into_iter().flatten() {
            let is_jslot = entry.file_type().is_file()
                && entry
                    .path()
                    .extension()
                    .map(|e| e.eq_ignore_ascii_case("jslot"))
                    .unwrap_or(false);
            if !is_jslot {
                continue;
            }
            let Ok(preset) = jslot::parse_file(entry.path()) else {
                continue;
            };
            presets += 1;
            let e = extract(&preset);
            textures.extend(e.textures.iter().map(|t| t.value.to_ascii_lowercase()));
            vanilla.extend(e.vanilla.iter().map(|v| v.to_ascii_lowercase()));
        }
        assert!(presets > 100, "expected a real corpus, saw {presets} presets");

        // Nothing the rule dropped may survive as a requirement...
        assert!(
            !textures.iter().any(|t| is_vanilla_skin_texture(t)),
            "a vanilla skin slot or tint mask leaked into the requirement list"
        );
        // ...and the two headline cases really are being dropped.
        for dropped in [
            r"actors\character\female\femalehead.dds",
            r"actors\character\character assets\tintmasks\skintone.dds",
        ] {
            assert!(!textures.contains(dropped), "{dropped} should be dropped");
            assert!(vanilla.contains(dropped), "{dropped} should be vanilla");
        }

        // The rule must NOT over-reach: mods living inside the vanilla tree
        // stay requirements. Racial Skin Variance sits at Actors\Character\RSV\.
        assert!(
            textures.iter().any(|t| t.contains(r"\rsv\")),
            "Racial Skin Variance textures were swallowed by the vanilla rule"
        );
        assert!(
            textures.iter().any(|t| t.contains(r"\overlays\")),
            "overlay textures were swallowed by the vanilla rule"
        );
    }

    fn unknown_ref(kind: &str, value: &str) -> AssetRef {
        AssetRef {
            kind: kind.into(),
            value: value.into(),
            appeared_in: vec!["test".into()],
        }
    }

    #[test]
    fn unknown_groups_collapse_to_one_entry_per_mod() {
        let groups = group_unknown(&[
            // One overlay mod's folder — three rows, one entry.
            unknown_ref("texture", r"Actors\Character\Overlays\Koralina_Male\a.dds"),
            unknown_ref("texture", r"Actors\Character\Overlays\Koralina_Male\b.dds"),
            unknown_ref("texture", r"Actors\Character\Overlays\Koralina_Male\deep\c.dds"),
            // Same mod, spelled with the Data\Textures\ prefix. Must not
            // become a second group.
            unknown_ref(
                "texture",
                r"Data\Textures\Actors\Character\PubicHairStyles\a.dds",
            ),
            unknown_ref("texture", r"Actors\Character\PubicHairStyles\b.dds"),
            // Loose files in a generic folder: only the filename prefix
            // identifies them.
            unknown_ref("texture", r"Actors\empyreancs_f_10_a.dds"),
            unknown_ref("texture", r"Actors\empyreancs_f_11_b.dds"),
            // Morph families carry their prefix the way the seed entries do.
            unknown_ref("morph", "EXPR_BrowWidth"),
            unknown_ref("morph", "EXPR_NoseLength"),
            // Plugins have no shared structure — one each.
            unknown_ref("plugin", "SomeHair.esp"),
        ]);

        let by_pattern = |p: &str| {
            groups
                .iter()
                .find(|g| g.pattern.eq_ignore_ascii_case(p))
                .unwrap_or_else(|| panic!("no group for {p}; got {:?}", patterns(&groups)))
        };

        let koralina = by_pattern(r"Actors\Character\Overlays\Koralina_Male\");
        assert_eq!(koralina.assets.len(), 3, "subfolders belong to their mod");
        assert_eq!(koralina.match_type, "prefix");
        // Capitalization is taken from the preset, not the lowercased key.
        assert_eq!(
            koralina.pattern,
            r"Actors\Character\Overlays\Koralina_Male\"
        );

        let pubic = by_pattern(r"Actors\Character\PubicHairStyles\");
        assert_eq!(
            pubic.assets.len(),
            2,
            "Data\\Textures\\ is a spelling, not a different mod"
        );

        let empyrean = by_pattern(r"Actors\empyreancs_");
        assert_eq!(empyrean.assets.len(), 2);
        assert_eq!(empyrean.match_type, "prefix");

        assert_eq!(by_pattern("EXPR_").assets.len(), 2);

        let plugin = by_pattern("somehair.esp");
        assert_eq!(plugin.match_type, "exact");
        assert_eq!(plugin.kind, "plugin");

        assert_eq!(groups.len(), 5, "got {:?}", patterns(&groups));
        // Keys must be unique — the UI uses them for a keyed list.
        let keys: std::collections::HashSet<&str> =
            groups.iter().map(|g| g.key.as_str()).collect();
        assert_eq!(keys.len(), groups.len());
        // Biggest group first.
        assert!(groups[0].assets.len() >= groups[groups.len() - 1].assets.len());
    }

    fn patterns(groups: &[UnknownGroup]) -> Vec<&str> {
        groups.iter().map(|g| g.pattern.as_str()).collect()
    }

    #[test]
    fn unnamed_texture_with_no_handle_stays_its_own_exact_group() {
        // No mod folder, no usable filename prefix — nothing to generalize,
        // so it must not silently join a broader group.
        let groups = group_unknown(&[unknown_ref("texture", r"Actors\thing.dds")]);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].match_type, "exact");
        assert_eq!(groups[0].assets.len(), 1);
    }

    /// The collapse ratio, measured on the real collection — a grouping rule
    /// that quietly stopped grouping would still pass the cases above.
    #[test]
    fn unknown_grouping_collapses_the_real_corpus() {
        let corpus = std::path::Path::new(r"D:\Nordic Souls\mods");
        if !corpus.is_dir() {
            return;
        }
        let mut refs: HashMap<String, AssetRef> = HashMap::new();
        for entry in walkdir::WalkDir::new(corpus).into_iter().flatten() {
            let is_jslot = entry.file_type().is_file()
                && entry
                    .path()
                    .extension()
                    .map(|e| e.eq_ignore_ascii_case("jslot"))
                    .unwrap_or(false);
            if !is_jslot {
                continue;
            }
            let Ok(preset) = jslot::parse_file(entry.path()) else {
                continue;
            };
            for t in extract(&preset).textures {
                refs.entry(t.value.to_ascii_lowercase()).or_insert(t);
            }
        }
        let assets: Vec<AssetRef> = refs.into_values().collect();
        assert!(assets.len() > 500, "expected a real corpus");
        let groups = group_unknown(&assets);
        assert!(
            groups.len() * 5 < assets.len(),
            "expected at least a 5x collapse, got {} groups for {} paths",
            groups.len(),
            assets.len()
        );
        // Every asset lands in exactly one group.
        let grouped: usize = groups.iter().map(|g| g.assets.len()).sum();
        assert_eq!(grouped, assets.len());
    }
}
