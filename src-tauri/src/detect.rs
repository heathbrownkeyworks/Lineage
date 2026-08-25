//! Mod-manager and game detection, plus default JSLOT root resolution.
//!
//! The detection approach mirrors Visage's environment.rs (Vortex manifest in
//! Data wins, then MO2 footprints, else Manual) and extends it with the
//! registry / common-path discovery Lineage needs for a zero-config first run:
//! Visage asks the user for folders first; Lineage auto-suggests them.
//!
//! Detection is a *suggestion* — the user can always override it in Settings,
//! and the override sticks.

use crate::settings::{AppSettings, JslotRoot, ModManagerKind, RootKind};
use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Mo2ProfileOption {
    pub name: String,
    pub path: String,
}

/// Everything first-run detection could discover, for the Settings UI to
/// pre-fill. Every field is a suggestion.
#[derive(Debug, Default, Serialize)]
pub struct DetectedEnvironment {
    /// "manual" | "mo2" | "vortex"
    pub manager: String,
    pub skyrim_folder: Option<String>,
    pub mo2_instance: Option<String>,
    pub mo2_mods_folder: Option<String>,
    pub mo2_profile_dir: Option<String>,
    pub mo2_profiles: Vec<Mo2ProfileOption>,
    pub vortex_present: bool,
    pub vortex_staging_folder: Option<String>,
}

/// Best-effort sweep of the machine: game install, MO2 instance, Vortex.
#[tauri::command]
pub fn detect_environment() -> DetectedEnvironment {
    let skyrim = find_skyrim_install();
    let mo2 = find_mo2_instance();
    let vortex_dir = vortex_app_dir();
    let vortex_present = vortex_dir.as_deref().map(Path::is_dir).unwrap_or(false);
    let vortex_staging = vortex_staging_folder(skyrim.as_deref());

    // Precedence mirrors Visage: a Vortex deployment manifest in Data is the
    // strongest signal, then MO2 footprints, else Manual.
    let vortex_manifest = skyrim
        .as_deref()
        .map(|s| s.join("Data").join(VORTEX_MANIFEST_NAME).is_file())
        .unwrap_or(false);
    let manager = if vortex_manifest {
        "vortex"
    } else if mo2.is_some() {
        "mo2"
    } else if vortex_present {
        "vortex"
    } else {
        "manual"
    };

    let (mo2_instance, mo2_mods, mo2_profile, mo2_profiles) = match &mo2 {
        Some(inst) => (
            Some(inst.root.display().to_string()),
            Some(inst.mods_dir.display().to_string()),
            inst.selected_profile_dir
                .as_ref()
                .map(|p| p.display().to_string()),
            inst.profiles.clone(),
        ),
        None => (None, None, None, Vec::new()),
    };

    DetectedEnvironment {
        manager: manager.to_string(),
        skyrim_folder: skyrim.map(|p| p.display().to_string()),
        mo2_instance,
        mo2_mods_folder: mo2_mods,
        mo2_profile_dir: mo2_profile,
        mo2_profiles,
        vortex_present,
        vortex_staging_folder: vortex_staging.map(|p| p.display().to_string()),
    }
}

/// Vortex writes this manifest into the game Data dir on deploy. It maps
/// deployed files back to their source mod folders (see NOTES.md for schema).
pub const VORTEX_MANIFEST_NAME: &str = "vortex.deployment.json";

// ---------------------------------------------------------------------------
// Skyrim install discovery
// ---------------------------------------------------------------------------

fn find_skyrim_install() -> Option<PathBuf> {
    // 1. Bethesda's own registry entry (set by the Steam installer).
    if let Some(path) = registry_string(
        winreg::enums::HKEY_LOCAL_MACHINE,
        r"SOFTWARE\WOW6432Node\Bethesda Softworks\Skyrim Special Edition",
        "Installed Path",
    ) {
        let p = PathBuf::from(path.trim().trim_matches('"'));
        if p.join("SkyrimSE.exe").is_file() {
            return Some(p);
        }
    }
    // 2. Steam library folders.
    if let Some(steam) = registry_string(
        winreg::enums::HKEY_LOCAL_MACHINE,
        r"SOFTWARE\WOW6432Node\Valve\Steam",
        "InstallPath",
    ) {
        let vdf = PathBuf::from(&steam)
            .join("steamapps")
            .join("libraryfolders.vdf");
        for lib in steam_library_paths(&vdf) {
            let p = lib
                .join("steamapps")
                .join("common")
                .join("Skyrim Special Edition");
            if p.join("SkyrimSE.exe").is_file() {
                return Some(p);
            }
        }
    }
    // 3. GOG.
    for gog_id in ["1711230643", "1801825368"] {
        if let Some(path) = registry_string(
            winreg::enums::HKEY_LOCAL_MACHINE,
            &format!(r"SOFTWARE\WOW6432Node\GOG.com\Games\{gog_id}"),
            "path",
        ) {
            let p = PathBuf::from(path.trim());
            if p.join("SkyrimSE.exe").is_file() {
                return Some(p);
            }
        }
    }
    None
}

