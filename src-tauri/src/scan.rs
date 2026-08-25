//! Recursive `.jslot` discovery across the configured roots.
//!
//! Scanning is always recursive — presets live in per-mod folders,
//! `SKSE\Plugins\CharGen\Presets\`, and arbitrary subtrees. A root of kind
//! `Mo2Mods` is an MO2 `mods` folder: only top-level mod folders enabled in
//! the active profile's modlist.txt are walked, which is how "every enabled
//! mod folder in the active profile" is expressed as a single root.

use crate::settings::{AppSettings, JslotRoot, RootKind};
use serde::Serialize;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

/// One discovered preset file.
#[derive(Debug, Clone, Serialize)]
pub struct JslotFile {
    pub path: String,
    pub file_name: String,
    /// Which configured root it was found under.
    pub root_id: String,
    pub root_label: String,
    /// Path relative to that root — this is what backup archives preserve.
    pub rel_path: String,
    pub size: u64,
    /// Unix seconds mtime, 0 when unavailable.
    pub modified: i64,
}

/// Per-root summary for the "Found N files across M locations" line.
#[derive(Debug, Clone, Serialize)]
pub struct RootScan {
    pub root_id: String,
    pub label: String,
    pub path: String,
    pub kind: RootKind,
    pub exists: bool,
    pub count: usize,
}

#[derive(Debug, Serialize)]
pub struct ScanResult {
    pub files: Vec<JslotFile>,
    pub roots: Vec<RootScan>,
    pub total: usize,
}

/// The set of enabled top-level mod folder names from an MO2 profile's
/// modlist.txt (lines starting with `+`), lowercased. None when unreadable —
/// the caller falls back to walking everything rather than scanning nothing.
fn enabled_mo2_mods(profile_dir: &str) -> Option<HashSet<String>> {
    let profile_dir = profile_dir.trim();
    if profile_dir.is_empty() {
        return None;
    }
    let text = std::fs::read_to_string(Path::new(profile_dir).join("modlist.txt")).ok()?;
    Some(
        text.lines()
            .filter_map(|line| line.strip_prefix('+'))
            .map(|name| name.trim().to_ascii_lowercase())
            .filter(|name| !name.is_empty())
            .collect(),
    )
}

fn collect_jslots_under(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in WalkDir::new(dir).follow_links(false).into_iter().flatten() {
        if !entry.file_type().is_file() {
            continue;
        }
        let is_jslot = entry
            .path()
            .extension()
            .map(|e| e.eq_ignore_ascii_case("jslot"))
            .unwrap_or(false);
        if is_jslot {
            out.push(entry.into_path());
        }
    }
}

/// Scan one root, honoring its kind.
fn scan_root(root: &JslotRoot, enabled_mods: Option<&HashSet<String>>) -> Vec<PathBuf> {
    let base = Path::new(root.path.trim());
    let mut found = Vec::new();
    if !base.is_dir() {
        return found;
    }
    match (root.kind, enabled_mods) {
        (RootKind::Mo2Mods, Some(enabled)) => {
            let Ok(entries) = std::fs::read_dir(base) else {
                return found;
            };
            for entry in entries.flatten() {
                if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                    continue;
                }
                let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
                if enabled.contains(&name) {
                    collect_jslots_under(&entry.path(), &mut found);
                }
            }
        }
        // Plain root, or an MO2 root with no readable modlist: walk everything.
        _ => collect_jslots_under(base, &mut found),
    }
    found
}

