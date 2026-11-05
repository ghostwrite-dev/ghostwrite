//! A plugin's zip, unzipped: its files (stored or deflated), each checked against its
//! CRC, and only into the folder it's unzipped in. No links, nothing above the folder,
//! and no more than a plugin could need.

use std::io::Read as _;
use std::path::{Component, Path};

/// The most a plugin unzips to.
const MOST: u64 = 1 << 30;

fn u16_at(bytes: &[u8], at: usize) -> Result<usize, String> {
    bytes.get(at..at + 2).map(|b| u16::from_le_bytes([b[0], b[1]]) as usize).ok_or_else(|| "it's cut short".to_string())
}

fn u32_at(bytes: &[u8], at: usize) -> Result<usize, String> {
    bytes.get(at..at + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize).ok_or_else(|| "it's cut short".to_string())
}

/// Unzip `zip` into the folder `to`, which it makes.
pub fn unzip(zip: &[u8], to: &Path) -> Result<(), String> {
    let wrong = |why: &str| format!("it isn't a zip that can be unzipped: {why}");
    // The end of the central directory: its last 22 bytes, unless there's a comment.
    let end =
        (0..zip.len().saturating_sub(21)).rev().take(1 << 16).find(|&at| zip[at..].starts_with(&[0x50, 0x4b, 0x05, 0x06])).ok_or_else(|| wrong("no end"))?;
    let (count, mut at) = (u16_at(zip, end + 10)?, u32_at(zip, end + 16)?);
    if count == 0xffff || at == 0xffff_ffff {
        return Err(wrong("it's zip64"));
    }
    std::fs::create_dir_all(to).map_err(|e| e.to_string())?;
    let mut total = 0u64;
    for _ in 0..count {
        if !zip.get(at..).is_some_and(|rest| rest.starts_with(&[0x50, 0x4b, 0x01, 0x02])) {
            return Err(wrong("its directory is broken"));
        }
        let (flags, method, crc) = (u16_at(zip, at + 8)?, u16_at(zip, at + 10)?, u32_at(zip, at + 16)? as u32);
        let (packed, size) = (u32_at(zip, at + 20)?, u32_at(zip, at + 24)?);
        let (name_len, extra_len, comment_len) = (u16_at(zip, at + 28)?, u16_at(zip, at + 30)?, u16_at(zip, at + 32)?);
        let (mode, local) = (u32_at(zip, at + 38)? >> 16, u32_at(zip, at + 42)?);
        let name = zip.get(at + 46..at + 46 + name_len).ok_or_else(|| wrong("a name is cut short"))?;
        let name = std::str::from_utf8(name).map_err(|_| wrong("a name isn't UTF-8"))?;
        at += 46 + name_len + extra_len + comment_len;

        let path = Path::new(name.trim_end_matches('/'));
        if name.contains('\\') || path.components().any(|part| !matches!(part, Component::Normal(_))) {
            return Err(format!("{name} isn't inside the plugin's folder"));
        }
        if mode & 0o170000 == 0o120000 {
            return Err(format!("{name} is a link, which a plugin can't have"));
        }
        if flags & 1 != 0 {
            return Err(format!("{name} is encrypted"));
        }
        if name.ends_with('/') {
            std::fs::create_dir_all(to.join(path)).map_err(|e| e.to_string())?;
            continue;
        }
        total += size as u64;
        if total > MOST {
            return Err(wrong("it's bigger than a plugin could be"));
        }
        if !zip.get(local..).is_some_and(|rest| rest.starts_with(&[0x50, 0x4b, 0x03, 0x04])) {
            return Err(wrong("a file is missing"));
        }
        let data = local + 30 + u16_at(zip, local + 26)? + u16_at(zip, local + 28)?;
        let data = zip.get(data..data + packed).ok_or_else(|| wrong("a file is cut short"))?;
        let bytes = match method {
            0 => data.to_vec(),
            8 => {
                let mut out = Vec::with_capacity(size);
                flate2::read::DeflateDecoder::new(data).take(size as u64 + 1).read_to_end(&mut out).map_err(|e| format!("{name}: {e}"))?;
                out
            }
            _ => return Err(format!("{name} is packed in a way this can't unpack ({method})")),
        };
        if bytes.len() != size || crc32fast::hash(&bytes) != crc {
            return Err(format!("{name} isn't what the zip says it is"));
        }
        let file = to.join(path);
        if let Some(parent) = file.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::write(&file, bytes).map_err(|e| format!("couldn't write {name}: {e}"))?;
    }
    Ok(())
}

