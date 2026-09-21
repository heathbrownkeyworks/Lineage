//! File names inside a Bethesda archive (TES4-family BSA: v103 Oblivion,
//! v104 Skyrim LE / Fallout 3 / New Vegas, v105 Skyrim SE).
//!
//! Only the name tables are read — never file contents — so listing even a
//! multi-GB texture archive touches a few MB at its head. Layout (UESP):
//! a 36-byte header; one record per folder (16 bytes, 24 in v105); then per
//! folder a length-prefixed name and its 16-byte file records; then every
//! file name, NUL-terminated, in file-record order.

use std::io::{BufReader, Read};
use std::path::Path;

const INCLUDE_DIRECTORY_NAMES: u32 = 0x1;
const INCLUDE_FILE_NAMES: u32 = 0x2;
/// Far past any real archive (vanilla Textures0 holds ~10k files): a count
/// beyond this means a corrupt header, not a big archive.
const MAX_ENTRIES: u32 = 5_000_000;

fn read_u32(r: &mut impl Read) -> std::io::Result<u32> {
    let mut b = [0u8; 4];
    r.read_exact(&mut b)?;
    Ok(u32::from_le_bytes(b))
}

fn skip(r: &mut impl Read, n: u64) -> std::io::Result<()> {
    let copied = std::io::copy(&mut r.take(n), &mut std::io::sink())?;
    if copied == n {
        Ok(())
    } else {
        Err(std::io::ErrorKind::UnexpectedEof.into())
    }
}

