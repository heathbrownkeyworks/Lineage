//! Requirements for a preset pack — the Nexus "Requirements" section, built
//! from the presets that will actually ship.
//!
//! Find Assets answers this for one preset and Collection Review for the
//! whole collection; a pack is the case in between. This resolves every
//! reference across a chosen set of presets once, and counts how many of
//! them need each mod. It resolves them *as they'll ship*: given clean
//! categories, each preset is cleaned in memory first — the files are never
//! written — because a cleaned-away body tattoo's texture mod is no longer a
//! requirement, and a Requirements list that still names it would be wrong.
//!
//! Rendering follows Heath's Forge BBCode house style (as on Horde's
//! shipped page): amber section header, `[url=…]Name[/url]
//! - why` list items, and no em or en dashes anywhere.

use crate::assets::{self, AssetRef, FindProgress, IdentifiedGroup};
use crate::clean::{self, CleanCategory};
use crate::jslot;
use crate::rawjson;
use crate::settings::{self, AppSettings};
use crate::snapshot::FailedFile;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeSet, HashMap};
use std::path::Path;

/// One mod a pack needs.
#[derive(Debug, Clone, Serialize)]
pub struct Requirement {
    /// Stable identity for a keyed list: the resolved group's key, or
    /// `implicit:racemenu`.
    pub key: String,
    pub name: String,
    pub url: Option<String>,
    /// Distinct chosen presets that need it.
    pub used_by: usize,
    /// How it was identified ("meta.ini", "md5", "library", …).
    pub resolved_by: String,
    /// The references that pulled it in. Empty for RaceMenu, which no preset
    /// references and every preset needs.
    pub assets: Vec<AssetRef>,
}

#[derive(Debug, Clone, Serialize)]
pub struct UnknownReference {
    pub asset: AssetRef,
    pub used_by: usize,
}

#[derive(Debug, Serialize)]
pub struct RequirementsReport {
    /// Presets read successfully; the rest are in `failed`.
    pub preset_count: usize,
    pub failed: Vec<FailedFile>,
    /// RaceMenu first, then by how many presets need each, then by name.
    pub requirements: Vec<Requirement>,
    /// References nothing could identify. Left out of the rendered list —
    /// the UI says so, so a pack never ships with a silently short one.
    pub unknown: Vec<UnknownReference>,
    pub api_key_present: bool,
    pub nexus_error: Option<String>,
    pub library_warning: Option<String>,
}

fn ref_key(r: &AssetRef) -> String {
    format!("{}|{}", r.kind, r.value.trim().to_ascii_lowercase())
}

/// A preset parsed as it will ship: cleaned in memory with `categories`,
/// or as-is when there are none. The file itself is never written.
fn shipped_value(path: &Path, categories: &[CleanCategory]) -> Result<Value, String> {
    if categories.is_empty() {
        return jslot::parse_file(path);
    }
    let bytes =
        std::fs::read(path).map_err(|e| format!("Couldn't read {}: {e}", path.display()))?;
    let (bom, text) = jslot::read_text(&bytes);
    serde_json::from_str::<Value>(&text).map_err(|e| format!("Not valid preset JSON ({e})"))?;
    let mut raw = rawjson::parse(&text).map_err(|e| format!("Not valid preset JSON ({e})"))?;
    clean::clean(&mut raw, categories);
    let cleaned = jslot::to_formatted_string(&raw, jslot::detect_format(&text, bom));
    serde_json::from_str(cleaned.trim_start_matches('\u{feff}'))
        .map_err(|e| format!("The cleaned preset didn't parse ({e})"))
}

struct Collected {
    preset_count: usize,
    failed: Vec<FailedFile>,
    plugins: Vec<AssetRef>,
    textures: Vec<AssetRef>,
    morphs: Vec<AssetRef>,
    /// `ref_key` → indices of the presets that reference it.
    users: HashMap<String, BTreeSet<usize>>,
}