/// A zip of `files` (a name ending in `/` is a folder), stored as they are: for tests.
#[doc(hidden)]
pub fn zip(files: &[(&str, &[u8])]) -> Vec<u8> {
    let (mut out, mut directory) = (vec![], vec![]);
    for (name, bytes) in files {
        let (crc, offset) = (crc32fast::hash(bytes), out.len() as u32);
        let common = |out: &mut Vec<u8>| {
            out.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 0]); // flags, method, time, date
            out.extend_from_slice(&crc.to_le_bytes());
            out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
            out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
            out.extend_from_slice(&(name.len() as u16).to_le_bytes());
            out.extend_from_slice(&[0, 0]);
        };
        out.extend_from_slice(&[0x50, 0x4b, 0x03, 0x04, 20, 0]);
        common(&mut out);
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(bytes);
        directory.extend_from_slice(&[0x50, 0x4b, 0x01, 0x02, 20, 3, 20, 0]);
        common(&mut directory);
        directory.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 0, 0, 0]); // comment, disk, internal, external
        directory.extend_from_slice(&offset.to_le_bytes());
        directory.extend_from_slice(name.as_bytes());
    }
    let start = out.len() as u32;
    out.extend_from_slice(&directory);
    out.extend_from_slice(&[0x50, 0x4b, 0x05, 0x06, 0, 0, 0, 0]);
    out.extend_from_slice(&(files.len() as u16).to_le_bytes());
    out.extend_from_slice(&(files.len() as u16).to_le_bytes());
    out.extend_from_slice(&(directory.len() as u32).to_le_bytes());
    out.extend_from_slice(&start.to_le_bytes());
    out.extend_from_slice(&[0, 0]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_zip_is_unzipped_and_nothing_escapes_it() {
        let to = ghostwrite_project::scratch_dir("unzip");
        unzip(&zip(&[("shelf/", b""), ("shelf/plugin.json", b"{}"), ("shelf/sub/a.txt", b"a")]), &to.join("one")).unwrap();
        assert_eq!(std::fs::read(to.join("one/shelf/sub/a.txt")).unwrap(), b"a");
        for name in ["../out", "/etc/out", "a/../../out", "a\\..\\out"] {
            assert!(unzip(&zip(&[(name, b"x")]), &to.join("two")).is_err(), "{name}");
        }
        assert!(!to.join("out").exists());
        let mut broken = zip(&[("a", b"abc")]);
        broken[30 + 1] = b'X';
        assert!(unzip(&broken, &to.join("three")).unwrap_err().contains("isn't what the zip says"));
        assert!(unzip(b"not a zip at all, not even close", &to.join("four")).is_err());
    }

    #[test]
    fn a_deflated_zip_is_unzipped() {
        // What `zip` makes of what it's given to read: a deflated `-`.
        let made =
            std::process::Command::new("zip").args(["-q", "-fz-", "-", "-"]).stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).spawn();
        let Ok(mut child) = made else { return eprintln!("no zip: skipped") };
        use std::io::Write as _;
        child.stdin.take().unwrap().write_all(&b"hello ".repeat(100)).unwrap();
        let zipped = child.wait_with_output().unwrap().stdout;
        let to = ghostwrite_project::scratch_dir("unzip");
        unzip(&zipped, &to).unwrap();
        assert_eq!(std::fs::read(to.join("-")).unwrap(), b"hello ".repeat(100));
    }
}
