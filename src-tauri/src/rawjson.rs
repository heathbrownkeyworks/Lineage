//! A minimal raw-token JSON tree for byte-faithful write-backs.
//!
//! serde_json (even with `arbitrary_precision`) normalizes some number
//! tokens (`-0` → `0`, `1e5` → `1e+5`) and re-encodes string escapes, which
//! breaks the "everything you didn't change stays byte-for-byte identical"
//! requirement. This tree keeps every scalar — numbers, strings, booleans,
//! null — as its verbatim source slice, so serialization re-emits exactly
//! what was read. Only the container layout is reproduced (see
//! `jslot::detect_format` for the dialect rules).
//!
//! serde_json remains the parser of record for validation and analysis; this
//! module is used only by operations that write presets back.

/// One JSON value with scalars kept as raw source tokens (strings include
/// their surrounding quotes).
#[derive(Debug, Clone, PartialEq)]
pub enum Raw {
    /// Verbatim scalar token: `-0`, `1e5`, `"KS Hairdo's.esp"`, `true`, `null`.
    Scalar(String),
    Array(Vec<Raw>),
    /// Keys are verbatim tokens including quotes.
    Object(Vec<(String, Raw)>),
}

impl Raw {
    /// Find an object member by its (unescaped, exact) key name.
    pub fn get(&self, key: &str) -> Option<&Raw> {
        let Raw::Object(members) = self else { return None };
        let quoted = format!("\"{key}\"");
        members.iter().find(|(k, _)| *k == quoted).map(|(_, v)| v)
    }

    /// Remove an object member by key name; true when something was removed.
    pub fn remove(&mut self, key: &str) -> bool {
        let Raw::Object(members) = self else { return false };
        let quoted = format!("\"{key}\"");
        let before = members.len();
        members.retain(|(k, _)| *k != quoted);
        members.len() != before
    }

    /// The unescaped string value, if this is a string scalar.
    pub fn as_str(&self) -> Option<String> {
        let Raw::Scalar(token) = self else { return None };
        let inner = token.strip_prefix('"')?.strip_suffix('"')?;
        Some(unescape(inner))
    }
}

fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('"') => out.push('"'),
            Some('\\') => out.push('\\'),
            Some('/') => out.push('/'),
            Some('b') => out.push('\u{8}'),
            Some('f') => out.push('\u{c}'),
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('t') => out.push('\t'),
            Some('u') => {
                let hex: String = chars.by_ref().take(4).collect();
                if let Ok(code) = u32::from_str_radix(&hex, 16) {
                    if (0xd800..0xdc00).contains(&code) {
                        // High surrogate: expect \uXXXX low surrogate next.
                        let mut rest = chars.clone();
                        if rest.next() == Some('\\') && rest.next() == Some('u') {
                            let low_hex: String = rest.by_ref().take(4).collect();
                            if let Ok(low) = u32::from_str_radix(&low_hex, 16) {
                                if (0xdc00..0xe000).contains(&low) {
                                    let combined =
                                        0x10000 + ((code - 0xd800) << 10) + (low - 0xdc00);
                                    if let Some(ch) = char::from_u32(combined) {
                                        out.push(ch);
                                        chars = rest;
                                        continue;
                                    }
                                }
                            }
                        }
                        out.push('\u{fffd}');
                    } else {
                        out.push(char::from_u32(code).unwrap_or('\u{fffd}'));
                    }
                }
            }
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Parser
// ---------------------------------------------------------------------------

pub fn parse(text: &str) -> Result<Raw, String> {
    let bytes = text.as_bytes();
    let mut pos = 0usize;
    skip_ws(bytes, &mut pos);
    let value = parse_value(bytes, &mut pos)?;
    skip_ws(bytes, &mut pos);
    if pos != bytes.len() {
        return Err(format!("trailing content at byte {pos}"));
    }
    Ok(value)
}

fn skip_ws(bytes: &[u8], pos: &mut usize) {
    while *pos < bytes.len() && matches!(bytes[*pos], b' ' | b'\t' | b'\n' | b'\r') {
        *pos += 1;
    }
}

fn parse_value(bytes: &[u8], pos: &mut usize) -> Result<Raw, String> {
    match bytes.get(*pos) {
        Some(b'{') => parse_object(bytes, pos),
        Some(b'[') => parse_array(bytes, pos),
        Some(b'"') => Ok(Raw::Scalar(parse_string_token(bytes, pos)?)),
        Some(_) => {
            // number / true / false / null: scan the bare token.
            let start = *pos;
            while *pos < bytes.len()
                && !matches!(bytes[*pos], b',' | b'}' | b']' | b' ' | b'\t' | b'\n' | b'\r')
            {
                *pos += 1;
            }
            if start == *pos {
                return Err(format!("unexpected character at byte {start}"));
            }
            Ok(Raw::Scalar(
                String::from_utf8_lossy(&bytes[start..*pos]).into_owned(),
            ))
        }
        None => Err("unexpected end of input".into()),
    }
}