fn collect(
    paths: &[String],
    categories: &[CleanCategory],
    progress: &mut dyn FnMut(FindProgress),
) -> Collected {
    let mut plugins: HashMap<String, AssetRef> = HashMap::new();
    let mut textures: HashMap<String, AssetRef> = HashMap::new();
    let mut morphs: HashMap<String, AssetRef> = HashMap::new();
    let mut users: HashMap<String, BTreeSet<usize>> = HashMap::new();
    let mut failed = Vec::new();
    let mut preset_count = 0usize;
    for (i, path) in paths.iter().enumerate() {
        let path = Path::new(path);
        progress(FindProgress {
            stage: "parsing".into(),
            current: i + 1,
            total: paths.len(),
            detail: path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
        });
        let value = match shipped_value(path, categories) {
            Ok(value) => value,
            Err(reason) => {
                failed.push(FailedFile {
                    path: path.display().to_string(),
                    reason,
                });
                continue;
            }
        };
        preset_count += 1;
        let extracted = assets::extract(&value);
        for (map, refs) in [
            (&mut plugins, extracted.plugins),
            (&mut textures, extracted.textures),
            (&mut morphs, extracted.morphs),
        ] {
            for r in refs {
                users.entry(ref_key(&r)).or_default().insert(i);
                let entry = map
                    .entry(r.value.trim().to_ascii_lowercase())
                    .or_insert_with(|| AssetRef {
                        kind: r.kind.clone(),
                        value: r.value.clone(),
                        appeared_in: Vec::new(),
                    });
                for section in r.appeared_in {
                    if !entry.appeared_in.contains(&section) {
                        entry.appeared_in.push(section);
                    }
                }
            }
        }
    }
    Collected {
        preset_count,
        failed,
        plugins: assets::sort_refs(plugins),
        textures: assets::sort_refs(textures),
        morphs: assets::sort_refs(morphs),
        users,
    }
}

/// Distinct presets needing any of `refs` — a union, not a sum, so a mod
/// whose three textures appear in the same preset counts that preset once.
fn used_by(refs: &[AssetRef], users: &HashMap<String, BTreeSet<usize>>) -> usize {
    let mut presets = BTreeSet::new();
    for r in refs {
        if let Some(set) = users.get(&ref_key(r)) {
            presets.extend(set.iter().copied());
        }
    }
    presets.len()
}

/// One mod, one line. Several library entries or resolution paths often
/// land on the same page — ECE's and CME's slider families are both
/// nexus/12302, a plugin and its morph family share a page, one mod ships
/// two plugin filenames — and a Requirements section that lists a mod twice
/// reads as a mistake. Requirements sharing a link (or, without one, a name)
/// merge; usage is recounted as a union, never summed.
fn merge_by_link(
    requirements: Vec<Requirement>,
    users: &HashMap<String, BTreeSet<usize>>,
) -> Vec<Requirement> {
    let link_key = |r: &Requirement| match r.url.as_deref().map(str::trim).filter(|u| !u.is_empty()) {
        Some(url) => format!("url:{}", url.trim_end_matches('/').to_ascii_lowercase()),
        None => format!("name:{}", r.name.trim().to_ascii_lowercase()),
    };
    let mut order: Vec<String> = Vec::new();
    let mut buckets: HashMap<String, Vec<Requirement>> = HashMap::new();
    for r in requirements {
        let key = link_key(&r);
        if !buckets.contains_key(&key) {
            order.push(key.clone());
        }
        buckets.entry(key).or_default().push(r);
    }
    order
        .into_iter()
        .map(|key| {
            let mut group = buckets.remove(&key).unwrap_or_default();
            if group.len() == 1 {
                return group.pop().unwrap();
            }
            group.sort_by(|a, b| b.used_by.cmp(&a.used_by));
            let name = merged_name(&group);
            let assets: Vec<AssetRef> = group.iter().flat_map(|r| r.assets.clone()).collect();
            let first = group.swap_remove(0);
            Requirement {
                used_by: used_by(&assets, users),
                name,
                assets,
                ..first
            }
        })
        .collect()
}

/// A name for requirements being merged: the shared part of their names
/// when there is a meaningful one ("Enhanced Character Edit SE" from its
/// "(CME sliders)" and "(ECE sliders)" entries), otherwise the most-used
/// requirement's name. `group` is sorted most-used first.
fn merged_name(group: &[Requirement]) -> String {
    let first = group[0].name.trim();
    if group.iter().all(|r| r.name.trim().eq_ignore_ascii_case(first)) {
        return first.to_string();
    }
    let mut prefix: Vec<char> = first.chars().collect();
    for r in &group[1..] {
        let common = prefix
            .iter()
            .zip(r.name.trim().chars())
            .take_while(|(a, b)| a.eq_ignore_ascii_case(b))
            .count();
        prefix.truncate(common);
    }
    let shared: String = prefix.into_iter().collect();
    let shared = shared.trim_end_matches(|c: char| c.is_whitespace() || "(-:,[".contains(c));
    if shared.chars().count() >= 4 {
        shared.to_string()
    } else {
        first.to_string()
    }
}

