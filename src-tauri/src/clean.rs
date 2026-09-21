//! Clean Preset — strip what a preset author's own setup leaked into a JSLOT.
//!
//! Loading a preset re-applies everything in it, so the author's BodySlide
//! sliders, skeleton scaling, weapon placement and body tattoos land on
//! whoever loads it. Removal works per node, not per section: `overrides`
//! holds body tattoos *and* face makeup, and `transforms` holds height *and*
//! head scale, so the node name decides what goes — never the section.
//!
//! Never touched: face overlays (`Face [OvlN]`). Those are the preset's own
//! look — makeup, freckles, warpaint — and there is deliberately no category
//! that removes them, so no batch run can strip a face.
//!
//! Writes go through the raw token tree and the file's own dialect, so
//! everything that isn't removed stays byte-for-byte as it was.

use crate::jslot::{self, JslotFormat};
use crate::rawjson::{self, Raw};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CleanCategory {
    /// `bodyMorphs`: BodySlide sliders and XPMSE morphs.
    BodyMorphs,
    /// `overrides` on Body, Hands and Feet overlay nodes — tattoos, paint.
    BodyOverlays,
    /// `transforms` on skeleton nodes: the root (height), spine, limbs…
    Skeleton,
    /// `transforms` on weapon, shield and quiver placement and CME cameras.
    WeaponCamera,
    /// `transforms` on head and neck nodes.
    HeadNeck,
}

/// Sections whose entries are classified one by one, by their `node`.
const NODE_SECTIONS: [&str; 2] = ["overrides", "transforms"];

/// Words that mark a transform node as equipment placement or camera.
const PLACEMENT_WORDS: [&str; 5] = ["weapon", "shield", "quiver", "bolt", "camera"];

/// `overrides` are named `<Region> [OvlN]`. Only the body regions go; Face
/// and anything unrecognized are never removed.
fn override_category(node: &str) -> Option<CleanCategory> {
    let region = node.split(" [").next().unwrap_or(node).trim();
    ["Body", "Hands", "Feet"]
        .iter()
        .any(|r| region.eq_ignore_ascii_case(r))
        .then_some(CleanCategory::BodyOverlays)
}

/// Every transform node is something: placement and camera first (the
/// `HDT`/`CME` weapon variants included), then head and neck, and everything
/// else is skeleton.
fn transform_category(node: &str) -> CleanCategory {
    let l = node.to_ascii_lowercase();
    if PLACEMENT_WORDS.iter().any(|w| l.contains(w)) {
        CleanCategory::WeaponCamera
    } else if l.contains("head") || l.contains("neck") {
        CleanCategory::HeadNeck
    } else {
        CleanCategory::Skeleton
    }
}

fn classify(section: &str, node: &str) -> Option<CleanCategory> {
    match section {
        "overrides" => override_category(node),
        "transforms" => Some(transform_category(node)),
        _ => None,
    }
}

fn node_of(entry: &Raw) -> Option<String> {
    entry.get("node").and_then(Raw::as_str)
}

/// What one category holds in a preset.
#[derive(Debug, Clone, Serialize)]
pub struct Finding {
    pub category: CleanCategory,
    /// Entries it would remove.
    pub count: usize,
    /// Morph names for body morphs, node names otherwise, in file order.
    pub items: Vec<String>,
    /// The removed entries exactly as this file writes them. Single-file
    /// inspection only; batch scans don't need the text.
    pub preview: Option<String>,
}

/// Every category present in `raw`. Pass the file's format to also render
/// each category's verbatim preview.
pub fn findings(raw: &Raw, format: Option<JslotFormat>) -> Vec<Finding> {
    let mut out = Vec::new();
    if let Some(key) = jslot::body_morph_key(raw) {
        let count = match raw.get(key) {
            Some(Raw::Array(entries)) => entries.len(),
            _ => 0,
        };
        if count > 0 {
            out.push(Finding {
                category: CleanCategory::BodyMorphs,
                count,
                items: jslot::body_morph_names(raw),
                preview: format.and_then(|f| jslot::section_preview(raw, key, f)),
            });
        }
    }
    for section in NODE_SECTIONS {
        let Some(Raw::Array(entries)) = raw.get(section) else { continue };
        let mut by_category: BTreeMap<CleanCategory, (Vec<String>, Vec<Raw>)> = BTreeMap::new();
        for entry in entries {
            let Some(node) = node_of(entry) else { continue };
            let Some(category) = classify(section, &node) else { continue };
            let slot = by_category.entry(category).or_default();
            slot.0.push(node);
            slot.1.push(entry.clone());
        }
        for (category, (items, removed)) in by_category {
            // Preview: the removed entries rendered as the section they come
            // from, so it reads exactly like the file does.
            let preview = format.and_then(|f| {
                let only = Raw::Object(vec![(format!("\"{section}\""), Raw::Array(removed))]);
                jslot::section_preview(&only, section, f)
            });
            out.push(Finding {
                category,
                count: items.len(),
                items,
                preview,
            });
        }
    }
    out.sort_by_key(|f| f.category);
    out
}

