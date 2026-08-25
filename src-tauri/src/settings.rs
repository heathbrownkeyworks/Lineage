//! Typed app settings, persisted as JSON in the Tauri app config dir.
//!
//! Follows Visage's settings.rs conventions: missing file or missing fields
//! deserialize to defaults so settings written by older builds load cleanly,
//! and every command surfaces errors as human-readable strings.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ModManagerKind {
    /// No mod manager — the user points Lineage at folders directly.
    #[default]
    Manual,
    Mo2,
    Vortex,
}

/// How a configured root should be scanned.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RootKind {
    /// Walk the whole tree.
    #[default]
    Plain,
    /// An MO2 `mods` folder: only walk top-level mod folders that are enabled
    /// in the active profile's modlist.txt. This is how Lineage represents
    /// "every enabled mod folder in the active profile" without listing
    /// hundreds of roots individually.
    Mo2Mods,
}

/// One root path Lineage scans (always recursively) for `.jslot` files.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct JslotRoot {
    /// Stable id — default roots use well-known ids ("mo2-mods", "manual-presets"…)
    /// so re-applying defaults never duplicates them.
    pub id: String,
    /// Short human label; also prefixes this root's entries inside backup zips.
    pub label: String,
    pub path: String,
    #[serde(default)]
    pub kind: RootKind,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AppSettings {
    #[serde(default)]
    pub mod_manager: ModManagerKind,
    /// Root Skyrim SE install folder (the one containing SkyrimSE.exe).
    #[serde(default)]
    pub skyrim_folder: String,
    /// MO2 instance folder (contains ModOrganizer.ini).
    #[serde(default)]
    pub mo2_instance: String,
    /// MO2 `mods` folder.
    #[serde(default)]
    pub mo2_mods_folder: String,
    /// MO2 active profile folder (contains modlist.txt).
    #[serde(default)]
    pub mo2_profile_dir: String,
    /// Vortex staging folder (e.g. %APPDATA%\Vortex\skyrimse\mods).
    #[serde(default)]
    pub vortex_staging_folder: String,
    /// The core setting: root paths scanned recursively for `.jslot` files.
    #[serde(default)]
    pub jslot_roots: Vec<JslotRoot>,
    /// Where backup archives and operation snapshots are written.
    #[serde(default)]
    pub backup_dir: String,
    /// How many operation snapshots to keep (full backups are never pruned).
    #[serde(default = "default_snapshot_retention")]
    pub snapshot_retention: u32,
    /// Personal Nexus API key. Stored here per spec; never logged, never
    /// echoed into error messages, masked in the UI once saved.
    #[serde(default)]
    pub nexus_api_key: String,
    /// Unix seconds of the last successful full backup.
    #[serde(default)]
    pub last_backup_at: Option<i64>,
    /// Archive path of the last successful full backup.
    #[serde(default)]
    pub last_backup_path: Option<String>,
    /// User dismissed the first-run setup prompt — don't auto-open Settings.
    #[serde(default)]
    pub setup_dismissed: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            mod_manager: ModManagerKind::default(),
            skyrim_folder: String::new(),
            mo2_instance: String::new(),
            mo2_mods_folder: String::new(),
            mo2_profile_dir: String::new(),
            vortex_staging_folder: String::new(),
            jslot_roots: Vec::new(),
            backup_dir: String::new(),
            snapshot_retention: default_snapshot_retention(),
            nexus_api_key: String::new(),
            last_backup_at: None,
            last_backup_path: None,
            setup_dismissed: false,
        }
    }
}

fn default_snapshot_retention() -> u32 {
    10
}

impl AppSettings {
    /// The game's Data dir, derived from the Skyrim install root. Empty
    /// skyrim_folder yields None — callers treat that as "not configured".
    pub fn data_dir(&self) -> Option<PathBuf> {
        let trimmed = self.skyrim_folder.trim();
        (!trimmed.is_empty()).then(|| Path::new(trimmed).join("Data"))
    }

    /// The backup destination, or None when unset.
    pub fn backup_dir_path(&self) -> Option<PathBuf> {
        let trimmed = self.backup_dir.trim();
        (!trimmed.is_empty()).then(|| PathBuf::from(trimmed))
    }

    /// The Snapshots subfolder of the backup destination.
    pub fn snapshots_dir(&self) -> Option<PathBuf> {
        self.backup_dir_path().map(|p| p.join("Snapshots"))
    }
}

fn settings_path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    use tauri::Manager;
    let dir = app
        .path()
        .app_config_dir()
        .map_err(|e| format!("failed to resolve app config dir: {e}"))?;
    Ok(dir.join("settings.json"))
}

/// Load settings from disk for backend consumers. Missing file or missing
/// fields deserialize to defaults.
pub fn load_from_app(app: &tauri::AppHandle) -> Result<AppSettings, String> {
    let path = settings_path(app)?;
    if !path.exists() {
        return Ok(AppSettings::default());
    }
    let bytes = std::fs::read(&path).map_err(|e| format!("read settings: {e}"))?;
    serde_json::from_slice(&bytes).map_err(|e| format!("parse settings: {e}"))
}

pub fn save_to_app(app: &tauri::AppHandle, settings: &AppSettings) -> Result<(), String> {
    let path = settings_path(app)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("create app config dir: {e}"))?;
    }
    let json =
        serde_json::to_string_pretty(settings).map_err(|e| format!("serialize settings: {e}"))?;
    std::fs::write(&path, json).map_err(|e| format!("write settings: {e}"))
}

