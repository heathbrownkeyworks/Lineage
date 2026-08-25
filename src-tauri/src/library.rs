//! The Asset Library: user-taught (and seed) mappings from preset references
//! to mod names and links. Two layers — a seed list compiled into the binary
//! and the user's library.json in the app config dir — merged at load with
//! user entries shadowing seed entries.

use crate::jslot;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const SEED_JSON: &str = include_str!("../resources/seed-library.json");

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
        let value_l = value.to_ascii_lowercase();
        self.entries
            .iter()
            .filter(|m| m.enabled)
            .filter(|m| !user_only || m.source == "user")
            .filter(|m| m.entry.kind.eq_ignore_ascii_case(kind))
            .filter(|m| {
                let p = m.entry.pattern.to_ascii_lowercase();
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
            entries: vec![
                user_entry("user-exact", "plugin", "KS Hairdo's.esp", "exact"),
                user_entry("user-short", "plugin", "KS", "prefix"),
                user_entry("user-long", "plugin", "KS Hairdo", "prefix"),
            ],
            disabled_seed_ids: vec![],
        };
        save_user_file(&dir, &file).unwrap();
        let lib = load_from_dir(&dir);
        // Exact beats prefix, case-insensitively.
        let hit = lib.match_entry("plugin", "ks hairdo's.esp", false).unwrap();
        assert_eq!(hit.entry.id, "user-exact");
        // Longer prefix beats shorter.
        let hit = lib.match_entry("plugin", "KS Hairdos Lite.esp", false).unwrap();
        assert_eq!(hit.entry.id, "user-long");
        // Kind is scoped.
        assert!(lib.match_entry("texture", "KS Hairdo's.esp", false).is_none());
        // user_only skips seed entries.
        assert!(lib.match_entry("morph", "EFM_Brow_Width", true).is_none());
        assert!(lib.match_entry("morph", "EFM_Brow_Width", false).is_some());
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
}