/// Parse the "path" entries out of Steam's libraryfolders.vdf. Line-based on
/// purpose — the VDF format is stable enough for this one key.
fn steam_library_paths(vdf: &Path) -> Vec<PathBuf> {
    let Ok(text) = std::fs::read_to_string(vdf) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            let rest = line.strip_prefix("\"path\"")?.trim();
            let value = rest.trim_matches('"');
            (!value.is_empty()).then(|| PathBuf::from(value.replace("\\\\", "\\")))
        })
        .collect()
}

// ---------------------------------------------------------------------------
// MO2 discovery
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct Mo2Instance {
    root: PathBuf,
    mods_dir: PathBuf,
    selected_profile_dir: Option<PathBuf>,
    profiles: Vec<Mo2ProfileOption>,
}

/// Find an MO2 instance: the registry's current instance, the nxm:// protocol
/// handler (portable installs), then global instances under %LOCALAPPDATA%.
fn find_mo2_instance() -> Option<Mo2Instance> {
    for root in mo2_instance_candidates() {
        if let Some(inst) = read_mo2_instance(&root) {
            return Some(inst);
        }
    }
    None
}

fn mo2_instance_candidates() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    let mut push = |p: PathBuf| {
        if !out.iter().any(|x| x == &p) {
            out.push(p);
        }
    };

    // Registry: MO2 records its current instance name; global instances live
    // under %LOCALAPPDATA%\ModOrganizer\<name>.
    let current = registry_string(
        winreg::enums::HKEY_CURRENT_USER,
        r"Software\Mod Organizer Team\Mod Organizer",
        "CurrentInstance",
    );
    let local_appdata = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
    if let (Some(name), Some(base)) = (current.as_deref(), local_appdata.as_deref()) {
        let name = name.trim();
        if !name.is_empty() && !name.eq_ignore_ascii_case("portable") {
            push(base.join("ModOrganizer").join(name));
        }
    }

    // nxm:// protocol handler — the strongest signal for portable installs.
    // Value looks like: "D:\Nordic Souls\nxmhandler.exe" "%1"
    for hive_path in [
        r"Software\Classes\nxm\shell\open\command",
        r"SOFTWARE\Classes\nxm\shell\open\command",
    ] {
        if let Some(cmd) = registry_string(winreg::enums::HKEY_CURRENT_USER, hive_path, "")
            .or_else(|| registry_string(winreg::enums::HKEY_LOCAL_MACHINE, hive_path, ""))
        {
            if let Some(exe) = first_quoted_token(&cmd) {
                if let Some(dir) = PathBuf::from(exe).parent() {
                    push(dir.to_path_buf());
                }
            }
        }
    }

    // Every global instance folder.
    if let Some(base) = local_appdata.as_deref() {
        if let Ok(entries) = std::fs::read_dir(base.join("ModOrganizer")) {
            for entry in entries.flatten() {
                if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                    push(entry.path());
                }
            }
        }
    }

    // Common portable install locations, last.
    for guess in [
        r"C:\Modding\MO2",
        r"C:\Mod Organizer 2",
        r"C:\MO2",
        r"D:\MO2",
        r"D:\Modding\MO2",
    ] {
        push(PathBuf::from(guess));
    }
    out
}

/// `"C:\path with spaces\x.exe" "%1"` → `C:\path with spaces\x.exe`
fn first_quoted_token(cmd: &str) -> Option<&str> {
    let cmd = cmd.trim();
    if let Some(rest) = cmd.strip_prefix('"') {
        rest.split('"').next()
    } else {
        cmd.split_whitespace().next()
    }
}

