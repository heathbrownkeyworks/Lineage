//! JSLOT parsing and writing.
//!
//! `.jslot` files come in (at least) two formatting dialects, both confirmed
//! against real presets on this machine (see NOTES.md):
//!
//! * **Styled** — RaceMenu's own writer (jsoncpp StyledWriter): 3-space
//!   indent, `"key" : value` separators, short scalar arrays inlined as
//!   `[ a, b ]`, trailing newline.
//! * **Pretty** — presets converted by external tools (Python-style
//!   `json.dumps(indent=2)`): 2-space indent, `"key": value`, arrays always
//!   multiline, no trailing newline.
//!
//! Reading uses two parsers: `serde_json` for validation and analysis, and
//! the `rawjson` tree for write-backs — the raw tree keeps every scalar as
//! its verbatim source token (serde_json normalizes `-0`, `1e5`, and string
//! escapes even with `arbitrary_precision`). Writing detects each file's
//! dialect and reproduces it, so a parse → serialize round trip reproduces
//! the original bytes. `clean::inspect` verifies that per file and reports it
//! honestly instead of assuming.

use crate::rawjson::Raw;
use serde_json::Value;
use std::path::Path;

/// The body-morph sections Lineage removes. A constant list rather than a
/// single hardcoded string so more section names are easy to add if other
/// tools are found writing them. Only `bodyMorphs` is confirmed from real
/// RaceMenu presets so far.
pub const BODY_MORPH_KEYS: &[&str] = &["bodyMorphs"];

/// jsoncpp StyledWriter wraps lines at this margin when deciding whether an
/// array fits on one line.
const RIGHT_MARGIN: usize = 74;
const INDENT: &str = "   ";

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

/// Strip an optional UTF-8 BOM and return (had_bom, text).
pub(crate) fn read_text(bytes: &[u8]) -> (bool, String) {
    match bytes.strip_prefix(b"\xef\xbb\xbf") {
        Some(rest) => (true, String::from_utf8_lossy(rest).into_owned()),
        None => (false, String::from_utf8_lossy(bytes).into_owned()),
    }
}

/// Parse a preset for analysis (serde_json Value). Malformed files come back
/// as a readable error, not a panic.
pub fn parse_file(path: &Path) -> Result<Value, String> {
    let bytes =
        std::fs::read(path).map_err(|e| format!("Couldn't read {}: {e}", path.display()))?;
    let (_, text) = read_text(&bytes);
    serde_json::from_str(&text).map_err(|e| format!("Not valid preset JSON ({e})"))
}

/// The first configured body-morph key present on this raw object.
pub fn body_morph_key(raw: &Raw) -> Option<&'static str> {
    BODY_MORPH_KEYS
        .iter()
        .copied()
        .find(|k| raw.get(k).is_some())
}

/// Names inside a body-morph section (`bodyMorphs[].name`), for the preview.
pub fn body_morph_names(raw: &Raw) -> Vec<String> {
    let Some(Raw::Array(entries)) = body_morph_key(raw).and_then(|key| raw.get(key)) else {
        return Vec::new();
    };
    entries
        .iter()
        .filter_map(|e| e.get("name").and_then(Raw::as_str))
        .collect()
}

// ---------------------------------------------------------------------------
// Format detection
// ---------------------------------------------------------------------------

/// The formatting a JSLOT file was written with, detected from its text so a
/// write-back reproduces it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JslotFormat {
    pub dialect: Dialect,
    pub trailing_newline: bool,
    pub bom: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dialect {
    /// jsoncpp StyledWriter (RaceMenu): 3-space indent, `"key" : value`.
    /// `inline_strings` is false for the Mimic-style variant that inlines
    /// short number arrays but always breaks string arrays across lines.
    Styled { inline_strings: bool },
    /// Standard pretty JSON: N-space indent, `"key": value`, no inline arrays.
    Pretty { indent: usize },
    /// Single-line minified JSON.
    Compact,
}

impl Default for JslotFormat {
    fn default() -> Self {
        Self {
            dialect: Dialect::Styled {
                inline_strings: true,
            },
            trailing_newline: true,
            bom: false,
        }
    }
}

