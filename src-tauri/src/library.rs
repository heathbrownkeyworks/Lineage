//! The Asset Library: user-taught (and seed) mappings from preset references
//! to mod names and links. Two layers — a seed list compiled into the binary
//! and the user's library.json in the app config dir — merged at load with
//! user entries shadowing seed entries.

use crate::jslot;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const SEED_JSON: &str = include_str!("../resources/seed-library.json");

/// Canonical comparison form for an asset reference: lowercase, backslashes,
/// and no leading `Data\` / `Textures\`. Shared by the library matcher and the
/// asset analysis in `assets.rs`; the two have to agree, or the pattern shown
/// for a group of unknown references would not match those references.
pub(crate) fn normalize_ref(s: &str) -> String {
    let lower = s.trim().to_ascii_lowercase().replace('/', "\\");
    let mut p = lower.trim_start_matches('\\');
    p = p.strip_prefix("data\\").unwrap_or(p);
    p = p.strip_prefix("textures\\").unwrap_or(p);
    p.to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LibraryEntry {
    /// "user-<unix-millis>" or "seed-<slug>".
    pub id: String,
    /// "plugin" | "texture" | "morph"
    pub kind: String,
    pub pattern: String,
    /// "exact" | "prefix"
    pub match_type: String,
    pub name: String,
    pub url: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct MergedEntry {
    #[serde(flatten)]
    pub entry: LibraryEntry,
    /// "seed" | "user"
    pub source: String,
    pub enabled: bool,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct UserLibraryFile {
    #[serde(default)]
    pub entries: Vec<LibraryEntry>,
    #[serde(default)]
    pub disabled_seed_ids: Vec<String>,
}

#[derive(Debug)]
pub struct Library {
    pub entries: Vec<MergedEntry>,
    /// Set when library.json was unreadable — the app runs seed-only.
    pub warning: Option<String>,
}

pub fn user_file_path(config_dir: &Path) -> PathBuf {
    config_dir.join("library.json")
}

pub fn load_user_file(config_dir: &Path) -> (UserLibraryFile, Option<String>) {
    let path = user_file_path(config_dir);
    if !path.exists() {
        return (UserLibraryFile::default(), None);
    }
    match std::fs::read_to_string(&path)
        .map_err(|e| e.to_string())
        .and_then(|t| serde_json::from_str::<UserLibraryFile>(&t).map_err(|e| e.to_string()))
    {
        Ok(file) => (file, None),
        Err(e) => (
            UserLibraryFile::default(),
            Some(format!(
                "Your library file couldn't be read ({e}). Lineage is running with built-in entries only; fix or delete {} to restore your additions.",
                path.display()
            )),
        ),
    }
}

pub fn save_user_file(config_dir: &Path, file: &UserLibraryFile) -> Result<(), String> {
    std::fs::create_dir_all(config_dir).map_err(|e| format!("create config dir: {e}"))?;
    let json = serde_json::to_string_pretty(file).map_err(|e| format!("serialize library: {e}"))?;
    jslot::write_atomic(&user_file_path(config_dir), json.as_bytes())
}

fn shadow_key(e: &LibraryEntry) -> (String, String, String) {
    (
        e.kind.to_ascii_lowercase(),
        e.match_type.to_ascii_lowercase(),
        e.pattern.to_ascii_lowercase(),
    )
}

pub fn load_from_dir(config_dir: &Path) -> Library {
    let seed: Vec<LibraryEntry> = serde_json::from_str(SEED_JSON)
        .expect("seed-library.json is validated by tests at build time");
    let (user, warning) = load_user_file(config_dir);

    let user_keys: std::collections::HashSet<_> = user.entries.iter().map(shadow_key).collect();
    let mut entries: Vec<MergedEntry> = user
        .entries
        .iter()
        .map(|e| MergedEntry {
            entry: e.clone(),
            source: "user".into(),
            enabled: true,
        })
        .collect();
    for e in seed {
        let disabled = user.disabled_seed_ids.contains(&e.id) || user_keys.contains(&shadow_key(&e));
        entries.push(MergedEntry {
            entry: e,
            source: "seed".into(),
            enabled: !disabled,
        });
    }
    Library { entries, warning }
}

impl Library {
    /// Best enabled entry for a reference. Specificity: exact beats prefix,
    /// longer prefix beats shorter, user beats seed. `user_only` restricts to
    /// user entries (the override layer that beats the automatic pipeline).
    pub fn match_entry(&self, kind: &str, value: &str, user_only: bool) -> Option<&MergedEntry> {
        // Both sides go through `normalize_ref`: plugin/morph patterns and
        // values never contain slashes, so this only affects texture refs.
        // There a forward-slash value must still hit a backslash pattern,
        // and — because presets spell the same mod's path both with and
        // without a `Data\Textures\` lead-in — one folder pattern has to
        // claim both spellings, or an entry silently leaves some of the very
        // references it was created from unmatched.
        let value_l = normalize_ref(value);
        self.entries
            .iter()
            .filter(|m| m.enabled)
            .filter(|m| !user_only || m.source == "user")
            .filter(|m| m.entry.kind.eq_ignore_ascii_case(kind))
            .filter(|m| {
                let p = normalize_ref(&m.entry.pattern);
                match m.entry.match_type.as_str() {
                    "prefix" => value_l.starts_with(&p),
                    _ => value_l == p,
                }
            })
            .max_by_key(|m| {
                let exact = (m.entry.match_type != "prefix") as usize;
                let user = (m.source == "user") as usize;
                // exact dominates, then pattern length, then user-over-seed.
                (exact, m.entry.pattern.len(), user)
            })
    }
}

/// The mod id from a nexusmods.com mod URL, else None.
pub fn nexus_mod_id_from_url(url: &str) -> Option<u32> {
    let lower = url.to_ascii_lowercase();
    let host_ok = lower.starts_with("https://www.nexusmods.com/")
        || lower.starts_with("https://nexusmods.com/")
        || lower.starts_with("http://www.nexusmods.com/")
        || lower.starts_with("http://nexusmods.com/");
    if !host_ok {
        return None;
    }
    let after = lower.split_once("/mods/")?.1;
    let digits: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

/// Validation shared by every write path. Human-readable errors.
fn validate(entry: &LibraryEntry) -> Result<(), String> {
    if entry.name.trim().is_empty() {
        return Err("Give the mod a name.".into());
    }
    let url = entry.url.trim();
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err("The link must be a full http(s) URL.".into());
    }
    if !matches!(entry.kind.as_str(), "plugin" | "texture" | "morph") {
        return Err("Unknown reference kind.".into());
    }
    match entry.match_type.as_str() {
        "exact" if !entry.pattern.trim().is_empty() => Ok(()),
        "prefix" if entry.pattern.trim().len() >= 3 => Ok(()),
        "prefix" => Err("A prefix pattern needs at least 3 characters.".into()),
        _ => Err("Pattern can't be empty.".into()),
    }
}

/// Shared by every write path: load the user file, apply `mutate`, then
/// save. If the load reported a corruption warning, the unreadable file is
/// first renamed aside to `library.json.invalid-<unix-millis>` so the fresh
/// write below doesn't silently destroy the user's damaged-but-recoverable
/// data — `load_user_file`'s warning text tells them to "fix or delete" it,
/// which is only true if a write doesn't get there first.
fn write_user_file(
    config_dir: &Path,
    mutate: impl FnOnce(&mut UserLibraryFile),
) -> Result<(), String> {
    let (mut file, warning) = load_user_file(config_dir);
    if warning.is_some() {
        let path = user_file_path(config_dir);
        if path.exists() {
            let millis = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0);
            let aside = config_dir.join(format!("library.json.invalid-{millis}"));
            std::fs::rename(&path, &aside)
                .map_err(|e| format!("preserve unreadable library file: {e}"))?;
        }
    }
    mutate(&mut file);
    save_user_file(config_dir, &file)
}

pub fn upsert_entries(config_dir: &Path, entries: Vec<LibraryEntry>) -> Result<(), String> {
    for e in &entries {
        validate(e)?;
    }
    write_user_file(config_dir, |file| {
        for mut e in entries {
            if e.id.trim().is_empty() {
                let millis = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis())
                    .unwrap_or(0);
                e.id = format!("user-{millis}-{}", file.entries.len());
            }
            // Replace by id, else by shadow key (same kind+match+pattern).
            file.entries
                .retain(|x| x.id != e.id && shadow_key(x) != shadow_key(&e));
            file.entries.push(e);
        }
    })
}

pub fn delete_entry(config_dir: &Path, id: &str) -> Result<(), String> {
    write_user_file(config_dir, |file| {
        if id.starts_with("seed-") {
            if !file.disabled_seed_ids.iter().any(|x| x == id) {
                file.disabled_seed_ids.push(id.to_string());
            }
        } else {
            file.entries.retain(|x| x.id != id);
        }
    })
}

pub fn restore_seed(config_dir: &Path, id: &str) -> Result<(), String> {
    write_user_file(config_dir, |file| {
        file.disabled_seed_ids.retain(|x| x != id);
    })
}

fn config_dir(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    use tauri::Manager;
    app.path()
        .app_config_dir()
        .map_err(|e| format!("failed to resolve app config dir: {e}"))
}

#[derive(Debug, Serialize)]
pub struct LibraryListing {
    pub entries: Vec<MergedEntry>,
    pub warning: Option<String>,
}

#[tauri::command]
pub fn library_list(app: tauri::AppHandle) -> Result<LibraryListing, String> {
    let lib = load_from_dir(&config_dir(&app)?);
    Ok(LibraryListing { entries: lib.entries, warning: lib.warning })
}

#[tauri::command]
pub fn library_save_entries(app: tauri::AppHandle, entries: Vec<LibraryEntry>) -> Result<(), String> {
    upsert_entries(&config_dir(&app)?, entries)
}

#[tauri::command]
pub fn library_delete_entry(app: tauri::AppHandle, id: String) -> Result<(), String> {
    delete_entry(&config_dir(&app)?, &id)
}

#[tauri::command]
pub fn library_restore_seed(app: tauri::AppHandle, id: String) -> Result<(), String> {
    restore_seed(&config_dir(&app)?, &id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("lineage-lib-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn user_entry(id: &str, kind: &str, pattern: &str, match_type: &str) -> LibraryEntry {
        LibraryEntry {
            id: id.into(), kind: kind.into(), pattern: pattern.into(),
            match_type: match_type.into(), name: format!("{id} name"),
            url: "https://example.com/mod".into(),
        }
    }

    #[test]
    fn upsert_replaces_by_id_and_by_shadow_key_and_assigns_ids() {
        let dir = temp_dir("upsert");
        // New entry with empty id gets a user- id assigned.
        let mut e = user_entry("", "plugin", "HG Hairdos 2.esp", "exact");
        upsert_entries(&dir, vec![e.clone()]).unwrap();
        let (file, _) = load_user_file(&dir);
        assert_eq!(file.entries.len(), 1);
        assert!(file.entries[0].id.starts_with("user-"));

        // Same (kind, match_type, pattern), different id → replaces, no dupe.
        e.id = String::new();
        e.name = "Corrected name".into();
        upsert_entries(&dir, vec![e]).unwrap();
        let (file, _) = load_user_file(&dir);
        assert_eq!(file.entries.len(), 1);
        assert_eq!(file.entries[0].name, "Corrected name");

        // Update by id keeps one entry even when the pattern changes.
        let mut existing = file.entries[0].clone();
        existing.pattern = "HG Hairdos".into();
        existing.match_type = "prefix".into();
        upsert_entries(&dir, vec![existing]).unwrap();
        let (file, _) = load_user_file(&dir);
        assert_eq!(file.entries.len(), 1);
        assert_eq!(file.entries[0].match_type, "prefix");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn upsert_validates_inputs() {
        let dir = temp_dir("validate");
        let mut bad = user_entry("", "plugin", "X.esp", "exact");
        bad.url = "ftp://nope".into();
        assert!(upsert_entries(&dir, vec![bad]).is_err());
        let mut bad = user_entry("", "plugin", "ab", "prefix"); // prefix min length 3
        bad.url = "https://ok.example".into();
        assert!(upsert_entries(&dir, vec![bad]).is_err());
        let mut bad = user_entry("", "plugin", "X.esp", "exact");
        bad.name = "  ".into();
        assert!(upsert_entries(&dir, vec![bad]).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn delete_removes_user_entry_and_disables_seed() {
        let dir = temp_dir("delete");
        upsert_entries(&dir, vec![user_entry("", "plugin", "X.esp", "exact")]).unwrap();
        let (file, _) = load_user_file(&dir);
        let id = file.entries[0].id.clone();
        delete_entry(&dir, &id).unwrap();
        assert!(load_user_file(&dir).0.entries.is_empty());

        delete_entry(&dir, "seed-expressive-facegen-morphs").unwrap();
        assert!(load_user_file(&dir).0.disabled_seed_ids.contains(&"seed-expressive-facegen-morphs".to_string()));
        restore_seed(&dir, "seed-expressive-facegen-morphs").unwrap();
        assert!(load_user_file(&dir).0.disabled_seed_ids.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn seed_parses_and_loads_without_user_file() {
        let dir = temp_dir("seed-only");
        let lib = load_from_dir(&dir);
        assert!(lib.warning.is_none());
        assert!(lib.entries.iter().any(|e| e.entry.id == "seed-expressive-facegen-morphs"));
        assert!(lib.entries.iter().all(|e| e.source == "seed" && e.enabled));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn user_entries_shadow_seed_and_disabled_seed_is_off() {
        let dir = temp_dir("shadow");
        let file = UserLibraryFile {
            // Same (kind, match_type, pattern) as the seed EFM entry, different case.
            entries: vec![user_entry("user-1", "morph", "efm_", "prefix")],
            disabled_seed_ids: vec![],
        };
        save_user_file(&dir, &file).unwrap();
        let lib = load_from_dir(&dir);
        let seed = lib.entries.iter().find(|e| e.entry.id == "seed-expressive-facegen-morphs").unwrap();
        assert!(!seed.enabled, "shadowed seed entry is disabled");
        let user = lib.entries.iter().find(|e| e.entry.id == "user-1").unwrap();
        assert!(user.enabled && user.source == "user");

        // Explicitly disabled seed id.
        let file = UserLibraryFile {
            entries: vec![],
            disabled_seed_ids: vec!["seed-expressive-facegen-morphs".into()],
        };
        save_user_file(&dir, &file).unwrap();
        let lib = load_from_dir(&dir);
        let seed = lib.entries.iter().find(|e| e.entry.id == "seed-expressive-facegen-morphs").unwrap();
        assert!(!seed.enabled);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn matching_specificity_exact_then_longer_prefix_then_user() {
        let dir = temp_dir("specificity");
        let file = UserLibraryFile {
            // "Zzq" namespace deliberately avoids overlapping any real seed
            // pattern (e.g. seed-ks-hairdos-prefix's "KS Hairdos"), so this
            // test's specificity assertions stay isolated from seed content.
            entries: vec![
                user_entry("user-exact", "plugin", "Zzq Hairdo's.esp", "exact"),
                user_entry("user-short", "plugin", "Zz", "prefix"),
                user_entry("user-long", "plugin", "Zzq Hairdo", "prefix"),
            ],
            disabled_seed_ids: vec![],
        };
        save_user_file(&dir, &file).unwrap();
        let lib = load_from_dir(&dir);
        // Exact beats prefix, case-insensitively.
        let hit = lib.match_entry("plugin", "zzq hairdo's.esp", false).unwrap();
        assert_eq!(hit.entry.id, "user-exact");
        // Longer prefix beats shorter.
        let hit = lib.match_entry("plugin", "Zzq Hairdos Lite.esp", false).unwrap();
        assert_eq!(hit.entry.id, "user-long");
        // Kind is scoped.
        assert!(lib.match_entry("texture", "Zzq Hairdo's.esp", false).is_none());
        // user_only skips seed entries.
        assert!(lib.match_entry("morph", "EFM_Brow_Width", true).is_none());
        assert!(lib.match_entry("morph", "EFM_Brow_Width", false).is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn corrupt_user_file_is_preserved_aside_by_a_write() {
        let dir = temp_dir("corrupt-write");
        std::fs::write(user_file_path(&dir), b"{ not json").unwrap();

        upsert_entries(&dir, vec![user_entry("", "plugin", "New.esp", "exact")]).unwrap();

        // The new library.json contains only the new entry — no warning now
        // that it round-trips as valid JSON.
        let (file, warning) = load_user_file(&dir);
        assert!(warning.is_none());
        assert_eq!(file.entries.len(), 1);
        assert_eq!(file.entries[0].pattern, "New.esp");

        // The user's damaged-but-recoverable original survives on disk as a
        // sibling file instead of being silently clobbered.
        let sibling = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .find(|e| e.file_name().to_string_lossy().starts_with("library.json.invalid-"))
            .expect("corrupt library.json preserved aside");
        assert_eq!(std::fs::read(sibling.path()).unwrap(), b"{ not json");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn match_entry_normalizes_forward_slashes_in_texture_values() {
        let dir = temp_dir("slash-normalize");
        let file = UserLibraryFile {
            entries: vec![user_entry(
                "user-1",
                "texture",
                "Actors\\Character\\Overlays\\Foo\\",
                "prefix",
            )],
            disabled_seed_ids: vec![],
        };
        save_user_file(&dir, &file).unwrap();
        let lib = load_from_dir(&dir);
        // Forward-slash value still hits the backslash-pattern prefix.
        let hit = lib
            .match_entry("texture", "actors/character/overlays/foo/bar.dds", false)
            .unwrap();
        assert_eq!(hit.entry.id, "user-1");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn corrupt_user_file_degrades_to_seed_with_warning() {
        let dir = temp_dir("corrupt");
        std::fs::write(user_file_path(&dir), b"{ not json").unwrap();
        let lib = load_from_dir(&dir);
        assert!(lib.warning.is_some(), "warning names the problem");
        assert!(lib.entries.iter().any(|e| e.source == "seed"));
        // The corrupt file was not overwritten.
        assert_eq!(std::fs::read(user_file_path(&dir)).unwrap(), b"{ not json");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn seed_file_is_valid_and_unique() {
        let seed: Vec<LibraryEntry> = serde_json::from_str(SEED_JSON).unwrap();
        let mut ids = std::collections::HashSet::new();
        let mut keys = std::collections::HashSet::new();
        for e in &seed {
            assert!(e.id.starts_with("seed-"), "{}", e.id);
            assert!(ids.insert(e.id.clone()), "duplicate id {}", e.id);
            assert!(keys.insert(shadow_key(e)), "duplicate pattern {}", e.pattern);
            assert!(validate(e).is_ok(), "invalid seed entry {}: {:?}", e.id, validate(e));
        }
        assert!(seed.len() >= 30, "seed list looks incomplete: {}", seed.len());
    }

    #[test]
    fn nexus_url_parsing() {
        assert_eq!(
            nexus_mod_id_from_url("https://www.nexusmods.com/skyrimspecialedition/mods/6817"),
            Some(6817)
        );
        assert_eq!(
            nexus_mod_id_from_url("https://nexusmods.com/skyrimspecialedition/mods/6817?tab=files"),
            Some(6817)
        );
        assert_eq!(nexus_mod_id_from_url("https://vectorplexus.com/files/file/283-high-poly-head/"), None);
        assert_eq!(nexus_mod_id_from_url("not a url"), None);
    }

    #[test]
    fn match_entry_ignores_a_data_textures_lead_in() {
        // Presets spell the same mod's path both ways. One folder entry has
        // to claim both, otherwise an entry created from a group of unknown
        // references fails to match some of those very references.
        let dir = temp_dir("data-textures-prefix");
        let file = UserLibraryFile {
            entries: vec![user_entry(
                "user-1",
                "texture",
                "Actors\\Character\\PubicHairStyles\\",
                "prefix",
            )],
            disabled_seed_ids: vec![],
        };
        save_user_file(&dir, &file).unwrap();
        let lib = load_from_dir(&dir);
        for value in [
            "Actors\\Character\\PubicHairStyles\\a.dds",
            "Data\\Textures\\Actors\\Character\\PubicHairStyles\\a.dds",
            "textures/actors/character/pubichairstyles/a.dds",
        ] {
            assert_eq!(
                lib.match_entry("texture", value, false).map(|m| m.entry.id.as_str()),
                Some("user-1"),
                "{value} should match the folder entry"
            );
        }
        // A pattern that itself carries the lead-in still matches a value
        // without it — both sides are normalized.
        let file = UserLibraryFile {
            entries: vec![user_entry(
                "user-2",
                "texture",
                "Data\\Textures\\Actors\\Character\\PubicHairStyles\\",
                "prefix",
            )],
            disabled_seed_ids: vec![],
        };
        save_user_file(&dir, &file).unwrap();
        let lib = load_from_dir(&dir);
        assert_eq!(
            lib.match_entry("texture", "Actors\\Character\\PubicHairStyles\\a.dds", false)
                .map(|m| m.entry.id.as_str()),
            Some("user-2")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
