//! Compare: what differs between two presets, and which presets are the
//! same face.
//!
//! A preset is read into facts — `(section, key) → value` — named the way
//! RaceMenu and the CK name them: head parts by type, the 19 CK face morphs,
//! custom sliders, sculpt per `.tri` host, tints per mask, overlays per node.
//! Everything Clean Preset would strip (body morphs, transforms, body
//! overlays, weight, height) sits in its own Body section, so two presets can
//! be "the same face" while differing only there. One engine answers three
//! questions: exact copies (identical bytes), same face (every fact outside
//! Body equal), and near-twins (faces a handful of facts apart).

use crate::jslot;
use crate::scan;
use crate::settings;
use crate::snapshot::{self, FailedFile};
use md5::{Digest, Md5};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Section {
    HeadParts,
    VanillaSliders,
    CustomSliders,
    Sculpt,
    Tints,
    FaceTextures,
    FaceOverlays,
    Appearance,
    Body,
}

impl Section {
    fn label(self) -> &'static str {
        match self {
            Section::HeadParts => "Head parts",
            Section::VanillaSliders => "Vanilla sliders",
            Section::CustomSliders => "Custom sliders",
            Section::Sculpt => "Sculpt",
            Section::Tints => "Tints",
            Section::FaceTextures => "Face textures",
            Section::FaceOverlays => "Face overlays",
            Section::Appearance => "Appearance",
            Section::Body => "Body & placement",
        }
    }
}

/// The CK's face morphs (NPC_ NAM9), in the order RaceMenu stores them.
const VANILLA_MORPHS: [&str; 19] = [
    "Nose Long/Short",
    "Nose Up/Down",
    "Jaw Up/Down",
    "Jaw Narrow/Wide",
    "Jaw Forward/Back",
    "Cheeks Up/Down",
    "Cheeks Forward/Back",
    "Eyes Up/Down",
    "Eyes In/Out",
    "Brows Up/Down",
    "Brows In/Out",
    "Brows Forward/Back",
    "Lips Up/Down",
    "Lips In/Out",
    "Chin Narrow/Wide",
    "Chin Up/Down",
    "Chin Underbite/Overbite",
    "Eyes Forward/Back",
    "Unused",
];
/// The CK's face presets (NPC_ NAMA).
const VANILLA_PRESETS: [&str; 4] = ["Nose type", "Unknown preset", "Eyes type", "Mouth type"];
const HEAD_PART_TYPES: [&str; 7] = ["Misc", "Face", "Eyes", "Hair", "Facial hair", "Scar", "Brows"];
const TEXTURE_SLOTS: [&str; 8] = [
    "Diffuse",
    "Normal",
    "Subsurface",
    "Detail",
    "Height",
    "Environment",
    "Multilayer",
    "Specular",
];
const BODY_NODES: [&str; 3] = ["body", "hands", "feet"];
/// A face this many facts apart from another is a near-twin.
const NEAR_TWIN_LIMIT: usize = 5;
const NEAR_TWIN_CAP: usize = 200;

#[derive(Debug, Clone)]
struct Fact {
    display: String,
    /// What equality means; the display, unless that hides detail.
    identity: String,
    /// Sculpt only: vertex index → offset, to count differing vertices.
    vertices: Option<HashMap<i64, [i64; 3]>>,
}

/// `(section, order, key)`: order keeps positional keys (the CK sliders,
/// texture slots) in their natural order; named keys sort by name.
type Key = (Section, u32, String);
type Facts = BTreeMap<Key, Fact>;

fn put(out: &mut Facts, section: Section, order: u32, key: impl Into<String>, display: String) {
    out.insert(
        (section, order, key.into()),
        Fact {
            identity: display.clone(),
            display,
            vertices: None,
        },
    );
}

/// Three decimals, trailing zeros dropped: 0.429999977 → "0.43".
fn fmt_num(x: f64) -> String {
    let s = format!("{x:.3}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".to_string() } else { s.to_string() }
}

fn fmt_rgb(c: u64) -> String {
    format!("#{:06X}", c & 0xFF_FFFF)
}

/// Tint colours are ARGB; a layer at zero alpha isn't drawn at all.
fn fmt_argb(c: u64) -> String {
    let alpha = (c >> 24) & 0xFF;
    if alpha == 0 {
        return "off".to_string();
    }
    format!("{} · {}%", fmt_rgb(c), (alpha * 100 + 127) / 255)
}

fn file_name(path: &str) -> &str {
    path.rsplit(['\\', '/']).next().unwrap_or(path)
}

fn stem(path: &str) -> &str {
    let name = file_name(path);
    name.rsplit_once('.').map(|(s, _)| s).unwrap_or(name)
}

fn is_body_node(node: &str) -> bool {
    let lower = node.to_ascii_lowercase();
    BODY_NODES.iter().any(|b| lower.starts_with(b))
}

/// Canonical text of a JSON value: object keys sorted, so equal data is
/// equal text however it was written.
fn canonical(v: &Value) -> String {
    match v {
        Value::Object(map) => {
            let sorted: BTreeMap<&String, String> = map.iter().map(|(k, v)| (k, canonical(v))).collect();
            let body: Vec<String> = sorted.iter().map(|(k, v)| format!("{k:?}:{v}")).collect();
            format!("{{{}}}", body.join(","))
        }
        Value::Array(items) => format!("[{}]", items.iter().map(canonical).collect::<Vec<_>>().join(",")),
        Value::Number(n) => n.as_f64().map(fmt_num).unwrap_or_else(|| n.to_string()),
        other => other.to_string(),
    }
}