/// Does the text contain a multiline array that opens directly onto a quoted
/// string (`[` at end of line, next line starting with `"`)?
fn has_multiline_string_array(text: &str) -> bool {
    let bytes = text.as_bytes();
    let mut i = 0;
    while let Some(off) = text[i..].find("[\n") {
        let mut j = i + off + 2;
        while j < bytes.len() && (bytes[j] == b' ' || bytes[j] == b'\t') {
            j += 1;
        }
        if bytes.get(j) == Some(&b'"') {
            return true;
        }
        i += off + 2;
    }
    false
}

pub fn detect_format(text: &str, bom: bool) -> JslotFormat {
    let trailing_newline = text.ends_with('\n');
    if !text.trim_end().contains('\n') {
        return JslotFormat {
            dialect: Dialect::Compact,
            trailing_newline,
            bom,
        };
    }
    // First indented key line decides: `" : "` = StyledWriter, `": "` = Pretty.
    for line in text.lines() {
        let indent = line.len() - line.trim_start().len();
        if indent == 0 || !line.trim_start().starts_with('"') {
            continue;
        }
        if line.contains("\" : ") {
            // Sniff the string-array style. Safe in every case: a file whose
            // short string arrays are inlined contains `[ "`; a file with only
            // too-long string arrays produces identical output either way.
            let inline_strings =
                text.contains("[ \"") || !has_multiline_string_array(text);
            return JslotFormat {
                dialect: Dialect::Styled { inline_strings },
                trailing_newline,
                bom,
            };
        }
        if line.contains("\": ") {
            return JslotFormat {
                dialect: Dialect::Pretty { indent },
                trailing_newline,
                bom,
            };
        }
    }
    JslotFormat {
        dialect: Dialect::Styled {
            inline_strings: true,
        },
        trailing_newline,
        bom,
    }
}

// ---------------------------------------------------------------------------
// Dialect-preserving serialization (over the raw-token tree)
// ---------------------------------------------------------------------------

/// Serialize in the given format so everything Lineage did not change stays
/// byte-for-byte identical.
pub fn to_formatted_string(raw: &Raw, format: JslotFormat) -> String {
    let mut out = String::new();
    if format.bom {
        out.push('\u{feff}');
    }
    match format.dialect {
        Dialect::Styled { inline_strings } => write_styled(raw, 0, inline_strings, &mut out),
        Dialect::Pretty { indent } => write_pretty(raw, 0, indent, &mut out),
        Dialect::Compact => write_compact(raw, &mut out),
    }
    if format.trailing_newline {
        out.push('\n');
    }
    out
}

fn write_styled(raw: &Raw, depth: usize, inline_strings: bool, out: &mut String) {
    match raw {
        Raw::Scalar(token) => out.push_str(token),
        Raw::Array(items) => {
            if items.is_empty() {
                out.push_str("[]");
                return;
            }
            // StyledWriter: multiline when any child is a non-empty container,
            // or the one-line form would blow past the right margin. The
            // Mimic-style variant also breaks any array holding strings.
            let has_container = items.iter().any(|v| match v {
                Raw::Array(a) => !a.is_empty(),
                Raw::Object(o) => !o.is_empty(),
                Raw::Scalar(_) => false,
            });
            let has_string = items
                .iter()
                .any(|v| matches!(v, Raw::Scalar(t) if t.starts_with('"')));
            let mut multiline = has_container
                || (has_string && !inline_strings)
                || items.len() * 3 >= RIGHT_MARGIN;
            if !multiline {
                let line_len = 4
                    + (items.len() - 1) * 2
                    + items
                        .iter()
                        .map(|v| match v {
                            Raw::Scalar(t) => t.len(),
                            _ => 2,
                        })
                        .sum::<usize>();
                multiline = line_len >= RIGHT_MARGIN;
            }
            if multiline {
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    out.push('\n');
                    push_indent(depth + 1, out);
                    write_styled(item, depth + 1, inline_strings, out);
                    if i + 1 < items.len() {
                        out.push(',');
                    }
                }
                out.push('\n');
                push_indent(depth, out);
                out.push(']');
            } else {
                out.push_str("[ ");
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    write_styled(item, depth, inline_strings, out);
                }
                out.push_str(" ]");
            }
        }
        Raw::Object(members) => {
            if members.is_empty() {
                out.push_str("{}");
                return;
            }
            out.push('{');
            for (i, (key, val)) in members.iter().enumerate() {
                out.push('\n');
                push_indent(depth + 1, out);
                out.push_str(key);
                out.push_str(" : ");
                write_styled(val, depth + 1, inline_strings, out);
                if i + 1 < members.len() {
                    out.push(',');
                }
            }
            out.push('\n');
            push_indent(depth, out);
            out.push('}');
        }
    }
}