fn parse_string_token(bytes: &[u8], pos: &mut usize) -> Result<String, String> {
    let start = *pos;
    debug_assert_eq!(bytes[start], b'"');
    *pos += 1;
    while *pos < bytes.len() {
        match bytes[*pos] {
            b'\\' => *pos += 2,
            b'"' => {
                *pos += 1;
                return Ok(String::from_utf8_lossy(&bytes[start..*pos]).into_owned());
            }
            _ => *pos += 1,
        }
    }
    Err(format!("unterminated string starting at byte {start}"))
}

fn parse_array(bytes: &[u8], pos: &mut usize) -> Result<Raw, String> {
    *pos += 1; // [
    let mut items = Vec::new();
    skip_ws(bytes, pos);
    if bytes.get(*pos) == Some(&b']') {
        *pos += 1;
        return Ok(Raw::Array(items));
    }
    loop {
        skip_ws(bytes, pos);
        items.push(parse_value(bytes, pos)?);
        skip_ws(bytes, pos);
        match bytes.get(*pos) {
            Some(b',') => *pos += 1,
            Some(b']') => {
                *pos += 1;
                return Ok(Raw::Array(items));
            }
            _ => return Err(format!("expected ',' or ']' at byte {pos}", pos = *pos)),
        }
    }
}

fn parse_object(bytes: &[u8], pos: &mut usize) -> Result<Raw, String> {
    *pos += 1; // {
    let mut members = Vec::new();
    skip_ws(bytes, pos);
    if bytes.get(*pos) == Some(&b'}') {
        *pos += 1;
        return Ok(Raw::Object(members));
    }
    loop {
        skip_ws(bytes, pos);
        if bytes.get(*pos) != Some(&b'"') {
            return Err(format!("expected object key at byte {pos}", pos = *pos));
        }
        let key = parse_string_token(bytes, pos)?;
        skip_ws(bytes, pos);
        if bytes.get(*pos) != Some(&b':') {
            return Err(format!("expected ':' at byte {pos}", pos = *pos));
        }
        *pos += 1;
        skip_ws(bytes, pos);
        let value = parse_value(bytes, pos)?;
        members.push((key, value));
        skip_ws(bytes, pos);
        match bytes.get(*pos) {
            Some(b',') => *pos += 1,
            Some(b'}') => {
                *pos += 1;
                return Ok(Raw::Object(members));
            }
            _ => return Err(format!("expected ',' or '}}' at byte {pos}", pos = *pos)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scalars_keep_their_exact_tokens() {
        let raw = parse(r#"{"a": -0, "b": 1e5, "c": "KS Hairdo's.esp", "d": -0.0}"#).unwrap();
        let Raw::Object(members) = &raw else { panic!() };
        assert_eq!(members[0].1, Raw::Scalar("-0".into()));
        assert_eq!(members[1].1, Raw::Scalar("1e5".into()));
        assert_eq!(members[2].1, Raw::Scalar(r#""KS Hairdo's.esp""#.into()));
        assert_eq!(members[2].1.as_str().unwrap(), "KS Hairdo's.esp");
        assert_eq!(members[3].1, Raw::Scalar("-0.0".into()));
    }

    #[test]
    fn get_and_remove_by_key() {
        let mut raw = parse(r#"{"bodyMorphs": [1, 2], "morphs": {}}"#).unwrap();
        assert!(raw.get("bodyMorphs").is_some());
        assert!(raw.remove("bodyMorphs"));
        assert!(raw.get("bodyMorphs").is_none());
        assert!(!raw.remove("bodyMorphs"));
        assert!(raw.get("morphs").is_some());
    }

    #[test]
    fn surrogate_pairs_unescape() {
        let text = concat!(
            r#"{"e": ""#,
            "\\ud83d\\ude00",
            r#"", "n": ""#,
            "\\u00e9",
            r#""}"#
        );
        let raw = parse(text).unwrap();
        assert_eq!(raw.get("e").unwrap().as_str().unwrap(), "😀");
        assert_eq!(raw.get("n").unwrap().as_str().unwrap(), "é");
        // The raw token itself stays escaped for byte-faithful re-emission.
        assert_eq!(
            raw.get("e").unwrap(),
            &Raw::Scalar(format!("\"{}\"", "\\ud83d\\ude00"))
        );
    }
}
