//! Changing the project's files: writing them safely, creating, renaming,
//! and moving to the Trash; and a file's line endings and when it last changed.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::SystemTime;

/// How a file's text is stored, to be written back the same way.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Encoding {
    #[default]
    Utf8,
    Utf8Bom,
    Utf16Le,
    Utf16Be,
    /// What's left: text that isn't UTF-8 is most likely this, a superset of Latin-1.
    Windows1252,
}

impl Encoding {
    pub fn name(self) -> &'static str {
        match self {
            Encoding::Utf8 => "UTF-8",
            Encoding::Utf8Bom => "UTF-8 with a BOM",
            Encoding::Utf16Le | Encoding::Utf16Be => "UTF-16",
            Encoding::Windows1252 => "Windows-1252",
        }
    }

    /// The text in `bytes`, and how it's stored. A file with NUL bytes that isn't UTF-16 is
    /// binary, not text, and an error: opened as text, saving it would ruin it.
    pub fn decode(bytes: &[u8]) -> std::io::Result<(String, Encoding)> {
        let units = |bytes: &[u8], make: fn([u8; 2]) -> u16| String::from_utf16_lossy(&bytes.chunks_exact(2).map(|b| make([b[0], b[1]])).collect::<Vec<u16>>());
        if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
            return Ok((units(rest, u16::from_le_bytes), Encoding::Utf16Le));
        }
        if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
            return Ok((units(rest, u16::from_be_bytes), Encoding::Utf16Be));
        }
        if bytes.contains(&0) {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "it's a binary file"));
        }
        if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF])
            && let Ok(text) = std::str::from_utf8(rest)
        {
            return Ok((text.to_string(), Encoding::Utf8Bom));
        }
        match std::str::from_utf8(bytes) {
            Ok(text) => Ok((text.to_string(), Encoding::Utf8)),
            Err(_) => Ok((encoding_rs::WINDOWS_1252.decode_without_bom_handling(bytes).0.into_owned(), Encoding::Windows1252)),
        }
    }

    /// `text` as it's stored; an error if there's something in it this encoding can't hold.
    pub fn encode(self, text: &str) -> Result<Vec<u8>, String> {
        let with = |bom: &[u8], rest: &[u8]| [bom, rest].concat();
        Ok(match self {
            Encoding::Utf8 => text.as_bytes().to_vec(),
            Encoding::Utf8Bom => with(&[0xEF, 0xBB, 0xBF], text.as_bytes()),
            Encoding::Utf16Le => with(&[0xFF, 0xFE], &text.encode_utf16().flat_map(u16::to_le_bytes).collect::<Vec<u8>>()),
            Encoding::Utf16Be => with(&[0xFE, 0xFF], &text.encode_utf16().flat_map(u16::to_be_bytes).collect::<Vec<u8>>()),
            Encoding::Windows1252 => {
                let (bytes, _, lossy) = encoding_rs::WINDOWS_1252.encode(text);
                if lossy {
                    return Err("there's a character in it Windows-1252 can't hold, so it wasn't written".into());
                }
                bytes.into_owned()
            }
        })
    }
}

/// The text of the file at `path`, however it's stored, and how.
pub fn read_text(path: &Path) -> std::io::Result<(String, Encoding)> {
    Encoding::decode(&std::fs::read(path)?)
}

/// Write `bytes` to `path` so it's never half-written: into a file beside
/// it, flushed to the disk, then renamed over it. A symlink is written
/// through, and an existing file keeps its permissions. Where a file can't
/// be made beside it (a folder we can't write to), it's written in place.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let target = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let dir = target.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let name = target.file_name().map_or_else(Default::default, |n| n.to_string_lossy().into_owned());
    // Its own, so that saves at the same moment, from any thread, can't write into each other's.
    static WRITES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let write = WRITES.fetch_add(1, Ordering::Relaxed);
    let temp = dir.join(format!(".{name}.ghostwrite-{}-{write}.tmp", std::process::id()));
    // Can't make a file beside it: write it where it is.
    let Ok(mut file) = std::fs::File::create(&temp) else { return std::fs::write(&target, bytes) };
    let written = (|| {
        file.write_all(bytes)?;
        if let Ok(meta) = std::fs::metadata(&target) {
            file.set_permissions(meta.permissions())?;
        }
        file.sync_all()?;
        std::fs::rename(&temp, &target)
    })();
    if written.is_err() {
        std::fs::remove_file(&temp).ok();
    }
    written
}