fn push_indent(depth: usize, out: &mut String) {
    for _ in 0..depth {
        out.push_str(INDENT);
    }
}

fn write_pretty(raw: &Raw, depth: usize, indent: usize, out: &mut String) {
    match raw {
        Raw::Scalar(token) => out.push_str(token),
        Raw::Array(items) => {
            if items.is_empty() {
                out.push_str("[]");
                return;
            }
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                out.push('\n');
                out.push_str(&" ".repeat(indent * (depth + 1)));
                write_pretty(item, depth + 1, indent, out);
                if i + 1 < items.len() {
                    out.push(',');
                }
            }
            out.push('\n');
            out.push_str(&" ".repeat(indent * depth));
            out.push(']');
        }
        Raw::Object(members) => {
            if members.is_empty() {
                out.push_str("{}");
                return;
            }
            out.push('{');
            for (i, (key, val)) in members.iter().enumerate() {
                out.push('\n');
                out.push_str(&" ".repeat(indent * (depth + 1)));
                out.push_str(key);
                out.push_str(": ");
                write_pretty(val, depth + 1, indent, out);
                if i + 1 < members.len() {
                    out.push(',');
                }
            }
            out.push('\n');
            out.push_str(&" ".repeat(indent * depth));
            out.push('}');
        }
    }
}

fn write_compact(raw: &Raw, out: &mut String) {
    match raw {
        Raw::Scalar(token) => out.push_str(token),
        Raw::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_compact(item, out);
            }
            out.push(']');
        }
        Raw::Object(members) => {
            out.push('{');
            for (i, (key, val)) in members.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(key);
                out.push(':');
                write_compact(val, out);
            }
            out.push('}');
        }
    }
}

/// Render one top-level section (`"key" : <value>`) exactly as it appears in
/// the file — same dialect, same nesting indentation, minus only the leading
/// indent of the first line. Drives the "what will be removed" preview.
pub fn section_preview(raw: &Raw, key: &str, format: JslotFormat) -> Option<String> {
    let value = raw.get(key)?;
    let mut out = String::new();
    match format.dialect {
        Dialect::Styled { inline_strings } => {
            out.push('"');
            out.push_str(key);
            out.push_str("\" : ");
            write_styled(value, 1, inline_strings, &mut out);
        }
        Dialect::Pretty { indent } => {
            out.push('"');
            out.push_str(key);
            out.push_str("\": ");
            write_pretty(value, 1, indent, &mut out);
        }
        Dialect::Compact => {
            out.push('"');
            out.push_str(key);
            out.push_str("\":");
            write_compact(value, &mut out);
        }
    }
    Some(out)
}

// ---------------------------------------------------------------------------
// Round-trip verification
// ---------------------------------------------------------------------------

/// Would parse → serialize reproduce this file byte-for-byte? Used to report
/// honestly whether formatting survives, never assumed.
pub fn roundtrip_faithful(original: &[u8], raw: &Raw) -> bool {
    let (bom, text) = read_text(original);
    let format = detect_format(&text, bom);
    to_formatted_string(raw, format).as_bytes() == original
}

// ---------------------------------------------------------------------------
// Atomic writes
// ---------------------------------------------------------------------------