/// Scan every configured root for `.jslot` files. Pure function of the
/// settings — callers pass what they want scanned.
pub fn scan_settings(settings: &AppSettings) -> ScanResult {
    let enabled = matches!(settings.mod_manager, crate::settings::ModManagerKind::Mo2)
        .then(|| enabled_mo2_mods(&settings.mo2_profile_dir))
        .flatten();

    let mut files = Vec::new();
    let mut roots = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    for root in &settings.jslot_roots {
        let base = PathBuf::from(root.path.trim());
        let found = scan_root(root, enabled.as_ref());
        let mut count = 0usize;
        for path in found {
            // Roots can overlap (e.g. a user adds a subfolder of another
            // root) — a preset only counts once, for its first root.
            let key = path.display().to_string().to_ascii_lowercase();
            if !seen.insert(key) {
                continue;
            }
            count += 1;
            let meta = std::fs::metadata(&path).ok();
            let rel = path
                .strip_prefix(&base)
                .map(|r| r.display().to_string())
                .unwrap_or_else(|_| {
                    path.file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default()
                });
            files.push(JslotFile {
                path: path.display().to_string(),
                file_name: path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                root_id: root.id.clone(),
                root_label: root.label.clone(),
                rel_path: rel,
                size: meta.as_ref().map(|m| m.len()).unwrap_or(0),
                modified: meta
                    .and_then(|m| m.modified().ok())
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0),
            });
        }
        roots.push(RootScan {
            root_id: root.id.clone(),
            label: root.label.clone(),
            path: root.path.clone(),
            kind: root.kind,
            exists: base.is_dir(),
            count,
        });
    }

    files.sort_by(|a, b| a.file_name.to_ascii_lowercase().cmp(&b.file_name.to_ascii_lowercase()));
    let total = files.len();
    ScanResult { files, roots, total }
}

/// Scan the configured roots for `.jslot` files. Runs on a blocking worker —
/// MO2 mod trees are large.
#[tauri::command]
pub async fn scan_jslots(app: tauri::AppHandle) -> Result<ScanResult, String> {
    let settings = crate::settings::load_from_app(&app)?;
    tauri::async_runtime::spawn_blocking(move || Ok(scan_settings(&settings)))
        .await
        .map_err(|e| format!("scan task failed: {e}"))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::ModManagerKind;

    fn write(path: &Path, contents: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }

    #[test]
    fn recursive_scan_dedupes_overlapping_roots() {
        let root = std::env::temp_dir().join(format!("lineage-scan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        write(&root.join("a").join("deep").join("one.jslot"), "{}");
        write(&root.join("a").join("two.JSLOT"), "{}");
        write(&root.join("b").join("three.jslot"), "{}");

        let settings = AppSettings {
            jslot_roots: vec![
                JslotRoot {
                    id: "r1".into(),
                    label: "All".into(),
                    path: root.display().to_string(),
                    kind: RootKind::Plain,
                },
                JslotRoot {
                    id: "r2".into(),
                    label: "A again".into(),
                    path: root.join("a").display().to_string(),
                    kind: RootKind::Plain,
                },
            ],
            ..Default::default()
        };
        let result = scan_settings(&settings);
        assert_eq!(result.total, 3, "case-insensitive .jslot match, no dupes");
        assert_eq!(result.roots[0].count, 3);
        assert_eq!(result.roots[1].count, 0, "overlap deduped to first root");
        assert!(result.files.iter().any(|f| f.rel_path.contains("deep")));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn mo2_root_only_walks_enabled_mods() {
        let root = std::env::temp_dir().join(format!("lineage-scan-mo2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let mods = root.join("mods");
        write(&mods.join("Enabled Mod").join("p.jslot"), "{}");
        write(&mods.join("Disabled Mod").join("q.jslot"), "{}");
        let profile = root.join("profiles").join("Default");
        write(&profile.join("modlist.txt"), "+Enabled Mod\n-Disabled Mod\n");

        let settings = AppSettings {
            mod_manager: ModManagerKind::Mo2,
            mo2_profile_dir: profile.display().to_string(),
            jslot_roots: vec![JslotRoot {
                id: "mo2-mods".into(),
                label: "MO2 mods".into(),
                path: mods.display().to_string(),
                kind: RootKind::Mo2Mods,
            }],
            ..Default::default()
        };
        let result = scan_settings(&settings);
        assert_eq!(result.total, 1);
        assert!(result.files[0].path.contains("Enabled Mod"));

        let _ = std::fs::remove_dir_all(&root);
    }
}