/// A file as it was when last read or written: when it changed, and how
/// big it was. If the disk's differs, another program changed it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stamp {
    modified: Option<SystemTime>,
    len: u64,
}

impl Stamp {
    /// `path`'s now; `None` if there's no file there.
    pub fn of(path: &Path) -> Option<Stamp> {
        let meta = std::fs::metadata(path).ok()?;
        Some(Stamp { modified: meta.modified().ok(), len: meta.len() })
    }
}

/// `text` with `\n` line endings, and whether it had `\r\n` ones (most of
/// its lines did): what to write back.
pub fn normalize(text: String) -> (String, bool) {
    let crlf = text.matches("\r\n").count();
    if crlf == 0 || crlf * 2 < text.matches('\n').count() {
        return (text, false);
    }
    (text.replace("\r\n", "\n"), true)
}

/// `text` as it's written: with `\r\n` line endings if the file had them.
pub fn denormalize(text: &str, crlf: bool) -> std::borrow::Cow<'_, str> {
    if crlf { text.replace('\n', "\r\n").into() } else { text.into() }
}

/// Create `name` inside `dir`: a folder when it ends in `/`, else an empty
/// file. Missing folders on the way are made; an existing entry is an error.
pub fn create(dir: &Path, name: &str) -> Result<PathBuf, String> {
    let name = name.trim();
    check_name(name)?;
    let path = dir.join(name.trim_end_matches('/'));
    if path.exists() {
        return Err(format!("{} already exists", path.display()));
    }
    let made = if name.ends_with('/') {
        std::fs::create_dir_all(&path)
    } else {
        path.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|()| std::fs::OpenOptions::new().write(true).create_new(true).open(&path).map(drop))
    };
    made.map(|()| path).map_err(|e| format!("couldn't create {name}: {e}"))
}

/// Rename `from` to `name` within its folder (or below it, if `name` has `/`).
pub fn rename(from: &Path, name: &str) -> Result<PathBuf, String> {
    let name = name.trim().trim_end_matches('/');
    check_name(name)?;
    move_to(from, &from.parent().unwrap_or(from).join(name))
}

/// Move `from` to `to`, making the folders on the way; an existing `to` is an error.
pub fn move_to(from: &Path, to: &Path) -> Result<PathBuf, String> {
    if to == from {
        return Ok(to.to_path_buf());
    }
    // `a.txt` → `A.txt` on a disk that ignores case: it "exists", as itself.
    let case_only = to.parent() == from.parent() && crate::same_file(from, to);
    if to.exists() && !case_only {
        return Err(format!("{} already exists", to.display()));
    }
    to.parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::rename(from, to))
        .map(|()| to.to_path_buf())
        .map_err(|e| format!("couldn't move {}: {e}", from.display()))
}

static DELETE_INSTEAD: AtomicBool = AtomicBool::new(false);

/// From now on, [`trash`] deletes outright instead of filling the real Trash (tests).
pub fn delete_instead_of_trash() {
    DELETE_INSTEAD.store(true, Ordering::Relaxed);
}

/// Move `path` to the Trash.
pub fn trash(path: &Path) -> Result<(), String> {
    let moved = if DELETE_INSTEAD.load(Ordering::Relaxed) || cfg!(test) {
        if path.is_dir() { std::fs::remove_dir_all(path) } else { std::fs::remove_file(path) }.map_err(|e| e.to_string())
    } else {
        ::trash::delete(path).map_err(|e| e.to_string())
    };
    moved.map_err(|e| format!("couldn't move {} to the Trash: {e}", path.display()))
}