/// Read persisted settings for the frontend.
#[tauri::command]
pub fn get_settings(app: tauri::AppHandle) -> Result<AppSettings, String> {
    load_from_app(&app)
}

/// Persist settings as pretty JSON, creating the app config dir if needed.
#[tauri::command]
pub fn save_settings(app: tauri::AppHandle, settings: AppSettings) -> Result<(), String> {
    save_to_app(&app, &settings)
}

/// The default backup destination: `Documents\Lineage\Backups`. Deliberately
/// outside the Skyrim install and any mod manager tree.
#[tauri::command]
pub fn default_backup_dir(app: tauri::AppHandle) -> Result<String, String> {
    use tauri::Manager;
    let docs = app
        .path()
        .document_dir()
        .map_err(|e| format!("failed to resolve Documents folder: {e}"))?;
    Ok(docs.join("Lineage").join("Backups").display().to_string())
}

/// What `validate_backup_dir` reports back to the settings UI.
#[derive(Debug, Serialize)]
pub struct BackupDirCheck {
    /// The folder exists (or could be created) and a test write succeeded.
    pub writable: bool,
    /// Human explanation when not writable.
    pub problem: Option<String>,
    /// Set when the path sits inside the game Data dir, an MO2 mods folder,
    /// or a Vortex staging folder — archives left in a mod tree get picked
    /// up by the mod manager and can end up deployed into the game.
    pub inside_mod_tree: Option<String>,
}

/// Validate a backup destination: writable, and not inside a mod tree.
#[tauri::command]
pub fn validate_backup_dir(app: tauri::AppHandle, path: String) -> Result<BackupDirCheck, String> {
    let settings = load_from_app(&app)?;
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Ok(BackupDirCheck {
            writable: false,
            problem: Some("No backup folder set. Pick a folder to hold your backups.".into()),
            inside_mod_tree: None,
        });
    }
    let dir = PathBuf::from(trimmed);

    // Warn when the destination lands inside a tree a mod manager owns.
    let mut danger_zones: Vec<(PathBuf, &str)> = Vec::new();
    if let Some(data) = settings.data_dir() {
        danger_zones.push((data, "your Skyrim Data folder"));
    }
    let mods = settings.mo2_mods_folder.trim();
    if !mods.is_empty() {
        danger_zones.push((PathBuf::from(mods), "your MO2 mods folder"));
    }
    let staging = settings.vortex_staging_folder.trim();
    if !staging.is_empty() {
        danger_zones.push((PathBuf::from(staging), "your Vortex staging folder"));
    }
    let inside_mod_tree = danger_zones.iter().find_map(|(zone, name)| {
        path_is_inside(&dir, zone).then(|| {
            format!(
                "This folder is inside {name}. Archives saved here can be picked up by your mod manager and deployed into the game — pick a folder outside your mod setup."
            )
        })
    });

    // Writability: create if needed, then try a real write.
    if let Err(e) = std::fs::create_dir_all(&dir) {
        return Ok(BackupDirCheck {
            writable: false,
            problem: Some(format!(
                "Lineage can't create this folder ({e}). Pick a different location."
            )),
            inside_mod_tree,
        });
    }
    let probe = dir.join(".lineage-write-test");
    let write_result = std::fs::write(&probe, b"ok");
    let _ = std::fs::remove_file(&probe);
    match write_result {
        Ok(()) => Ok(BackupDirCheck {
            writable: true,
            problem: None,
            inside_mod_tree,
        }),
        Err(e) => Ok(BackupDirCheck {
            writable: false,
            problem: Some(format!(
                "Lineage can't write to this folder ({e}). Pick a different location."
            )),
            inside_mod_tree,
        }),
    }
}

/// Is `path` equal to or nested under `ancestor`? Compared case-insensitively
/// on normalized components — good enough for the Windows paths we handle.
pub fn path_is_inside(path: &Path, ancestor: &Path) -> bool {
    let norm = |p: &Path| -> Vec<String> {
        p.components()
            .map(|c| c.as_os_str().to_string_lossy().to_ascii_lowercase())
            .collect()
    };
    let path = norm(path);
    let ancestor = norm(ancestor);
    !ancestor.is_empty() && path.len() >= ancestor.len() && path[..ancestor.len()] == ancestor[..]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_settings_json_deserializes_with_defaults() {
        let s: AppSettings = serde_json::from_str(r#"{"skyrim_folder":"D:\\Games\\Skyrim"}"#).unwrap();
        assert_eq!(s.mod_manager, ModManagerKind::Manual);
        assert_eq!(s.snapshot_retention, 10);
        assert!(s.jslot_roots.is_empty());
        assert!(s.last_backup_at.is_none());
    }

    #[test]
    fn path_inside_checks_are_case_insensitive() {
        assert!(path_is_inside(
            Path::new(r"D:\Nordic Souls\MODS\backup"),
            Path::new(r"d:\nordic souls\mods")
        ));
        assert!(!path_is_inside(
            Path::new(r"D:\Backups"),
            Path::new(r"D:\Nordic Souls\mods")
        ));
        assert!(!path_is_inside(Path::new(r"D:\Backups"), Path::new("")));
    }

    #[test]
    fn data_dir_derives_from_skyrim_folder() {
        let s = AppSettings {
            skyrim_folder: r"D:\Games\Skyrim".into(),
            ..Default::default()
        };
        assert_eq!(s.data_dir(), Some(PathBuf::from(r"D:\Games\Skyrim\Data")));
        assert_eq!(AppSettings::default().data_dir(), None);
    }
}
