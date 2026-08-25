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

struct Extracted {
    plugins: Vec<AssetRef>,
    textures: Vec<AssetRef>,
    morphs: Vec<AssetRef>,
    vanilla: Vec<String>,
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
fn collect_path_strings(value: &Value, section: &str, out: &mut HashMap<String, AssetRef>) {
    match value {
        Value::String(s) => {
            let l = s.to_ascii_lowercase();
            if l.ends_with(".dds") || l.ends_with(".nif") || l.ends_with(".tri") {
                note_ref(out, "texture", s, section);
            }
        }
        Value::Array(items) => {
            for v in items {
                collect_path_strings(v, section, out);
            }
        }
        Value::Object(obj) => {
            for v in obj.values() {
                collect_path_strings(v, section, out);
            }
        }
        _ => {}
    }
}

fn extract(preset: &Value) -> Extracted {
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
                    note_ref(&mut textures, "texture", tex, section);
                }
            }
        }
    }
    for section in ["overrides", "skinOverrides"] {
        if let Some(v) = preset.get(section) {
            collect_path_strings(v, section, &mut textures);
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

    let sorted = |map: HashMap<String, AssetRef>| -> Vec<AssetRef> {
        let mut v: Vec<AssetRef> = map.into_values().collect();
        v.sort_by(|a, b| a.value.to_ascii_lowercase().cmp(&b.value.to_ascii_lowercase()));
        v
    };
    Extracted {
        plugins: sorted(plugins),
        textures: sorted(textures),
        morphs: sorted(morphs),
        vanilla,
    }
}

// ---------------------------------------------------------------------------
// Local context: mod folders, meta.ini, Vortex manifest
// ---------------------------------------------------------------------------

struct LocalContext {
    /// Top-level mod folders (MO2 mods dir / Vortex staging), name → path.
    mod_folders: Vec<(String, PathBuf)>,
    data_dir: Option<PathBuf>,
    /// Deployed rel path (lowercase, backslashes) → source mod folder name.
    vortex_map: HashMap<String, String>,
}

fn build_context(settings: &AppSettings) -> LocalContext {
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
fn meta_ini_mod_id(mod_folder: &Path) -> Option<u32> {
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
            self.map.insert(key.clone(), make());
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
/// case but local identification still runs. `library` is threaded through
/// unused for now (see Task 4).
pub(crate) fn resolve_refs(
    app: Option<&tauri::AppHandle>,
    settings: &AppSettings,
    ctx: &LocalContext,
    _library: &crate::library::Library,
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

        let hit = if is_plugin {
            find_plugin(ctx, &asset.value)
        } else {
            find_texture(ctx, &asset.value)
        };
        let Some(hit) = hit else {
            if is_plugin {
                unresolved_plugins.push(asset.clone());
            } else if asset
                .value
                .to_ascii_lowercase()
                .replace('/', "\\")
                .starts_with("actors\\character\\")
            {
                // Not loose anywhere + a base-game character path: almost
                // certainly ships in the vanilla BSAs.
                if !extra_vanilla.iter().any(|v| v.eq_ignore_ascii_case(&asset.value)) {
                    extra_vanilla.push(asset.value.clone());
                }
            } else {
                unknown.push(asset.clone());
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
            None => unknown.push(asset.clone()),
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
            None => unknown.push(asset),
        }
    }

    // Morphs resolve to sliders, not files — straight to Unknown.
    unknown.extend(morphs.iter().cloned());

    ResolveOutput {
        identified: groups.into_vec(),
        unknown,
        extra_vanilla,
        api_key_present,
        nexus_error,
    }
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
        })
    })
    .await
    .map_err(|e| format!("find assets task failed: {e}"))?
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