/// An overlay's values, readable: texture file, colour, opacity.
fn overlay_display(values: &[Value]) -> String {
    let mut texture = None;
    let mut color = None;
    let mut alpha = None;
    for v in values {
        match v.get("key").and_then(Value::as_u64) {
            Some(9) => texture = v.get("data").and_then(Value::as_str).map(|s| file_name(s).to_string()),
            Some(7) => color = v.get("data").and_then(Value::as_u64).map(fmt_rgb),
            Some(8) => alpha = v.get("data").and_then(Value::as_f64).map(|a| format!("{}%", (a * 100.0).round())),
            _ => {}
        }
    }
    let parts: Vec<String> = [texture, color, alpha].into_iter().flatten().collect();
    if parts.is_empty() {
        format!("{} values", values.len())
    } else {
        parts.join(" · ")
    }
}

fn sorted_values(values: &[Value]) -> Vec<Value> {
    let mut v = values.to_vec();
    v.sort_by_key(|x| {
        (
            x.get("key").and_then(Value::as_u64).unwrap_or(0),
            x.get("index").and_then(Value::as_u64).unwrap_or(0),
        )
    });
    v
}

fn facts(preset: &Value, with_vertices: bool) -> Facts {
    let mut out = Facts::new();

    // Load-order index → plugin, for head parts saved with only a formId:
    // the raw id means nothing without it (0D043C55 is a different hair in
    // every load order).
    let plugins: HashMap<u64, &str> = preset
        .get("mods")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|m| Some((m.get("index")?.as_u64()?, m.get("name")?.as_str()?)))
        .collect();

    // Head parts, by type. Several of one type (scars, misc) are one fact.
    let mut by_type: BTreeMap<(u32, String), Vec<String>> = BTreeMap::new();
    for p in preset.get("headParts").and_then(Value::as_array).into_iter().flatten() {
        let t = p.get("type").and_then(Value::as_u64).unwrap_or(u64::MAX);
        let name = HEAD_PART_TYPES
            .get(t as usize)
            .map(|n| n.to_string())
            .unwrap_or_else(|| format!("Other part (type {t})"));
        let ident = p
            .get("formIdentifier")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| {
                let id = p.get("formId").and_then(Value::as_u64)?;
                Some(match plugins.get(&(id >> 24)) {
                    Some(plugin) => format!("{plugin}|{:06X}", id & 0xFF_FFFF),
                    None => format!("{id:08X}"),
                })
            })
            .unwrap_or_default();
        by_type.entry((t.min(u32::MAX as u64) as u32, name)).or_default().push(ident);
    }
    for ((order, name), mut ids) in by_type {
        ids.sort_by_key(|i| i.to_lowercase());
        put(&mut out, Section::HeadParts, order, name, ids.join(", "));
    }

    let morphs = preset.get("morphs");
    let default = morphs.and_then(|m| m.get("default"));
    for (i, v) in default
        .and_then(|d| d.get("morphs"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
    {
        let name = VANILLA_MORPHS.get(i).map(|n| n.to_string()).unwrap_or_else(|| format!("Slider {}", i + 1));
        put(&mut out, Section::VanillaSliders, i as u32, name, v.as_f64().map(fmt_num).unwrap_or_default());
    }
    for (i, v) in default
        .and_then(|d| d.get("presets"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
    {
        let name = VANILLA_PRESETS.get(i).map(|n| n.to_string()).unwrap_or_else(|| format!("Preset {}", i + 1));
        let value = match v.as_i64() {
            Some(-1) | Some(4_294_967_295) | None => "none".to_string(),
            Some(n) => n.to_string(),
        };
        put(&mut out, Section::VanillaSliders, 100 + i as u32, name, value);
    }

    // Custom sliders: one at 0 does nothing, so it counts as absent.
    for c in morphs.and_then(|m| m.get("custom")).and_then(Value::as_array).into_iter().flatten() {
        let (Some(name), Some(value)) = (c.get("name").and_then(Value::as_str), c.get("value").and_then(Value::as_f64))
        else {
            continue;
        };
        let shown = fmt_num(value);
        if shown != "0" {
            put(&mut out, Section::CustomSliders, 0, name, shown);
        }
    }

    let sculpts = morphs.and_then(|m| m.get("sculpt")).and_then(Value::as_array);
    for s in sculpts.into_iter().flatten() {
        let host = s.get("host").and_then(Value::as_str).unwrap_or_default();
        let mut vertices: Vec<(i64, [i64; 3])> = s
            .get("data")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|d| {
                let d = d.as_array()?;
                let n = |i: usize| d.get(i).and_then(Value::as_i64);
                Some((n(0)?, [n(1)?, n(2)?, n(3)?]))
            })
            .collect();
        vertices.sort_unstable();
        // The full host path is part of what's equal: two .tri files can
        // share a name in different folders.
        let identity = format!(
            "{}|{}",
            host.to_lowercase().replace('/', "\\"),
            vertices
                .iter()
                .map(|(i, o)| format!("{i}:{},{},{}", o[0], o[1], o[2]))
                .collect::<Vec<_>>()
                .join(";")
        );
        let mut key = file_name(host).to_string();
        let mut n = 1;
        while out.contains_key(&(Section::Sculpt, 0, key.clone())) {
            n += 1;
            key = format!("{} ({n})", file_name(host));
        }
        out.insert(
            (Section::Sculpt, 0, key),
            Fact {
                display: format!("{} vertices moved", vertices.len()),
                identity,
                vertices: with_vertices.then(|| vertices.into_iter().collect()),
            },
        );
    }
    if sculpts.is_some_and(|s| !s.is_empty()) {
        if let Some(d) = morphs.and_then(|m| m.get("sculptDivisor")) {
            put(&mut out, Section::Sculpt, 1, "Divisor", canonical(d));
        }
    }

    for t in preset.get("tintInfo").and_then(Value::as_array).into_iter().flatten() {
        let texture = t.get("texture").and_then(Value::as_str).unwrap_or_default();
        let color = t.get("color").and_then(Value::as_u64).unwrap_or(0);
        let mut key = stem(texture).to_string();
        if out.contains_key(&(Section::Tints, 0, key.clone())) {
            key = format!("{key} ({})", t.get("index").and_then(Value::as_u64).unwrap_or(0));
        }
        let display = fmt_argb(color);
        out.insert(
            (Section::Tints, 0, key),
            Fact {
                // Masks with one name in different folders aren't the same mask.
                identity: format!("{}|{display}", texture.to_lowercase().replace('/', "\\")),
                display,
                vertices: None,
            },
        );
    }

    for t in preset.get("faceTextures").and_then(Value::as_array).into_iter().flatten() {
        let index = t.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
        let slot = TEXTURE_SLOTS.get(index).map(|s| s.to_string()).unwrap_or_else(|| format!("Slot {index}"));
        let texture = t.get("texture").and_then(Value::as_str).unwrap_or_default().to_string();
        put(&mut out, Section::FaceTextures, index as u32, slot, texture);
    }

    for (section_name, prefix) in [("overrides", ""), ("skinOverrides", "Skin override: ")] {
        for o in preset.get(section_name).and_then(Value::as_array).into_iter().flatten() {
            let node = o.get("node").and_then(Value::as_str).unwrap_or_default();
            let values = sorted_values(o.get("values").and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[]));
            let body = !prefix.is_empty() || is_body_node(node);
            let (section, key) = if body {
                (Section::Body, format!("{}{node}", if prefix.is_empty() { "Overlay: " } else { prefix }))
            } else {
                (Section::FaceOverlays, node.to_string())
            };
            out.insert(
                (section, 0, key),
                Fact {
                    display: overlay_display(&values),
                    identity: canonical(&Value::Array(values)),
                    vertices: None,
                },
            );
        }
    }

    if let Some(actor) = preset.get("actor") {
        if let Some(c) = actor.get("hairColor").and_then(Value::as_u64) {
            put(&mut out, Section::Appearance, 0, "Hair colour", fmt_rgb(c));
        }
        if let Some(t) = actor.get("headTexture").and_then(Value::as_str) {
            put(&mut out, Section::Appearance, 1, "Skin (head texture set)", t.to_string());
        }
        for (field, label) in [("weight", "Weight"), ("height", "Height")] {
            if let Some(v) = actor.get(field).and_then(Value::as_f64) {
                put(&mut out, Section::Body, 0, label, fmt_num(v));
            }
        }
    }

    for m in preset.get("bodyMorphs").and_then(Value::as_array).into_iter().flatten() {
        let Some(name) = m.get("name").and_then(Value::as_str) else { continue };
        let keys = m.get("keys").cloned().unwrap_or(Value::Null);
        let total: f64 = keys
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|k| k.get("value").and_then(Value::as_f64))
            .sum();
        out.insert(
            (Section::Body, 0, format!("Body morph: {name}")),
            Fact {
                display: fmt_num(total),
                identity: canonical(&keys),
                vertices: None,
            },
        );
    }

    for t in preset.get("transforms").and_then(Value::as_array).into_iter().flatten() {
        let node = t.get("node").and_then(Value::as_str).unwrap_or_default();
        let first = t.get("firstPerson").and_then(Value::as_bool).unwrap_or(false);
        let keys = t.get("keys").cloned().unwrap_or(Value::Null);
        let data: Vec<String> = keys
            .as_array()
            .into_iter()
            .flatten()
            .flat_map(|k| k.get("values").and_then(Value::as_array).cloned().unwrap_or_default())
            .filter_map(|v| v.get("data").and_then(Value::as_f64).map(fmt_num))
            .collect();
        out.insert(
            (Section::Body, 0, format!("Transform: {node}{}", if first { " (1st person)" } else { "" })),
            Fact {
                display: if data.is_empty() { "set".to_string() } else { data.join(", ") },
                identity: canonical(&keys),
                vertices: None,
            },
        );
    }
    out
}