fn check_name(name: &str) -> Result<(), String> {
    if name.is_empty() || name.starts_with('/') || name.split('/').any(|part| part == "..") {
        return Err("give a name in the folder: not starting with `/`, without `..`".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_goes_back_as_it_came() {
        for (bytes, text, encoding) in [
            (b"plain\n".to_vec(), "plain\n", Encoding::Utf8),
            ("bl\u{e5}\n".as_bytes().to_vec(), "bl\u{e5}\n", Encoding::Utf8),
            ([&[0xEF, 0xBB, 0xBF], b"bom\n".as_slice()].concat(), "bom\n", Encoding::Utf8Bom),
            (vec![0xFF, 0xFE, b'h', 0, b'i', 0], "hi", Encoding::Utf16Le),
            (vec![0xFE, 0xFF, 0, b'h', 0, b'i'], "hi", Encoding::Utf16Be),
            (vec![b'b', b'l', 0xE5, b'\n'], "bl\u{e5}\n", Encoding::Windows1252),
        ] {
            let (read, found) = Encoding::decode(&bytes).unwrap();
            assert_eq!((read.as_str(), found), (text, encoding));
            assert_eq!(found.encode(&read).unwrap(), bytes, "{encoding:?}");
        }
        assert!(Encoding::decode(&[b'a', 0, b'b']).is_err(), "binary isn't text");
        assert!(Encoding::Windows1252.encode("\u{2192}").is_err(), "not written with something lost");
    }

    #[test]
    fn saves_at_the_same_moment_leave_one_of_them_whole() {
        let path = crate::scratch_dir("atomic-race").join("file");
        let texts: Vec<String> = (0..8).map(|i| i.to_string().repeat(200_000)).collect();
        std::thread::scope(|threads| {
            for text in &texts {
                let path = &path;
                threads.spawn(move || {
                    for _ in 0..5 {
                        write_atomic(path, text.as_bytes()).unwrap();
                    }
                });
            }
        });
        assert!(texts.contains(&std::fs::read_to_string(&path).unwrap()));
        let left = std::fs::read_dir(path.parent().unwrap()).unwrap().count();
        assert_eq!(left, 1, "and no temporary files");
    }

    #[cfg(unix)]
    #[test]
    fn writes_whole_files_through_symlinks_keeping_permissions() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = crate::scratch_dir("atomic");
        let real = dir.join("real.sh");
        std::fs::write(&real, "old").unwrap();
        std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o755)).unwrap();
        let link = dir.join("link.sh");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        write_atomic(&link, b"new").unwrap();
        assert!(link.is_symlink(), "the link stays a link");
        assert_eq!(std::fs::read_to_string(&real).unwrap(), "new");
        assert_eq!(std::fs::metadata(&real).unwrap().permissions().mode() & 0o777, 0o755);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 2, "no temporary file left");
        write_atomic(&dir.join("fresh.txt"), b"x").unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("fresh.txt")).unwrap(), "x");
    }

    #[test]
    fn line_endings_round_trip() {
        assert_eq!(normalize("a\r\nb\r\n".into()), ("a\nb\n".into(), true));
        assert_eq!(normalize("a\nb\r\nc\n".into()), ("a\nb\r\nc\n".into(), false), "mostly \\n: left as it is");
        assert_eq!(denormalize("a\nb\n", true), "a\r\nb\r\n");
    }

    #[test]
    fn a_rename_can_change_only_the_case() {
        let dir = crate::scratch_dir("case");
        let from = create(&dir, "readme.md").unwrap();
        let to = rename(&from, "README.md").unwrap();
        assert_eq!(std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name()).collect::<Vec<_>>(), ["README.md"]);
        assert!(rename(&to, "other.md").is_ok());
        create(&dir, "taken.md").unwrap();
        assert!(rename(&dir.join("other.md"), "taken.md").is_err(), "another file is still in the way");
    }

    #[test]
    fn creates_and_renames_entries() {
        let dir = crate::scratch_dir("ops");
        let file = create(&dir, "src/lib.rs").unwrap();
        assert!(file.is_file());
        assert!(create(&dir, "docs/").unwrap().is_dir());
        assert!(create(&dir, "src/lib.rs").is_err());
        assert!(create(&dir, "../escape").is_err());
        let moved = rename(&file, "main.rs").unwrap();
        assert_eq!(moved, dir.join("src/main.rs"));
        assert!(moved.is_file() && !file.exists());
        assert!(rename(&moved, "../../x").is_err());
        trash(&moved).unwrap();
        assert!(!moved.exists());
    }
}