/// Read an MO2 instance from a folder holding ModOrganizer.ini. Resolves the
/// mods / profiles folders (honoring [Settings] overrides + %BASE_DIR%) and
/// the selected profile.
fn read_mo2_instance(root: &Path) -> Option<Mo2Instance> {
    let ini_path = root.join("ModOrganizer.ini");
    let ini = std::fs::read_to_string(&ini_path).ok()?;
    let get = |key: &str| ini_value(&ini, key).map(|v| unwrap_bytearray(&v));

    let base_dir = get("base_directory")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.to_path_buf());
    let resolve = |raw: Option<String>, default_leaf: &str| -> PathBuf {
        match raw {
            Some(v) if !v.trim().is_empty() => {
                let v = v.replace("%BASE_DIR%", &base_dir.display().to_string());
                PathBuf::from(v)
            }
            _ => base_dir.join(default_leaf),
        }
    };
    let mods_dir = resolve(get("mod_directory"), "mods");
    let profiles_dir = resolve(get("profile_directory"), "profiles");
    if !mods_dir.is_dir() {
        return None;
    }

    let profiles = list_profiles(&profiles_dir);
    let selected = get("selected_profile")
        .map(|name| profiles_dir.join(name))
        .filter(|p| p.is_dir())
        .or_else(|| profiles.first().map(|p| PathBuf::from(&p.path)));

    Some(Mo2Instance {
        root: root.to_path_buf(),
        mods_dir,
        selected_profile_dir: selected,
        profiles,
    })
}

fn list_profiles(profiles_dir: &Path) -> Vec<Mo2ProfileOption> {
    let Ok(entries) = std::fs::read_dir(profiles_dir) else {
        return Vec::new();
    };
    let mut out: Vec<Mo2ProfileOption> = entries
        .flatten()
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .filter(|e| e.path().join("modlist.txt").is_file())
        .map(|e| Mo2ProfileOption {
            name: e.file_name().to_string_lossy().into_owned(),
            path: e.path().display().to_string(),
        })
        .collect();
    out.sort_by_key(|p| p.name.to_ascii_lowercase());
    out
}

/// Grab `key=value` from a Qt ini, returning the raw value. Only the first
/// occurrence matters for the keys we read ([General]/[Settings] don't repeat).
fn ini_value(ini: &str, key: &str) -> Option<String> {
    ini.lines().find_map(|line| {
        let line = line.trim();
        let rest = line.strip_prefix(key)?;
        let rest = rest.trim_start();
        let rest = rest.strip_prefix('=')?;
        Some(rest.trim().to_string())
    })
}

/// Qt writes some ini values as `@ByteArray(D:\\path\\here)`. Unwrap and
/// unescape; plain values pass through unchanged.
fn unwrap_bytearray(value: &str) -> String {
    let inner = value
        .strip_prefix("@ByteArray(")
        .and_then(|v| v.strip_suffix(')'))
        .unwrap_or(value);
    inner.replace("\\\\", "\\")
}

/// List usable MO2 profiles next to a mods folder — drives the Settings
/// profile dropdown (same contract as Visage's list_mo2_profiles).
#[tauri::command]
pub fn list_mo2_profiles(mods_dir: String) -> Result<Vec<Mo2ProfileOption>, String> {
    let mods_dir = mods_dir.trim().to_string();
    if mods_dir.is_empty() {
        return Ok(Vec::new());
    }
    let profiles_dir = Path::new(&mods_dir)
        .parent()
        .ok_or_else(|| format!("MO2 mods folder has no parent: {mods_dir}"))?
        .join("profiles");
    Ok(list_profiles(&profiles_dir))
}

// ---------------------------------------------------------------------------
// Vortex discovery
// ---------------------------------------------------------------------------

fn vortex_app_dir() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|d| PathBuf::from(d).join("Vortex"))
}

/// Vortex's default staging folder for Skyrim SE. The user can relocate it in
/// Vortex; this is only the suggestion.
fn vortex_staging_folder(_skyrim: Option<&Path>) -> Option<PathBuf> {
    let dir = vortex_app_dir()?.join("skyrimse").join("mods");
    dir.is_dir().then_some(dir)
}

// ---------------------------------------------------------------------------
// Registry helper
// ---------------------------------------------------------------------------

fn registry_string(hive: winreg::HKEY, path: &str, value: &str) -> Option<String> {
    let key = winreg::RegKey::predef(hive).open_subkey(path).ok()?;
    key.get_value::<String, _>(value).ok()
}

// ---------------------------------------------------------------------------
// Default JSLOT roots per manager mode
// ---------------------------------------------------------------------------