/// Remove the chosen categories from `raw` in place; returns what went. A
/// section emptied by the removal is dropped, as RaceMenu writes a preset
/// that has none — but one that was already empty is left alone.
pub fn clean(raw: &mut Raw, chosen: &[CleanCategory]) -> Vec<Finding> {
    let removed: Vec<Finding> = findings(raw, None)
        .into_iter()
        .filter(|f| chosen.contains(&f.category))
        .collect();
    if removed.is_empty() {
        return removed;
    }
    if chosen.contains(&CleanCategory::BodyMorphs) {
        if let Some(key) = jslot::body_morph_key(raw) {
            raw.remove(key);
        }
    }
    for section in NODE_SECTIONS {
        let (changed, now_empty) = match raw.get_mut(section) {
            Some(Raw::Array(entries)) => {
                let before = entries.len();
                entries.retain(|entry| {
                    match node_of(entry).and_then(|node| classify(section, &node)) {
                        Some(category) => !chosen.contains(&category),
                        None => true,
                    }
                });
                (entries.len() < before, entries.is_empty())
            }
            _ => continue,
        };
        if changed && now_empty {
            raw.remove(section);
        }
    }
    removed
}

#[derive(Debug, Clone, Serialize)]
pub struct CleanDetail {
    pub path: String,
    pub removed: Vec<Finding>,
}

/// Clean a preset on disk and write it back atomically in its own dialect.
/// `Ok(None)` when the chosen categories hold nothing — the file is left
/// untouched. The caller snapshots first.
pub fn clean_in_place(path: &Path, chosen: &[CleanCategory]) -> Result<Option<CleanDetail>, String> {
    let bytes =
        std::fs::read(path).map_err(|e| format!("Couldn't read {}: {e}", path.display()))?;
    let (bom, text) = jslot::read_text(&bytes);
    // serde_json is the validator of record — its errors read better.
    serde_json::from_str::<Value>(&text).map_err(|e| format!("Not valid preset JSON ({e})"))?;
    let mut raw = rawjson::parse(&text).map_err(|e| format!("Not valid preset JSON ({e})"))?;
    let format = jslot::detect_format(&text, bom);
    let removed = clean(&mut raw, chosen);
    if removed.is_empty() {
        return Ok(None);
    }
    jslot::write_atomic(path, jslot::to_formatted_string(&raw, format).as_bytes())?;
    Ok(Some(CleanDetail {
        path: path.display().to_string(),
        removed,
    }))
}

/// What one preset looks like to Clean Preset.
#[derive(Debug, Serialize)]
pub struct PresetInspection {
    pub path: String,
    pub file_name: String,
    pub findings: Vec<Finding>,
    /// Set when the file couldn't be parsed; everything else is then empty.
    pub parse_error: Option<String>,
    /// True when parse → serialize reproduces the file exactly, so a clean
    /// changes only what it removes.
    pub roundtrip_faithful: bool,
}