/// Write `contents` to `path` atomically: temp file in the same folder (an
/// atomic same-volume replace requires it), then swap into place. The temp
/// file is removed on every failure path — Lineage never leaves stray files
/// in a mod folder.
pub fn write_atomic(path: &Path, contents: &[u8]) -> Result<(), String> {
    let dir = path
        .parent()
        .ok_or_else(|| format!("{} has no parent folder", path.display()))?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp = dir.join(format!(".lineage-{}-{stamp}.tmp", std::process::id()));

    let result = (|| -> Result<(), String> {
        std::fs::write(&tmp, contents).map_err(|e| format!("write temp file: {e}"))?;
        replace_file(&tmp, path).map_err(|e| format!("replace {}: {e}", path.display()))
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// Atomically move `from` over `to`, replacing it.
#[cfg(target_os = "windows")]
fn replace_file(from: &Path, to: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    // MOVEFILE_REPLACE_EXISTING(0x1) | MOVEFILE_WRITE_THROUGH(0x8): an atomic
    // same-volume replace that doesn't return until the file is on disk —
    // a power cut mid-operation leaves either the old or the new preset,
    // never a truncated one. std::fs::rename can't replace on Windows.
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn MoveFileExW(from: *const u16, to: *const u16, flags: u32) -> i32;
    }
    let wide = |p: &Path| -> Vec<u16> {
        p.as_os_str().encode_wide().chain(std::iter::once(0)).collect()
    };
    let from_w = wide(from);
    let to_w = wide(to);
    let ok = unsafe { MoveFileExW(from_w.as_ptr(), to_w.as_ptr(), 0x1 | 0x8) };
    if ok == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn replace_file(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::rename(from, to)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rawjson;

    const SAMPLE: &str = r#"{
   "actor" : {
      "hairColor" : 1118481,
      "headTexture" : "Skyrim.esm|03B522",
      "weight" : 100
   },
   "bodyMorphs" : [
      {
         "keys" : [
            {
               "key" : "XPMSE.esp",
               "value" : 1
            }
         ],
         "name" : "XPMSEAABase_2hweqp"
      }
   ],
   "morphs" : {
      "custom" : [
         {
            "name" : "EFM_Brow_Width",
            "value" : 1.689999938011169
         }
      ]
   },
   "version" : {
      "skseVersion" : 33554736
   }
}
"#;

    fn parse_raw(text: &str) -> Raw {
        rawjson::parse(text).unwrap()
    }

    #[test]
    fn styled_roundtrip_is_byte_identical() {
        let raw = parse_raw(SAMPLE);
        assert!(roundtrip_faithful(SAMPLE.as_bytes(), &raw));
    }

    #[test]
    fn pretty_roundtrip_is_byte_identical() {
        let sample = "{\n  \"actor\": {\n    \"weight\": 100\n  },\n  \"bodyMorphs\": [],\n  \"tintInfo\": [\n    {\n      \"color\": 1,\n      \"index\": 0\n    }\n  ]\n}";
        let raw = parse_raw(sample);
        assert!(roundtrip_faithful(sample.as_bytes(), &raw));
    }

    #[test]
    fn short_scalar_arrays_inline_like_styledwriter() {
        let raw = parse_raw(r#"{"a":[1,2,3]}"#);
        let format = JslotFormat::default();
        assert_eq!(
            to_formatted_string(&raw, format),
            "{\n   \"a\" : [ 1, 2, 3 ]\n}\n"
        );
    }

    #[test]
    fn empty_containers_stay_compact() {
        let raw = parse_raw(r#"{"a":[],"b":{}}"#);
        assert_eq!(
            to_formatted_string(&raw, JslotFormat::default()),
            "{\n   \"a\" : [],\n   \"b\" : {}\n}\n"
        );
    }

    #[test]
    fn number_text_survives_exactly() {
        let raw = parse_raw(r#"{"v":0.2599999904632568,"w":100,"z":-0,"f":-0.0,"e":1e5}"#);
        let out = to_formatted_string(&raw, JslotFormat::default());
        assert!(out.contains("0.2599999904632568"));
        assert!(out.contains("\"w\" : 100"));
        assert!(out.contains("\"z\" : -0"), "negative zero must survive: {out}");
        assert!(out.contains("\"f\" : -0.0"), "-0.0 must survive: {out}");
        assert!(out.contains("\"e\" : 1e5"), "exponent form must survive: {out}");
    }

    #[test]
    fn section_preview_matches_the_file_verbatim() {
        let raw = parse_raw(SAMPLE);
        let preview = section_preview(&raw, "bodyMorphs", detect_format(SAMPLE, false)).unwrap();
        // The preview must be exactly the section's slice of the file, minus
        // the first line's leading indent (and the trailing comma).
        let start = SAMPLE.find("\"bodyMorphs\"").unwrap();
        let end = SAMPLE.find("],\n   \"morphs\"").unwrap() + 1;
        assert_eq!(preview, &SAMPLE[start..end]);

        // Pretty dialect keeps its own separator and indent.
        let pretty = "{\n  \"bodyMorphs\": [\n    {\n      \"name\": \"X\"\n    }\n  ],\n  \"z\": 1\n}";
        let raw = parse_raw(pretty);
        let preview = section_preview(&raw, "bodyMorphs", detect_format(pretty, false)).unwrap();
        assert_eq!(preview, "\"bodyMorphs\": [\n    {\n      \"name\": \"X\"\n    }\n  ]");

        // Absent section → no preview.
        assert!(section_preview(&raw, "nope", detect_format(pretty, false)).is_none());
    }

    /// The real corpus check: parse → serialize the user's actual presets and
    /// require byte-identical output. Skips quietly on machines without the
    /// corpus folder.
    #[test]
    fn roundtrip_against_real_presets() {
        let corpus = std::path::Path::new(
            r"D:\Nordic Souls\mods\ColdSun's Face Presets\SKSE\Plugins\CharGen\Presets",
        );
        if !corpus.is_dir() {
            return;
        }
        let mut checked = 0;
        let mut unfaithful: Vec<String> = Vec::new();
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
            let original = std::fs::read(entry.path()).unwrap();
            let (bom, text) = read_text(&original);
            if serde_json::from_str::<Value>(&text).is_err() {
                continue; // malformed files are a separate feature path
            }
            let Ok(raw) = rawjson::parse(&text) else {
                unfaithful.push(format!("{} (raw parse failed)", entry.path().display()));
                continue;
            };
            checked += 1;
            if !roundtrip_faithful(&original, &raw) {
                if unfaithful.is_empty() {
                    // Show where the first file diverges to make fixes easy.
                    let format = detect_format(&text, bom);
                    let produced = to_formatted_string(&raw, format);
                    let a = produced.as_bytes();
                    let b = &original[..];
                    let n = a.len().min(b.len());
                    let diff_at = (0..n).find(|&i| a[i] != b[i]).unwrap_or(n);
                    let s = diff_at.saturating_sub(120);
                    eprintln!("FIRST DIFF in {} at byte {diff_at}", entry.path().display());
                    eprintln!(
                        "== ORIGINAL ==\n{}",
                        String::from_utf8_lossy(&b[s..(diff_at + 160).min(b.len())])
                    );
                    eprintln!(
                        "== PRODUCED ==\n{}",
                        String::from_utf8_lossy(&a[s..(diff_at + 160).min(a.len())])
                    );
                }
                unfaithful.push(entry.path().display().to_string());
            }
        }
        assert!(checked > 0, "corpus folder present but no presets parsed");
        // A handful of hand-edited files carry irregular whitespace (e.g.
        // Sigrun.jslot has a 9-space indent on one line) that no layout
        // algorithm can reproduce; `inspect` reports those honestly as
        // roundtrip_faithful=false. Anything above 1% means a real
        // dialect regression, not stray hand edits.
        let ratio = unfaithful.len() as f64 / checked as f64;
        assert!(
            ratio < 0.01,
            "{} of {} real presets did not round-trip byte-identically:\n{}",
            unfaithful.len(),
            checked,
            unfaithful.join("\n")
        );
    }
}