/// The default scan roots for the current settings. Stable ids let the
/// frontend re-apply defaults without duplicating rows the user already has.
#[tauri::command]
pub fn default_jslot_roots(settings: AppSettings) -> Vec<JslotRoot> {
    let mut roots = Vec::new();
    match settings.mod_manager {
        ModManagerKind::Manual => {
            if let Some(data) = settings.data_dir() {
                roots.push(JslotRoot {
                    id: "manual-presets".into(),
                    label: "Game presets".into(),
                    path: data
                        .join("SKSE")
                        .join("Plugins")
                        .join("CharGen")
                        .join("Presets")
                        .display()
                        .to_string(),
                    kind: RootKind::Plain,
                });
            }
        }
        ModManagerKind::Mo2 => {
            let mods = settings.mo2_mods_folder.trim();
            if !mods.is_empty() {
                // One root of kind Mo2Mods = "every enabled mod folder in the
                // active profile" — the scanner filters to enabled mods.
                roots.push(JslotRoot {
                    id: "mo2-mods".into(),
                    label: "MO2 mods (enabled)".into(),
                    path: mods.to_string(),
                    kind: RootKind::Mo2Mods,
                });
            }
            let instance = settings.mo2_instance.trim();
            if !instance.is_empty() {
                roots.push(JslotRoot {
                    id: "mo2-overwrite".into(),
                    label: "MO2 overwrite".into(),
                    path: Path::new(instance).join("overwrite").display().to_string(),
                    kind: RootKind::Plain,
                });
            }
        }
        ModManagerKind::Vortex => {
            let staging = settings.vortex_staging_folder.trim();
            if !staging.is_empty() {
                roots.push(JslotRoot {
                    id: "vortex-staging".into(),
                    label: "Vortex staging".into(),
                    path: staging.to_string(),
                    kind: RootKind::Plain,
                });
            }
            if let Some(data) = settings.data_dir() {
                roots.push(JslotRoot {
                    id: "vortex-data".into(),
                    label: "Game Data".into(),
                    path: data.display().to_string(),
                    kind: RootKind::Plain,
                });
            }
        }
    }
    roots
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytearray_values_unwrap_and_unescape() {
        assert_eq!(
            unwrap_bytearray(r"@ByteArray(D:\\Nordic Souls\\Game Root)"),
            r"D:\Nordic Souls\Game Root"
        );
        assert_eq!(
            unwrap_bytearray("@ByteArray(Nordic Souls - ENB)"),
            "Nordic Souls - ENB"
        );
        assert_eq!(unwrap_bytearray("plain"), "plain");
    }

    #[test]
    fn ini_lookup_finds_first_match() {
        let ini = "[General]\ngamePath=@ByteArray(D:\\\\Games)\nselected_profile=@ByteArray(Default)\n";
        assert_eq!(ini_value(ini, "gamePath").as_deref(), Some("@ByteArray(D:\\\\Games)"));
        assert_eq!(ini_value(ini, "missing"), None);
    }

    #[test]
    fn quoted_command_token_extracts_exe() {
        assert_eq!(
            first_quoted_token(r#""D:\Nordic Souls\nxmhandler.exe" "%1""#),
            Some(r"D:\Nordic Souls\nxmhandler.exe")
        );
        assert_eq!(
            first_quoted_token(r"C:\mo2\nxmhandler.exe %1"),
            Some(r"C:\mo2\nxmhandler.exe")
        );
    }

    #[test]
    fn default_roots_follow_manager_mode() {
        let s = AppSettings {
            mod_manager: ModManagerKind::Mo2,
            mo2_instance: r"D:\NS".into(),
            mo2_mods_folder: r"D:\NS\mods".into(),
            ..Default::default()
        };
        let roots = default_jslot_roots(s);
        assert_eq!(roots.len(), 2);
        assert_eq!(roots[0].id, "mo2-mods");
        assert_eq!(roots[0].kind, RootKind::Mo2Mods);
        assert_eq!(roots[1].path, r"D:\NS\overwrite");

        let s = AppSettings {
            mod_manager: ModManagerKind::Manual,
            skyrim_folder: r"D:\Skyrim".into(),
            ..Default::default()
        };
        let roots = default_jslot_roots(s);
        assert_eq!(roots.len(), 1);
        assert!(roots[0].path.ends_with(r"Data\SKSE\Plugins\CharGen\Presets"));
    }
}