fn hash_of(x: impl Hash) -> u64 {
    let mut h = DefaultHasher::new();
    x.hash(&mut h);
    h.finish()
}

/// Everything outside Body, as one number: equal means the same face.
fn face_identity(facts: &Facts) -> u64 {
    hash_of(
        facts
            .iter()
            .filter(|(k, _)| k.0 != Section::Body)
            .map(|(k, f)| (k, &f.identity))
            .collect::<Vec<_>>(),
    )
}

fn read_preset(path: &Path) -> Result<(Vec<u8>, Value), String> {
    let bytes = std::fs::read(path).map_err(|e| format!("Couldn't read {} ({e})", path.display()))?;
    let (_, text) = jslot::read_text(&bytes);
    let value = serde_json::from_str(&text).map_err(|e| format!("Not valid preset JSON ({e})"))?;
    Ok((bytes, value))
}

#[derive(Debug, Serialize)]
pub struct ItemDiff {
    pub key: String,
    pub left: Option<String>,
    pub right: Option<String>,
    pub same: bool,
    /// Extra detail for a difference ("12 vertices differ").
    pub note: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct SectionDiff {
    pub section: Section,
    pub label: String,
    pub differences: usize,
    /// Every item, same or not, in the section's natural order.
    pub items: Vec<ItemDiff>,
}

#[derive(Debug, Serialize)]
pub struct Comparison {
    pub left: String,
    pub right: String,
    pub same_bytes: bool,
    pub same_face: bool,
    /// Differences outside Body & placement.
    pub face_differences: usize,
    pub body_differences: usize,
    pub sections: Vec<SectionDiff>,
}

fn diff_facts(a: &Facts, b: &Facts) -> Vec<SectionDiff> {
    let keys: std::collections::BTreeSet<&Key> = a.keys().chain(b.keys()).collect();
    let mut sections: Vec<SectionDiff> = Vec::new();
    for key in keys {
        let (fa, fb) = (a.get(key), b.get(key));
        let same = match (fa, fb) {
            (Some(x), Some(y)) => x.identity == y.identity,
            _ => false,
        };
        let note = match (fa.and_then(|f| f.vertices.as_ref()), fb.and_then(|f| f.vertices.as_ref())) {
            (Some(va), Some(vb)) if !same => {
                let differ = va
                    .iter()
                    .filter(|(i, o)| vb.get(i) != Some(o))
                    .count()
                    + vb.keys().filter(|i| !va.contains_key(i)).count();
                Some(if differ == 1 { "1 vertex differs".to_string() } else { format!("{differ} vertices differ") })
            }
            _ => None,
        };
        let item = ItemDiff {
            key: key.2.clone(),
            left: fa.map(|f| f.display.clone()),
            right: fb.map(|f| f.display.clone()),
            same,
            note,
        };
        match sections.last_mut() {
            Some(s) if s.section == key.0 => {
                s.differences += usize::from(!same);
                s.items.push(item);
            }
            _ => sections.push(SectionDiff {
                section: key.0,
                label: key.0.label().to_string(),
                differences: usize::from(!same),
                items: vec![item],
            }),
        }
    }
    sections
}

pub fn compare_paths(left: &Path, right: &Path) -> Result<Comparison, String> {
    let (bytes_a, a) = read_preset(left)?;
    let (bytes_b, b) = read_preset(right)?;
    let (fa, fb) = (facts(&a, true), facts(&b, true));
    let sections = diff_facts(&fa, &fb);
    let count = |body: bool| {
        sections
            .iter()
            .filter(|s| (s.section == Section::Body) == body)
            .map(|s| s.differences)
            .sum()
    };
    Ok(Comparison {
        left: left.display().to_string(),
        right: right.display().to_string(),
        same_bytes: bytes_a == bytes_b,
        same_face: face_identity(&fa) == face_identity(&fb),
        face_differences: count(false),
        body_differences: count(true),
        sections,
    })
}

#[derive(Debug, Clone, Serialize)]
pub struct PresetRef {
    pub path: String,
    pub file_name: String,
    /// Unix seconds, 0 when unknown.
    pub modified: i64,
}

#[derive(Debug, Serialize)]
pub struct DuplicateGroup {
    pub presets: Vec<PresetRef>,
}

#[derive(Debug, Serialize)]
pub struct NearTwin {
    pub left: PresetRef,
    pub right: PresetRef,
    pub differences: usize,
    /// What differs, e.g. "Tints: FemaleHeadWarPaint_02".
    pub what: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct DuplicatesReport {
    pub total: usize,
    /// Byte-identical files.
    pub exact: Vec<DuplicateGroup>,
    /// Same face, not all byte-identical: they differ only in Body & placement.
    pub same_face: Vec<DuplicateGroup>,
    /// Faces 1–5 facts apart, fewest first.
    pub near_twins: Vec<NearTwin>,
    pub unreadable: Vec<FailedFile>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CompareProgress {
    pub current: usize,
    pub total: usize,
    pub detail: String,
}

fn preset_ref(path: &str) -> PresetRef {
    let modified = std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    PresetRef {
        path: path.to_string(),
        file_name: file_name(path).to_string(),
        modified,
    }
}

/// Face facts reduced to comparable numbers: (interned key, value hash),
/// sorted by key, so two faces compare with one merge.
type Compact = Vec<(u32, u64)>;

fn differences_within(a: &Compact, b: &Compact, limit: usize) -> Option<usize> {
    let (mut i, mut j, mut n) = (0, 0, 0);
    while i < a.len() || j < b.len() {
        match (a.get(i), b.get(j)) {
            (Some(x), Some(y)) if x.0 == y.0 => {
                n += usize::from(x.1 != y.1);
                i += 1;
                j += 1;
            }
            (Some(x), Some(y)) if x.0 < y.0 => {
                n += 1;
                i += 1;
            }
            (Some(_), None) => {
                n += 1;
                i += 1;
            }
            _ => {
                n += 1;
                j += 1;
            }
        }
        if n > limit {
            return None;
        }
    }
    Some(n)
}

pub fn find_duplicates(paths: &[String], progress: &mut dyn FnMut(CompareProgress)) -> DuplicatesReport {
    let mut unreadable = Vec::new();
    let mut by_bytes: HashMap<[u8; 16], Vec<usize>> = HashMap::new();
    let mut by_face: HashMap<u64, Vec<usize>> = HashMap::new();
    let mut byte_hash: HashMap<usize, [u8; 16]> = HashMap::new();
    // One representative per distinct face, for near-twins: (head mesh, compact).
    let mut faces: Vec<(usize, String, Compact)> = Vec::new();
    let mut key_ids: HashMap<Key, u32> = HashMap::new();

    for (i, path) in paths.iter().enumerate() {
        progress(CompareProgress {
            current: i + 1,
            total: paths.len(),
            detail: file_name(path).to_string(),
        });
        let (bytes, value) = match read_preset(Path::new(path)) {
            Ok(x) => x,
            Err(reason) => {
                unreadable.push(FailedFile {
                    path: path.clone(),
                    reason,
                });
                continue;
            }
        };
        let digest: [u8; 16] = Md5::digest(&bytes).into();
        by_bytes.entry(digest).or_default().push(i);
        byte_hash.insert(i, digest);
        let f = facts(&value, false);
        let face = face_identity(&f);
        let group = by_face.entry(face).or_default();
        group.push(i);
        if group.len() == 1 {
            let mesh = f
                .get(&(Section::HeadParts, 1, "Face".to_string()))
                .map(|x| x.identity.clone())
                .unwrap_or_default();
            let mut compact: Compact = f
                .iter()
                .filter(|(k, _)| k.0 != Section::Body)
                .map(|(k, fact)| {
                    let next = key_ids.len() as u32;
                    (*key_ids.entry(k.clone()).or_insert(next), hash_of(&fact.identity))
                })
                .collect();
            compact.sort_unstable();
            faces.push((i, mesh, compact));
        }
    }

    let refs = |ids: &[usize]| -> Vec<PresetRef> { ids.iter().map(|&i| preset_ref(&paths[i])).collect() };
    let by_name = |g: &DuplicateGroup| g.presets.first().map(|p| p.file_name.to_lowercase()).unwrap_or_default();
    let mut exact: Vec<DuplicateGroup> = by_bytes
        .values()
        .filter(|ids| ids.len() > 1)
        .map(|ids| DuplicateGroup { presets: refs(ids) })
        .collect();
    exact.sort_by_key(by_name);
    let mut same_face: Vec<DuplicateGroup> = by_face
        .values()
        .filter(|ids| ids.len() > 1)
        .filter(|ids| ids.iter().any(|i| byte_hash[i] != byte_hash[&ids[0]]))
        .map(|ids| DuplicateGroup { presets: refs(ids) })
        .collect();
    same_face.sort_by_key(by_name);

    // Near-twins: only faces sharing a head mesh are worth comparing.
    let mut blocks: HashMap<&str, Vec<usize>> = HashMap::new();
    for (n, (_, mesh, _)) in faces.iter().enumerate() {
        blocks.entry(mesh.as_str()).or_default().push(n);
    }
    let mut pairs: Vec<(usize, usize, usize)> = Vec::new();
    for members in blocks.values() {
        for (x, &a) in members.iter().enumerate() {
            for &b in &members[x + 1..] {
                if let Some(d) = differences_within(&faces[a].2, &faces[b].2, NEAR_TWIN_LIMIT) {
                    if d > 0 {
                        pairs.push((d, faces[a].0, faces[b].0));
                    }
                }
            }
        }
    }
    // Fully ordered, so which pairs make the cap never depends on hashing.
    let order = |p: &(usize, usize, usize)| {
        (
            p.0,
            file_name(&paths[p.1]).to_lowercase(),
            file_name(&paths[p.2]).to_lowercase(),
            paths[p.1].clone(),
            paths[p.2].clone(),
        )
    };
    pairs.sort_by_key(order);
    pairs.truncate(NEAR_TWIN_CAP);
    let near_twins = pairs
        .into_iter()
        .map(|(differences, a, b)| {
            let what = compare_paths(Path::new(&paths[a]), Path::new(&paths[b]))
                .map(|c| {
                    c.sections
                        .iter()
                        .filter(|s| s.section != Section::Body)
                        .flat_map(|s| s.items.iter().filter(|i| !i.same).map(move |i| format!("{}: {}", s.label, i.key)))
                        .collect()
                })
                .unwrap_or_default();
            NearTwin {
                left: preset_ref(&paths[a]),
                right: preset_ref(&paths[b]),
                differences,
                what,
            }
        })
        .collect();

    DuplicatesReport {
        total: paths.len() - unreadable.len(),
        exact,
        same_face,
        near_twins,
        unreadable,
    }
}

#[derive(Debug, Serialize)]
pub struct RemoveOutcome {
    pub snapshot_id: String,
    pub removed: Vec<String>,
    pub failed: Vec<FailedFile>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GroupKind {
    Exact,
    SameFace,
}

/// A group as the page showed it, so removal can be checked against it.
#[derive(Debug, Deserialize)]
pub struct RemovalGroup {
    pub kind: GroupKind,
    pub members: Vec<String>,
}

/// A file as it is right now: where it really lives, its bytes, its face.
struct Current {
    real: PathBuf,
    bytes: [u8; 16],
    face: u64,
}

fn current(path: &str) -> Option<Current> {
    let real = std::fs::canonicalize(path).ok()?;
    let (bytes, value) = read_preset(Path::new(path)).ok()?;
    Some(Current {
        real,
        bytes: Md5::digest(&bytes).into(),
        face: face_identity(&facts(&value, false)),
    })
}

/// Remove duplicates after snapshotting them, so History can put them back.
///
/// The page's report can be stale (a preset re-saved since the search), and
/// one file can appear under two path spellings, so nothing is trusted: every
/// group touched must keep a member, and every file removed must still match,
/// right now, a kept member that is a different file — identical bytes in an
/// exact group, the same face in a same-face group. Otherwise nothing is
/// removed.
pub fn remove_duplicates_in(
    settings: &settings::AppSettings,
    groups: &[RemovalGroup],
    paths: &[String],
) -> Result<RemoveOutcome, String> {
    if paths.is_empty() {
        return Err("Nothing chosen to remove.".to_string());
    }
    let removing: HashSet<&str> = paths.iter().map(String::as_str).collect();
    for p in paths {
        let f = Path::new(p);
        let is_preset = f.extension().is_some_and(|e| e.eq_ignore_ascii_case("jslot"));
        if !is_preset || !f.is_file() {
            return Err(format!("{p} isn't a preset file, so nothing was removed."));
        }
        if !groups.iter().any(|g| g.members.contains(p)) {
            return Err(format!("{p} isn't in a duplicate group, so nothing was removed."));
        }
    }
    let mut now: HashMap<&str, Option<Current>> = HashMap::new();
    for g in groups.iter().filter(|g| g.members.iter().any(|m| removing.contains(m.as_str()))) {
        for m in &g.members {
            now.entry(m.as_str()).or_insert_with(|| current(m));
        }
    }
    let mut problems: Vec<String> = Vec::new();
    for g in groups.iter().filter(|g| g.members.iter().any(|m| removing.contains(m.as_str()))) {
        let kept: Vec<&Current> = g
            .members
            .iter()
            .filter(|m| !removing.contains(m.as_str()))
            .filter_map(|m| now[m.as_str()].as_ref())
            .collect();
        for r in g.members.iter().filter(|m| removing.contains(m.as_str())) {
            let Some(gone) = now[r.as_str()].as_ref() else {
                problems.push(format!("{r} couldn't be read"));
                continue;
            };
            let covered = kept.iter().any(|k| {
                k.real != gone.real
                    && match g.kind {
                        GroupKind::Exact => k.bytes == gone.bytes,
                        GroupKind::SameFace => k.face == gone.face,
                    }
            });
            if !covered {
                problems.push(format!("{r} no longer has a matching copy that stays"));
            }
        }
    }
    if !problems.is_empty() {
        problems.sort();
        problems.dedup();
        return Err(format!(
            "Nothing was removed. Search again — the presets changed since the last search, or a group would lose every copy:\n{}",
            problems.join("\n")
        ));
    }

    // One file under two spellings is removed (and snapshotted) once.
    let mut seen: HashSet<PathBuf> = HashSet::new();
    let files: Vec<PathBuf> = paths
        .iter()
        .filter(|p| now.get(p.as_str()).and_then(|c| c.as_ref()).is_none_or(|c| seen.insert(c.real.clone())))
        .map(PathBuf::from)
        .collect();
    let meta = snapshot::write_snapshot(settings, "remove-duplicates", &files)?;
    let mut outcome = RemoveOutcome {
        snapshot_id: meta.id,
        removed: Vec::new(),
        failed: Vec::new(),
    };
    for f in &files {
        match std::fs::remove_file(f) {
            Ok(()) => outcome.removed.push(f.display().to_string()),
            Err(e) => outcome.failed.push(FailedFile {
                path: f.display().to_string(),
                reason: e.to_string(),
            }),
        }
    }
    Ok(outcome)
}

#[tauri::command]
pub async fn compare_presets(left: String, right: String) -> Result<Comparison, String> {
    tauri::async_runtime::spawn_blocking(move || compare_paths(Path::new(&left), Path::new(&right)))
        .await
        .map_err(|e| format!("compare task failed: {e}"))?
}

#[tauri::command]
pub async fn find_duplicate_presets(
    app: tauri::AppHandle,
    on_progress: tauri::ipc::Channel<CompareProgress>,
) -> Result<DuplicatesReport, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let settings = settings::load_from_app(&app)?;
        let paths: Vec<String> = scan::scan_settings(&settings).files.into_iter().map(|f| f.path).collect();
        if paths.is_empty() {
            return Err("No presets were found in your JSLOT locations.".to_string());
        }
        Ok(find_duplicates(&paths, &mut |p| {
            let _ = on_progress.send(p);
        }))
    })
    .await
    .map_err(|e| format!("duplicate search failed: {e}"))?
}

#[tauri::command]
pub async fn remove_duplicates(
    app: tauri::AppHandle,
    groups: Vec<RemovalGroup>,
    paths: Vec<String>,
) -> Result<RemoveOutcome, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let settings = settings::load_from_app(&app)?;
        remove_duplicates_in(&settings, &groups, &paths)
    })
    .await
    .map_err(|e| format!("remove task failed: {e}"))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn base() -> Value {
        json!({
            "actor": {"hairColor": 328965, "headTexture": "Skyrim.esm|03B522", "weight": 20},
            "headParts": [
                {"formId": 1, "formIdentifier": "High Poly Head.esm|000A0C", "type": 1},
                {"formId": 2, "formIdentifier": "KS Hairdo's.esp|001234", "type": 3},
                {"formId": 3, "formIdentifier": "Skyrim.esm|0EC1B2", "type": 2}
            ],
            "morphs": {
                "custom": [
                    {"name": "CME_EyesSize", "value": 0.429999977350235},
                    {"name": "CME_Zero", "value": 0.0}
                ],
                "default": {"morphs": [-0.2, 0.5], "presets": [13, 4294967295u64, 28, 3]},
                "sculpt": [{"host": "Actors\\Character\\Character Assets\\Mouth\\MouthHumanFChargen.tri",
                            "vertices": 2, "data": [[0, -316, 1195, 4], [1, -197, 1197, 2]]}],
                "sculptDivisor": 10000
            },
            "tintInfo": [
                {"color": 4291945965u64, "index": 0, "texture": "Actors\\Character\\Character Assets\\TintMasks\\SkinTone.dds"},
                {"color": 16775147, "index": 1, "texture": "Actors\\Character\\Character Assets\\TintMasks\\FemaleUpperEyeSocket.dds"}
            ],
            "faceTextures": [{"index": 0, "texture": "Actors\\Character\\Female\\FemaleHead.dds"}],
            "overrides": [
                {"node": "Face [Ovl0]", "values": [
                    {"data": "Actors\\Character\\Overlays\\Freckles\\f.dds", "index": 0, "key": 9, "type": 2},
                    {"data": 0.5, "index": -1, "key": 8, "type": 1}]},
                {"node": "Body [Ovl0]", "values": [
                    {"data": "Actors\\Character\\Overlays\\Tattoos\\t.dds", "index": 0, "key": 9, "type": 2}]}
            ],
            "bodyMorphs": [{"name": "Breasts", "keys": [{"key": "RaceMenuMorphsCBBE.esp", "value": 0.3}]}],
            "transforms": [{"node": "NPC", "firstPerson": false,
                "keys": [{"name": "RSMPlugin", "values": [{"data": 0.98, "index": 0, "key": 30, "type": 4}]}]}],
            "version": {"formatVersion": 3}
        })
    }

    fn get<'a>(f: &'a Facts, section: Section, key: &str) -> Option<&'a str> {
        f.iter().find(|(k, _)| k.0 == section && k.2 == key).map(|(_, v)| v.display.as_str())
    }

    #[test]
    fn a_preset_reads_into_named_facts() {
        let f = facts(&base(), true);
        use Section::*;
        assert_eq!(get(&f, HeadParts, "Hair"), Some("KS Hairdo's.esp|001234"));
        assert_eq!(get(&f, HeadParts, "Face"), Some("High Poly Head.esm|000A0C"));
        assert_eq!(get(&f, VanillaSliders, "Nose Long/Short"), Some("-0.2"));
        assert_eq!(get(&f, VanillaSliders, "Nose Up/Down"), Some("0.5"));
        assert_eq!(get(&f, VanillaSliders, "Unknown preset"), Some("none"));
        assert_eq!(get(&f, CustomSliders, "CME_EyesSize"), Some("0.43"));
        assert_eq!(get(&f, CustomSliders, "CME_Zero"), None, "a slider at 0 is absent");
        assert_eq!(get(&f, Sculpt, "MouthHumanFChargen.tri"), Some("2 vertices moved"));
        assert_eq!(get(&f, Tints, "SkinTone"), Some("#D1E5ED · 100%"));
        assert_eq!(get(&f, Tints, "FemaleUpperEyeSocket"), Some("off"), "zero alpha isn't drawn");
        assert_eq!(get(&f, FaceTextures, "Diffuse"), Some("Actors\\Character\\Female\\FemaleHead.dds"));
        assert_eq!(get(&f, FaceOverlays, "Face [Ovl0]"), Some("f.dds · 50%"));
        assert_eq!(get(&f, Appearance, "Hair colour"), Some("#050505"));
        assert_eq!(get(&f, Body, "Overlay: Body [Ovl0]"), Some("t.dds"));
        assert_eq!(get(&f, Body, "Body morph: Breasts"), Some("0.3"));
        assert_eq!(get(&f, Body, "Transform: NPC"), Some("0.98"));
        assert_eq!(get(&f, Body, "Weight"), Some("20"));
        // Positional keys keep the CK's order, not the alphabet's.
        let vanilla: Vec<&str> =
            f.keys().filter(|k| k.0 == VanillaSliders).map(|k| k.2.as_str()).take(2).collect();
        assert_eq!(vanilla, ["Nose Long/Short", "Nose Up/Down"]);
    }

    struct Dir(PathBuf);
    impl Dir {
        fn new(tag: &str) -> Dir {
            let d = std::env::temp_dir().join(format!("lineage-compare-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&d);
            std::fs::create_dir_all(&d).unwrap();
            Dir(d)
        }
        fn write(&self, name: &str, v: &Value) -> String {
            let p = self.0.join(name);
            std::fs::write(&p, serde_json::to_string_pretty(v).unwrap()).unwrap();
            p.display().to_string()
        }
    }
    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_comparison_names_each_difference_and_keeps_body_apart() {
        let dir = Dir::new("diff");
        let mut other = base();
        other["morphs"]["custom"][0]["value"] = json!(0.1);
        other["morphs"]["sculpt"][0]["data"][1] = json!([1, -197, 1197, 9]);
        other["tintInfo"][1]["color"] = json!(4278190335u64);
        other["bodyMorphs"][0]["keys"][0]["value"] = json!(1.0);
        let a = dir.write("a.jslot", &base());
        let b = dir.write("b.jslot", &other);

        let c = compare_paths(Path::new(&a), Path::new(&b)).unwrap();
        assert!(!c.same_bytes && !c.same_face);
        assert_eq!((c.face_differences, c.body_differences), (3, 1));
        type Row = (String, Option<String>, Option<String>, Option<String>);
        let changed: Vec<Row> = c
            .sections
            .iter()
            .flat_map(|s| s.items.iter().filter(|i| !i.same).map(|i| (i.key.clone(), i.left.clone(), i.right.clone(), i.note.clone())))
            .collect();
        let s = |x: &str| Some(x.to_string());
        assert_eq!(
            changed,
            [
                ("CME_EyesSize".to_string(), s("0.43"), s("0.1"), None),
                ("MouthHumanFChargen.tri".to_string(), s("2 vertices moved"), s("2 vertices moved"), s("1 vertex differs")),
                ("FemaleUpperEyeSocket".to_string(), s("off"), s("#0000FF · 100%"), None),
                ("Body morph: Breasts".to_string(), s("0.3"), s("1"), None),
            ]
        );
        let same = compare_paths(Path::new(&a), Path::new(&a)).unwrap();
        assert!(same.same_bytes && same.same_face && same.face_differences == 0);
    }

    #[test]
    fn duplicates_split_into_exact_same_face_and_near_twins() {
        let dir = Dir::new("dupes");
        let a = dir.write("A.jslot", &base());
        let copy = dir.write("A copy.jslot", &base());
        let mut body = base();
        body["actor"]["weight"] = json!(100);
        body["transforms"] = json!([]);
        let b = dir.write("B body.jslot", &body);
        let mut twin = base();
        twin["morphs"]["custom"][0]["value"] = json!(0.9);
        let c = dir.write("C twin.jslot", &twin);
        let mut stranger = base();
        stranger["headParts"][0]["formIdentifier"] = json!("Skyrim.esm|05150F");
        stranger["morphs"]["custom"][0]["value"] = json!(0.9);
        let d = dir.write("D stranger.jslot", &stranger);
        let broken = dir.0.join("E.jslot");
        std::fs::write(&broken, "{ nope").unwrap();

        let paths = vec![a.clone(), copy.clone(), b.clone(), c.clone(), d, broken.display().to_string()];
        let report = find_duplicates(&paths, &mut |_| {});
        let names = |g: &DuplicateGroup| {
            let mut n: Vec<String> = g.presets.iter().map(|p| p.file_name.clone()).collect();
            n.sort();
            n
        };
        assert_eq!(report.total, 5);
        assert_eq!(report.unreadable.len(), 1);
        assert_eq!(report.exact.len(), 1);
        assert_eq!(names(&report.exact[0]), ["A copy.jslot", "A.jslot"]);
        assert_eq!(report.same_face.len(), 1);
        assert_eq!(names(&report.same_face[0]), ["A copy.jslot", "A.jslot", "B body.jslot"]);
        // C is one slider from A. D has that slider too but another head
        // mesh, so it's never compared.
        assert_eq!(report.near_twins.len(), 1);
        let t = &report.near_twins[0];
        assert_eq!((t.left.file_name.as_str(), t.right.file_name.as_str(), t.differences), ("A.jslot", "C twin.jslot", 1));
        assert_eq!(t.what, ["Custom sliders: CME_EyesSize"]);
    }

    #[test]
    fn head_parts_saved_by_form_id_resolve_through_the_mods_table() {
        let with_mods = |plugin: &str| {
            json!({"mods": [{"index": 13, "name": plugin}],
                   "headParts": [{"formId": 0x0D043C55u64, "type": 3}]})
        };
        let hair = |v: &Value| get(&facts(v, false), Section::HeadParts, "Hair").map(str::to_string);
        assert_eq!(hair(&with_mods("KS Hairdo's.esp")).as_deref(), Some("KS Hairdo's.esp|043C55"));
        // The same raw id in another load order is another hair.
        assert_ne!(
            face_identity(&facts(&with_mods("KS Hairdo's.esp"), false)),
            face_identity(&facts(&with_mods("HG Hairdos 2.esp"), false))
        );
        // And it matches a preset that saved the identifier outright.
        let named = json!({"headParts": [{"formIdentifier": "KS Hairdo's.esp|043C55", "type": 3}]});
        assert_eq!(hair(&named), hair(&with_mods("KS Hairdo's.esp")));
    }

    #[test]
    fn masks_and_hosts_with_one_name_in_different_folders_differ() {
        let mut a = base();
        a["tintInfo"][0]["texture"] = json!("Actors\\ModA\\SkinTone.dds");
        let mut b = base();
        b["tintInfo"][0]["texture"] = json!("Actors\\ModB\\SkinTone.dds");
        assert_ne!(face_identity(&facts(&a, false)), face_identity(&facts(&b, false)));
        let mut c = base();
        c["morphs"]["sculpt"][0]["host"] = json!("Other\\MouthHumanFChargen.tri");
        assert_ne!(face_identity(&facts(&base(), false)), face_identity(&facts(&c, false)));
    }

    fn settings_in(dir: &Dir) -> settings::AppSettings {
        settings::AppSettings {
            backup_dir: dir.0.join("backups").display().to_string(),
            ..Default::default()
        }
    }

    fn exact(members: &[&String]) -> RemovalGroup {
        RemovalGroup {
            kind: GroupKind::Exact,
            members: members.iter().map(|m| m.to_string()).collect(),
        }
    }

    #[test]
    fn removal_is_refused_unless_a_matching_copy_stays() {
        let dir = Dir::new("refuse");
        let settings = settings_in(&dir);
        let a = dir.write("A.jslot", &base());
        let copy = dir.write("A copy.jslot", &base());

        // Every copy of a group.
        assert!(remove_duplicates_in(&settings, &[exact(&[&a, &copy])], &[a.clone(), copy.clone()]).is_err());
        // A path in no group.
        let loose = dir.write("B.jslot", &base());
        assert!(remove_duplicates_in(&settings, &[exact(&[&a, &copy])], std::slice::from_ref(&loose)).is_err());
        // One file under two spellings is not a copy of itself.
        let same_file = a.replace('\\', "/");
        assert!(remove_duplicates_in(&settings, &[exact(&[&a, &same_file])], std::slice::from_ref(&same_file)).is_err());
        // The keeper was re-saved since the search: no longer identical.
        let mut edited = base();
        edited["morphs"]["custom"][0]["value"] = json!(0.9);
        dir.write("A.jslot", &edited);
        assert!(remove_duplicates_in(&settings, &[exact(&[&a, &copy])], std::slice::from_ref(&copy)).is_err());
        assert!(Path::new(&copy).exists() && Path::new(&a).exists(), "a refusal removes nothing");
    }

    #[test]
    fn removal_snapshots_first_and_history_brings_it_back() {
        let dir = Dir::new("remove");
        let victim = dir.write("Extra.jslot", &base());
        let keeper = dir.write("Keeper.jslot", &base());
        let before = std::fs::read(&victim).unwrap();
        let settings = settings_in(&dir);
        let groups = [exact(&[&keeper, &victim])];

        let outcome = remove_duplicates_in(&settings, &groups, std::slice::from_ref(&victim)).unwrap();
        assert_eq!(outcome.removed, std::slice::from_ref(&victim));
        assert!(!Path::new(&victim).exists());
        snapshot::restore_snapshot_in(&settings, &outcome.snapshot_id).unwrap();
        assert_eq!(std::fs::read(&victim).unwrap(), before);
    }
}