/// Inspect a preset. `with_previews` renders each finding's verbatim text —
/// the single-file screen wants it; a batch scan of thousands doesn't.
pub fn inspect(path: &Path, with_previews: bool) -> PresetInspection {
    let base = PresetInspection {
        path: path.display().to_string(),
        file_name: path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        findings: Vec::new(),
        parse_error: None,
        roundtrip_faithful: false,
    };
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) => {
            return PresetInspection {
                parse_error: Some(format!("Couldn't read the file: {e}")),
                ..base
            }
        }
    };
    let (bom, text) = jslot::read_text(&bytes);
    if let Err(e) = serde_json::from_str::<Value>(&text) {
        return PresetInspection {
            parse_error: Some(format!("Not valid preset JSON ({e})")),
            ..base
        };
    }
    let raw = match rawjson::parse(&text) {
        Ok(raw) => raw,
        Err(e) => {
            return PresetInspection {
                parse_error: Some(format!("Not valid preset JSON ({e})")),
                ..base
            }
        }
    };
    let format = jslot::detect_format(&text, bom);
    PresetInspection {
        findings: findings(&raw, with_previews.then_some(format)),
        roundtrip_faithful: jslot::roundtrip_faithful(&bytes, &raw),
        ..base
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use CleanCategory::*;

    /// The default selection the UI ticks: everything but head & neck.
    const DEFAULTS: [CleanCategory; 4] = [BodyMorphs, BodyOverlays, Skeleton, WeaponCamera];

    fn transform(node: &str) -> String {
        format!(
            r#"{{"firstPerson":false,"keys":[{{"name":"XPMSE.esp","values":[{{"data":0.85,"index":0,"key":30,"type":4}}]}}],"node":"{node}"}}"#
        )
    }

    fn overlay(node: &str, texture: &str) -> String {
        format!(
            r#"{{"node":"{node}","values":[{{"data":"{texture}","index":0,"key":9,"type":2}}]}}"#
        )
    }

    /// A preset in compact source form; `canonical` turns it into exactly
    /// what RaceMenu's StyledWriter would have written.
    fn preset(body_morphs: bool, overrides: &[String], transforms: &[String]) -> String {
        let mut members = vec![r#""actor":{"weight":100}"#.to_string()];
        if body_morphs {
            members.push(
                r#""bodyMorphs":[{"keys":[{"key":"XPMSE.esp","value":1}],"name":"Breasts"}]"#.into(),
            );
        }
        if !overrides.is_empty() {
            members.push(format!(r#""overrides":[{}]"#, overrides.join(",")));
        }
        if !transforms.is_empty() {
            members.push(format!(r#""transforms":[{}]"#, transforms.join(",")));
        }
        members.push(r#""version":{"skseVersion":33554736}"#.into());
        format!("{{{}}}", members.join(","))
    }

    fn canonical(compact: &str) -> String {
        jslot::to_formatted_string(&rawjson::parse(compact).unwrap(), JslotFormat::default())
    }

    fn temp_file(name: &str, contents: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("lineage-clean-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("p.jslot");
        std::fs::write(&path, contents).unwrap();
        path
    }

    #[test]
    fn nodes_are_classified_by_name_not_section() {
        assert_eq!(override_category("Body [Ovl0]"), Some(BodyOverlays));
        assert_eq!(override_category("Hands [Ovl1]"), Some(BodyOverlays));
        assert_eq!(override_category("Feet [Ovl2]"), Some(BodyOverlays));
        assert_eq!(override_category("Face [Ovl0]"), None, "face overlays are the look");
        assert_eq!(override_category("Something Else"), None, "unknown is never removed");

        for (node, want) in [
            ("NPC", Skeleton),
            ("NPC L Butt", Skeleton),
            ("NPC Spine2 [Spn2]", Skeleton),
            ("NPC L MagicNode [LMag]", Skeleton),
            ("CME L Finger10 [LF10]", Skeleton),
            ("ShieldBack", WeaponCamera),
            ("WEAPON", WeaponCamera),
            ("HDT WeaponSword", WeaponCamera),
            ("CME WeaponBackAxeMaceFSM", WeaponCamera),
            ("QUIVER", WeaponCamera),
            ("BOLT_QUIVER", WeaponCamera),
            ("CME Camera3rd [Cam3]", WeaponCamera),
            ("NPC Head [Head]", HeadNeck),
            ("NPC Head MagicNode [Hmag]", HeadNeck),
            ("CME Neck [Neck]", HeadNeck),
        ] {
            assert_eq!(transform_category(node), want, "{node}");
        }
    }

    #[test]
    fn cleaning_removes_exactly_the_chosen_nodes_and_nothing_else() {
        let tattoo = overlay("Body [Ovl0]", r"Actors\\Character\\Overlays\\Tattoo.dds");
        let makeup = overlay("Face [Ovl0]", r"Actors\\Character\\Overlays\\Makeup.dds");
        let before = preset(
            true,
            &[tattoo, makeup.clone()],
            &[transform("NPC"), transform("ShieldBack"), transform("NPC Head [Head]")],
        );
        let path = temp_file("defaults", &canonical(&before));

        let detail = clean_in_place(&path, &DEFAULTS).unwrap().expect("something to clean");
        let removed: Vec<(CleanCategory, Vec<String>)> =
            detail.removed.iter().map(|f| (f.category, f.items.clone())).collect();
        assert_eq!(
            removed,
            vec![
                (BodyMorphs, vec!["Breasts".to_string()]),
                (BodyOverlays, vec!["Body [Ovl0]".to_string()]),
                (Skeleton, vec!["NPC".to_string()]),
                (WeaponCamera, vec!["ShieldBack".to_string()]),
            ]
        );

        // Byte-level: the file is exactly the same preset, re-serialized,
        // minus the removed entries — the face overlay and head scale stay.
        let expected = canonical(&preset(false, &[makeup], &[transform("NPC Head [Head]")]));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), expected);

        // Idempotent, and no stray temp files beside the preset.
        assert!(clean_in_place(&path, &DEFAULTS).unwrap().is_none());
        let dir = path.parent().unwrap();
        assert_eq!(std::fs::read_dir(dir).unwrap().count(), 1, "stray files in {}", dir.display());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_section_emptied_by_cleaning_is_dropped_but_an_already_empty_one_stays() {
        // Only body overlays: removing them empties `overrides` → dropped.
        let before = preset(false, &[overlay("Body [Ovl0]", "a.dds")], &[transform("NPC Head [Head]")]);
        let path = temp_file("emptied", &canonical(&before));
        clean_in_place(&path, &[BodyOverlays]).unwrap().unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            canonical(&preset(false, &[], &[transform("NPC Head [Head]")]))
        );
        let _ = std::fs::remove_dir_all(path.parent().unwrap());

        // An `overrides: []` the author wrote is theirs, and nothing is
        // removed from it — the file must not change at all.
        let text = canonical(r#"{"actor":{"weight":100},"overrides":[],"transforms":[]}"#);
        let path = temp_file("already-empty", &text);
        assert!(clean_in_place(&path, &DEFAULTS).unwrap().is_none());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn head_and_neck_only_go_when_chosen() {
        let before = preset(false, &[], &[transform("NPC Head [Head]"), transform("CME Neck [Neck]")]);
        let path = temp_file("head", &canonical(&before));
        assert!(clean_in_place(&path, &DEFAULTS).unwrap().is_none(), "not in the defaults");
        let detail = clean_in_place(&path, &[HeadNeck]).unwrap().unwrap();
        assert_eq!(detail.removed[0].count, 2);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn previews_show_only_the_removed_entries_in_the_files_own_dialect() {
        let text = canonical(&preset(
            false,
            &[overlay("Body [Ovl0]", "tattoo.dds"), overlay("Face [Ovl0]", "makeup.dds")],
            &[],
        ));
        let path = temp_file("preview", &text);
        let inspection = inspect(&path, true);
        let body = inspection
            .findings
            .iter()
            .find(|f| f.category == BodyOverlays)
            .unwrap();
        let preview = body.preview.as_deref().unwrap();
        assert!(preview.starts_with("\"overrides\" : ["), "{preview}");
        assert!(preview.contains("tattoo.dds"));
        assert!(!preview.contains("makeup.dds"), "the kept face overlay isn't in the preview");
        assert!(inspection.findings.iter().all(|f| f.category != HeadNeck));
        assert!(inspect(&path, false).findings[0].preview.is_none(), "batch path stays lean");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// Across the real collection, cleaning with the defaults must remove
    /// only what it reports and leave every other value untouched — checked
    /// on the parsed data, independently of the writer that produced it.
    #[test]
    fn cleaning_the_real_collection_changes_only_what_it_reports() {
        let corpus = std::path::Path::new(r"D:\Nordic Souls\mods");
        if !corpus.is_dir() {
            return;
        }
        let mut cleaned = 0usize;
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
            let bytes = std::fs::read(entry.path()).unwrap();
            let (bom, text) = jslot::read_text(&bytes);
            let Ok(original) = serde_json::from_str::<Value>(&text) else { continue };
            let Ok(mut raw) = rawjson::parse(&text) else { continue };
            let removed = clean(&mut raw, &DEFAULTS);
            if removed.is_empty() {
                continue;
            }
            cleaned += 1;
            let out = jslot::to_formatted_string(&raw, jslot::detect_format(&text, bom));
            let after: Value = serde_json::from_str(out.trim_start_matches('\u{feff}'))
                .unwrap_or_else(|e| panic!("{} no longer parses: {e}", entry.path().display()));

            let (Value::Object(was), Value::Object(now)) = (&original, &after) else {
                panic!("not an object")
            };
            for (key, value) in was {
                match key.as_str() {
                    "bodyMorphs" => assert!(!now.contains_key(key)),
                    "overrides" | "transforms" => {
                        let Some(list) = value.as_array() else {
                            // Not a list: clean never touches it.
                            assert_eq!(now.get(key), Some(value));
                            continue;
                        };
                        // What remains is the original list, in order, with
                        // exactly the removable entries taken out.
                        let kept: Vec<&Value> = list
                            .iter()
                            .filter(|e| {
                                let node = e.get("node").and_then(Value::as_str);
                                !node.and_then(|n| classify(key, n)).is_some_and(|c| DEFAULTS.contains(&c))
                            })
                            .collect();
                        let remaining: Vec<&Value> = now
                            .get(key)
                            .and_then(Value::as_array)
                            .map(|a| a.iter().collect())
                            .unwrap_or_default();
                        assert_eq!(remaining, kept, "{} {key}", entry.path().display());
                    }
                    _ => assert_eq!(now.get(key), Some(value), "{} {key}", entry.path().display()),
                }
            }
            // And a second pass finds nothing left to take.
            assert!(clean(&mut raw, &DEFAULTS).is_empty());
        }
        assert!(cleaned > 100, "expected a real corpus, cleaned {cleaned}");
    }
}