/// Call `f` with every file in the archive as `folder\file`, lowercased,
/// backslash-separated (`textures\actors\character\x.dds`).
pub fn for_each_path(path: &Path, f: &mut dyn FnMut(&str)) -> Result<(), String> {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let fail = |e: std::io::Error| format!("{name} couldn't be read as an archive ({e})");
    let file = std::fs::File::open(path).map_err(fail)?;
    let len = file.metadata().map_err(fail)?.len();
    let mut r = BufReader::new(file);

    let mut magic = [0u8; 4];
    r.read_exact(&mut magic).map_err(fail)?;
    if &magic != b"BSA\0" {
        return Err(format!("{name} isn't a Skyrim-era BSA archive."));
    }
    let version = read_u32(&mut r).map_err(fail)?;
    if !matches!(version, 103..=105) {
        return Err(format!("{name} is BSA version {version}, which Lineage can't read."));
    }
    let header_size = read_u32(&mut r).map_err(fail)?;
    let flags = read_u32(&mut r).map_err(fail)?;
    let folder_count = read_u32(&mut r).map_err(fail)?;
    let file_count = read_u32(&mut r).map_err(fail)?;
    let _folder_names_len = read_u32(&mut r).map_err(fail)?;
    let file_names_len = read_u32(&mut r).map_err(fail)?;
    let _file_flags = read_u32(&mut r).map_err(fail)?;
    // Every table must fit in the file: a damaged header mustn't be able to
    // ask for gigabytes of memory.
    let record_size: u64 = if version == 105 { 24 } else { 16 };
    let tables = u64::from(folder_count) * record_size + u64::from(file_count) * 16 + u64::from(file_names_len);
    if folder_count > MAX_ENTRIES || file_count > MAX_ENTRIES || tables > len {
        return Err(format!("{name} has a damaged header."));
    }
    if flags & INCLUDE_DIRECTORY_NAMES == 0 || flags & INCLUDE_FILE_NAMES == 0 {
        // Names stripped (hash-only archives): nothing to match against.
        return Ok(());
    }
    skip(&mut r, u64::from(header_size.saturating_sub(36))).map_err(fail)?;

    // Folder records: only the per-folder file counts matter here.
    let mut counts = Vec::with_capacity(folder_count as usize);
    for _ in 0..folder_count {
        skip(&mut r, 8).map_err(fail)?; // name hash
        counts.push(read_u32(&mut r).map_err(fail)?);
        skip(&mut r, record_size - 12).map_err(fail)?;
    }

    // Per folder: bzstring name, then its file records.
    let mut folders = Vec::with_capacity(counts.len());
    for &count in &counts {
        let mut len = [0u8; 1];
        r.read_exact(&mut len).map_err(fail)?;
        let mut bytes = vec![0u8; usize::from(len[0])];
        r.read_exact(&mut bytes).map_err(fail)?;
        let text = String::from_utf8_lossy(&bytes);
        folders.push(text.trim_end_matches('\0').replace('/', "\\").to_lowercase());
        skip(&mut r, u64::from(count) * 16).map_err(fail)?;
    }

    // File names, in record order.
    let mut names = vec![0u8; file_names_len as usize];
    r.read_exact(&mut names).map_err(fail)?;
    let mut names = names.split(|&b| b == 0);
    let mut full = String::new();
    for (folder, &count) in folders.iter().zip(&counts) {
        for _ in 0..count {
            let Some(file) = names.next() else {
                return Err(format!("{name} has fewer file names than files."));
            };
            full.clear();
            full.push_str(folder);
            full.push('\\');
            full.push_str(&String::from_utf8_lossy(file).to_lowercase());
            f(&full);
        }
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A minimal, well-formed archive: names only, no file data.
    pub(crate) fn build(version: u32, folders: &[(&str, &[&str])]) -> Vec<u8> {
        let file_count: usize = folders.iter().map(|(_, files)| files.len()).sum();
        let folder_names_len: usize = folders.iter().map(|(name, _)| name.len() + 1).sum();
        let file_names: Vec<u8> = folders
            .iter()
            .flat_map(|(_, files)| files.iter())
            .flat_map(|f| f.bytes().chain(std::iter::once(0)))
            .collect();
        let mut out = Vec::new();
        out.extend_from_slice(b"BSA\0");
        for v in [
            version,
            36,
            INCLUDE_DIRECTORY_NAMES | INCLUDE_FILE_NAMES,
            folders.len() as u32,
            file_count as u32,
            folder_names_len as u32,
            file_names.len() as u32,
            0,
        ] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        for (_, files) in folders {
            out.extend_from_slice(&0u64.to_le_bytes());
            out.extend_from_slice(&(files.len() as u32).to_le_bytes());
            if version == 105 {
                out.extend_from_slice(&0u32.to_le_bytes());
                out.extend_from_slice(&0u64.to_le_bytes());
            } else {
                out.extend_from_slice(&0u32.to_le_bytes());
            }
        }
        for (name, files) in folders {
            out.push((name.len() + 1) as u8);
            out.extend_from_slice(name.as_bytes());
            out.push(0);
            out.extend(std::iter::repeat_n(0u8, files.len() * 16));
        }
        out.extend_from_slice(&file_names);
        out
    }

    fn paths_of(bytes: &[u8]) -> Result<Vec<String>, String> {
        let p = std::env::temp_dir().join(format!("lineage-bsa-{}-{}.bsa", std::process::id(), bytes.len()));
        std::fs::write(&p, bytes).unwrap();
        let mut out = Vec::new();
        let result = for_each_path(&p, &mut |s| out.push(s.to_string()));
        let _ = std::fs::remove_file(&p);
        result.map(|_| out)
    }

    #[test]
    fn lists_every_file_as_folder_and_name_in_both_versions() {
        let folders: &[(&str, &[&str])] = &[
            ("textures\\actors\\character\\overlays", &["Tattoo.dds", "freckles.dds"]),
            ("meshes/actors", &["head.nif"]),
        ];
        for version in [104, 105] {
            assert_eq!(
                paths_of(&build(version, folders)).unwrap(),
                [
                    "textures\\actors\\character\\overlays\\tattoo.dds",
                    "textures\\actors\\character\\overlays\\freckles.dds",
                    "meshes\\actors\\head.nif",
                ],
                "v{version}"
            );
        }
    }

    #[test]
    fn rejects_what_isnt_a_readable_bsa() {
        assert!(paths_of(b"BTDX\x01\0\0\0GNRL").is_err(), "a Fallout 4 BA2");
        let mut wrong_version = build(105, &[("textures", &["a.dds"])]);
        wrong_version[4] = 106;
        assert!(paths_of(&wrong_version).is_err());
        let full = build(105, &[("textures", &["a.dds", "b.dds"])]);
        assert!(paths_of(&full[..full.len() - 4]).is_err(), "truncated name table");
    }
}