fn display_name(g: &IdentifiedGroup) -> String {
    g.name
        .clone()
        .or_else(|| g.nexus.as_ref().and_then(|n| n.name.clone()))
        .or_else(|| g.mod_folder.clone())
        .unwrap_or_else(|| "Unknown mod".into())
}

pub fn build_report(
    app: Option<&tauri::AppHandle>,
    settings: &AppSettings,
    library: &crate::library::Library,
    paths: &[String],
    categories: &[CleanCategory],
    progress: &mut dyn FnMut(FindProgress),
) -> RequirementsReport {
    let c = collect(paths, categories, progress);
    let ctx = assets::build_context(settings);
    let output = assets::resolve_refs(
        app,
        settings,
        &ctx,
        library,
        &c.plugins,
        &c.textures,
        &c.morphs,
        progress,
    );

    let requirements: Vec<Requirement> = output
        .identified
        .into_iter()
        .map(|g| Requirement {
            key: g.key.clone(),
            name: display_name(&g),
            url: g.page_url.clone(),
            used_by: used_by(&g.assets, &c.users),
            resolved_by: g.resolved_by.clone(),
            assets: g.assets,
        })
        .collect();
    let mut requirements = merge_by_link(requirements, &c.users);
    requirements.sort_by(|a, b| {
        b.used_by
            .cmp(&a.used_by)
            .then_with(|| a.name.to_ascii_lowercase().cmp(&b.name.to_ascii_lowercase()))
    });

    // A JSLOT is RaceMenu's own format: every preset needs it, yet none
    // references it. Named through the library so an edited entry follows.
    if c.preset_count > 0 {
        if let Some(racemenu) = library.match_entry("plugin", "RaceMenu.esp", false) {
            let listed = requirements.iter().any(|r| {
                r.url.as_deref() == Some(racemenu.entry.url.as_str())
                    || r.name.eq_ignore_ascii_case(&racemenu.entry.name)
            });
            if !listed {
                requirements.insert(
                    0,
                    Requirement {
                        key: "implicit:racemenu".into(),
                        name: racemenu.entry.name.clone(),
                        url: Some(racemenu.entry.url.clone()),
                        used_by: c.preset_count,
                        resolved_by: "library".into(),
                        assets: Vec::new(),
                    },
                );
            }
        }
    }

    let mut unknown: Vec<UnknownReference> = output
        .unknown
        .into_iter()
        .map(|asset| UnknownReference {
            used_by: used_by(std::slice::from_ref(&asset), &c.users),
            asset,
        })
        .collect();
    unknown.sort_by(|a, b| b.used_by.cmp(&a.used_by).then_with(|| a.asset.value.cmp(&b.asset.value)));

    RequirementsReport {
        preset_count: c.preset_count,
        failed: c.failed,
        requirements,
        unknown,
        api_key_present: output.api_key_present,
        nexus_error: output.nexus_error,
        library_warning: library.warning.clone(),
    }
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportFormat {
    Bbcode,
    Markdown,
    Plain,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RenderLine {
    pub name: String,
    pub url: Option<String>,
    pub used_by: usize,
}

/// The house style forbids em and en dashes, and Nexus titles often carry
/// them, so a name is cleaned before it lands in the output.
fn no_dashes(s: &str) -> String {
    s.replace(['\u{2014}', '\u{2013}'], "-")
}

fn usage(used_by: usize, total: usize) -> Option<String> {
    (used_by < total).then(|| format!("used by {used_by} of {total} presets"))
}

/// One Requirements section. `total` is the number of presets in the pack;
/// a mod every preset needs gets no usage note.
pub fn render(lines: &[RenderLine], total: usize, format: ExportFormat, header: bool) -> String {
    let mut out = String::new();
    match format {
        ExportFormat::Bbcode => {
            if header {
                out.push_str("[size=5][b][color=#f59e0b]Requirements[/color][/b][/size]\n\n");
            }
            out.push_str("[list]\n");
            for line in lines {
                // Square brackets in a name would read as tags, and Nexus
                // strips tags it doesn't know — "[Dint999] HairPack02" would
                // lose its author.
                let name = no_dashes(line.name.trim()).replace('[', "(").replace(']', ")");
                out.push_str("[*]");
                match line.url.as_deref().map(str::trim).filter(|u| !u.is_empty()) {
                    Some(url) => out.push_str(&format!("[url={url}]{name}[/url]")),
                    None => out.push_str(&name),
                }
                if let Some(note) = usage(line.used_by, total) {
                    out.push_str(" - ");
                    out.push_str(&note);
                }
                out.push('\n');
            }
            out.push_str("[/list]\n");
        }
        ExportFormat::Markdown => {
            if header {
                out.push_str("## Requirements\n\n");
            }
            for line in lines {
                let name = no_dashes(line.name.trim()).replace('[', "\\[").replace(']', "\\]");
                out.push_str("- ");
                match line.url.as_deref().map(str::trim).filter(|u| !u.is_empty()) {
                    Some(url) => out.push_str(&format!("[{name}]({url})")),
                    None => out.push_str(&name),
                }
                if let Some(note) = usage(line.used_by, total) {
                    out.push_str(" - ");
                    out.push_str(&note);
                }
                out.push('\n');
            }
        }
        ExportFormat::Plain => {
            if header {
                out.push_str("Requirements\n\n");
            }
            for line in lines {
                out.push_str("- ");
                out.push_str(&no_dashes(line.name.trim()));
                if let Some(url) = line.url.as_deref().map(str::trim).filter(|u| !u.is_empty()) {
                    out.push_str(&format!(" ({url})"));
                }
                if let Some(note) = usage(line.used_by, total) {
                    out.push_str(" - ");
                    out.push_str(&note);
                }
                out.push('\n');
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/// Every `.jslot` under a folder, sorted — how a pack is usually chosen.
pub fn presets_in(folder: &Path) -> Result<Vec<String>, String> {
    if !folder.is_dir() {
        return Err(format!("{} isn't a folder.", folder.display()));
    }
    let mut out: Vec<String> = walkdir::WalkDir::new(folder)
        .into_iter()
        .flatten()
        .filter(|e| {
            e.file_type().is_file()
                && e.path()
                    .extension()
                    .map(|x| x.eq_ignore_ascii_case("jslot"))
                    .unwrap_or(false)
        })
        .map(|e| e.path().display().to_string())
        .collect();
    out.sort_by_key(|p| p.to_ascii_lowercase());
    Ok(out)
}

#[tauri::command]
pub async fn list_presets_in(folder: String) -> Result<Vec<String>, String> {
    tauri::async_runtime::spawn_blocking(move || presets_in(Path::new(&folder)))
        .await
        .map_err(|e| format!("preset listing task failed: {e}"))?
}

#[tauri::command]
pub async fn requirements_for(
    app: tauri::AppHandle,
    paths: Vec<String>,
    categories: Vec<CleanCategory>,
    on_progress: tauri::ipc::Channel<FindProgress>,
) -> Result<RequirementsReport, String> {
    tauri::async_runtime::spawn_blocking(move || {
        if paths.is_empty() {
            return Err("No presets chosen.".to_string());
        }
        let settings = settings::load_from_app(&app)?;
        let library = assets::library_for(&app)?;
        let mut progress = |p: FindProgress| {
            let _ = on_progress.send(p);
        };
        Ok(build_report(
            Some(&app),
            &settings,
            &library,
            &paths,
            &categories,
            &mut progress,
        ))
    })
    .await
    .map_err(|e| format!("requirements task failed: {e}"))?
}

#[tauri::command]
pub fn render_requirements(
    lines: Vec<RenderLine>,
    total: usize,
    format: ExportFormat,
    header: bool,
) -> String {
    render(&lines, total, format, header)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::{Library, LibraryEntry, MergedEntry};

    const RACEMENU: &str = "https://www.nexusmods.com/skyrimspecialedition/mods/19080";

    fn lines() -> Vec<RenderLine> {
        let line = |name: &str, url: Option<&str>, used_by| RenderLine {
            name: name.into(),
            url: url.map(str::to_string),
            used_by,
        };
        vec![
            line("RaceMenu", Some(RACEMENU), 4),
            // Nexus titles carry em and en dashes; the house style has none.
            line("Kalilies Brows \u{2014} COTR \u{2013} UBE", Some("https://example.com/k"), 2),
            // Brackets would read as a BBCode tag and be stripped.
            line("[Dint999] HairPack02", Some("https://example.com/d"), 1),
            line("Local Folder Mod", None, 4),
        ]
    }

    #[test]
    fn bbcode_matches_the_house_requirements_section() {
        assert_eq!(
            render(&lines(), 4, ExportFormat::Bbcode, true),
            "[size=5][b][color=#f59e0b]Requirements[/color][/b][/size]\n\
             \n\
             [list]\n\
             [*][url=https://www.nexusmods.com/skyrimspecialedition/mods/19080]RaceMenu[/url]\n\
             [*][url=https://example.com/k]Kalilies Brows - COTR - UBE[/url] - used by 2 of 4 presets\n\
             [*][url=https://example.com/d](Dint999) HairPack02[/url] - used by 1 of 4 presets\n\
             [*]Local Folder Mod\n\
             [/list]\n"
        );
        // Without the header it's just the list, for pasting under your own.
        assert!(render(&lines(), 4, ExportFormat::Bbcode, false).starts_with("[list]\n"));
    }

    #[test]
    fn markdown_and_plain_escape_what_they_must() {
        assert_eq!(
            render(&lines(), 4, ExportFormat::Markdown, true),
            "## Requirements\n\
             \n\
             - [RaceMenu](https://www.nexusmods.com/skyrimspecialedition/mods/19080)\n\
             - [Kalilies Brows - COTR - UBE](https://example.com/k) - used by 2 of 4 presets\n\
             - [\\[Dint999\\] HairPack02](https://example.com/d) - used by 1 of 4 presets\n\
             - Local Folder Mod\n"
        );
        assert_eq!(
            render(&lines(), 4, ExportFormat::Plain, true),
            "Requirements\n\
             \n\
             - RaceMenu (https://www.nexusmods.com/skyrimspecialedition/mods/19080)\n\
             - Kalilies Brows - COTR - UBE (https://example.com/k) - used by 2 of 4 presets\n\
             - [Dint999] HairPack02 (https://example.com/d) - used by 1 of 4 presets\n\
             - Local Folder Mod\n"
        );
    }

    #[test]
    fn no_format_emits_a_dash_or_a_colour_outside_the_palette() {
        const PALETTE: [&str; 9] = [
            "#f59e0b", "#fbbf24", "#d99563", "#b8a285", "#382c1f", "#34d399", "#f43f5e",
            "#ef4444", "#60a5fa",
        ];
        for format in [ExportFormat::Bbcode, ExportFormat::Markdown, ExportFormat::Plain] {
            let out = render(&lines(), 4, format, true);
            assert!(!out.contains('\u{2014}') && !out.contains('\u{2013}'), "{format:?}: {out}");
            for (i, _) in out.match_indices("#") {
                let Some(hex) = out.get(i..i + 7) else { continue };
                if hex[1..].chars().all(|c| c.is_ascii_hexdigit()) {
                    assert!(PALETTE.contains(&hex), "{format:?} uses {hex}, not in the Forge palette");
                }
            }
        }
    }

    fn entry(id: &str, kind: &str, pattern: &str, match_type: &str, name: &str, url: &str) -> MergedEntry {
        MergedEntry {
            entry: LibraryEntry {
                id: id.into(),
                kind: kind.into(),
                pattern: pattern.into(),
                match_type: match_type.into(),
                name: name.into(),
                url: url.into(),
            },
            source: "seed".into(),
            enabled: true,
        }
    }

    #[test]
    fn requirements_count_presets_and_follow_the_clean() {
        let dir = std::env::temp_dir().join(format!("lineage-req-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("pack").join("sub")).unwrap();
        let write = |name: &str, body: &str| {
            let p = dir.join("pack").join(name);
            std::fs::write(&p, body).unwrap();
            p.display().to_string()
        };
        let tattooed = r#"{"modNames":["KS Hairdo's.esp"],"overrides":[{"node":"Body [Ovl0]","values":[{"data":"Actors\\Character\\Overlays\\Tattoos\\t.dds","index":0,"key":9,"type":2}]}]}"#;
        let a = write("a.jslot", tattooed);
        let b = write("b.jslot", r#"{"modNames":["KS Hairdo's.esp"]}"#);
        let c = write("sub/c.jslot", r#"{"modNames":["High Poly Head.esm"]}"#);
        let d = write("d.jslot", "{ not json");
        let e = write("e.jslot", r#"{"modNames":["Mystery.esp"]}"#);
        std::fs::write(dir.join("pack").join("notes.txt"), "x").unwrap();

        assert_eq!(
            presets_in(&dir.join("pack")).unwrap().len(),
            5,
            "every .jslot, recursively, and nothing else"
        );

        let library = Library {
            entries: vec![
                entry("seed-ks", "plugin", "KS Hairdo's.esp", "exact", "KS Hairdos SSE", "https://example.com/ks"),
                entry("seed-hph", "plugin", "High Poly Head.esm", "exact", "High Poly Head", "https://example.com/hph"),
                entry("seed-tat", "texture", r"Actors\Character\Overlays\Tattoos\", "prefix", "Tattoo Mod", "https://example.com/t"),
                entry("seed-rm", "plugin", "RaceMenu.esp", "exact", "RaceMenu", RACEMENU),
            ],
            warning: None,
        };
        let settings = AppSettings::default();
        let paths = vec![a.clone(), b, c, d, e];
        let summary = |r: &RequirementsReport| -> Vec<(String, usize)> {
            r.requirements.iter().map(|q| (q.name.clone(), q.used_by)).collect()
        };

        let as_is = build_report(None, &settings, &library, &paths, &[], &mut |_| {});
        assert_eq!(as_is.preset_count, 4);
        assert_eq!(as_is.failed.len(), 1, "the unreadable preset is reported, not fatal");
        assert_eq!(
            summary(&as_is),
            vec![
                ("RaceMenu".to_string(), 4),
                ("KS Hairdos SSE".to_string(), 2),
                ("High Poly Head".to_string(), 1),
                ("Tattoo Mod".to_string(), 1),
            ]
        );
        assert_eq!(as_is.unknown.len(), 1);
        assert_eq!(as_is.unknown[0].asset.value, "Mystery.esp");
        assert_eq!(as_is.unknown[0].used_by, 1);

        // Shipped cleaned, the tattoo's texture mod is no longer needed — and
        // the preset on disk is untouched.
        let before = std::fs::read(&a).unwrap();
        let cleaned = build_report(
            None,
            &settings,
            &library,
            &paths,
            &[CleanCategory::BodyOverlays],
            &mut |_| {},
        );
        assert!(summary(&cleaned).iter().all(|(name, _)| name != "Tattoo Mod"));
        assert_eq!(summary(&cleaned).len(), 3);
        assert_eq!(std::fs::read(&a).unwrap(), before, "cleaning happens in memory only");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_mod_reached_through_several_entries_is_one_line_counted_once() {
        let dir = std::env::temp_dir().join(format!("lineage-req-merge-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let write = |name: &str, morphs: &[&str]| {
            let list: Vec<String> = morphs.iter().map(|m| format!(r#"{{"name":"{m}","value":1}}"#)).collect();
            let p = dir.join(name);
            std::fs::write(&p, format!(r#"{{"morphs":{{"custom":[{}]}}}}"#, list.join(","))).unwrap();
            p.display().to_string()
        };
        let paths = vec![
            write("a.jslot", &["ECE_Nose"]),
            write("b.jslot", &["CME_Brow"]),
            // Uses both slider families: one preset, not two.
            write("c.jslot", &["ECE_Lip", "CME_Jaw"]),
        ];
        let ece = "https://www.nexusmods.com/skyrimspecialedition/mods/12302";
        let library = Library {
            entries: vec![
                entry("seed-ece", "morph", "ECE_", "prefix", "Enhanced Character Edit SE (ECE sliders)", ece),
                entry("seed-cme", "morph", "CME_", "prefix", "Enhanced Character Edit SE (CME sliders)", ece),
            ],
            warning: None,
        };
        let report = build_report(None, &AppSettings::default(), &library, &paths, &[], &mut |_| {});
        let ece_lines: Vec<&Requirement> =
            report.requirements.iter().filter(|r| r.url.as_deref() == Some(ece)).collect();
        assert_eq!(ece_lines.len(), 1, "{:?}", report.requirements);
        assert_eq!(ece_lines[0].name, "Enhanced Character Edit SE", "the shared part of both names");
        assert_eq!(ece_lines[0].used_by, 3, "a union of presets, never a sum");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
