//! Where things go, and how programs are run, on each system: the one place that
//! knows, so that nothing else has to.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Where the settings are, and what's beside them: the config, themes and plugins.
pub fn config_dir() -> Option<PathBuf> {
    if cfg!(windows) {
        return std::env::var_os("APPDATA").map(|dir| PathBuf::from(dir).join("Ghostwrite"));
    }
    Some(
        std::env::var_os("XDG_CONFIG_HOME")
            .filter(|_| !cfg!(target_os = "macos"))
            .map_or_else(|| std::env::home_dir().map(|home| home.join(".config")), |dir| Some(dir.into()))?
            .join("ghostwrite"),
    )
}

/// Where what's remembered between launches is kept.
pub fn data_dir() -> Option<PathBuf> {
    if cfg!(windows) {
        return std::env::var_os("APPDATA").map(|dir| PathBuf::from(dir).join("Ghostwrite"));
    }
    if cfg!(target_os = "macos") {
        return Some(std::env::home_dir()?.join("Library/Application Support/Ghostwrite"));
    }
    Some(
        std::env::var_os("XDG_DATA_HOME")
            .map_or_else(|| std::env::home_dir().map(|home| home.join(".local/share")), |dir| Some(dir.into()))?
            .join("ghostwrite"),
    )
}

/// Where what can be fetched again is kept: downloaded models.
pub fn cache_dir() -> Option<PathBuf> {
    if cfg!(windows) {
        return std::env::var_os("LOCALAPPDATA").map(|dir| PathBuf::from(dir).join("Ghostwrite"));
    }
    if cfg!(target_os = "macos") {
        return Some(std::env::home_dir()?.join("Library/Caches/ghostwrite"));
    }
    Some(std::env::var_os("XDG_CACHE_HOME").map_or_else(|| std::env::home_dir().map(|home| home.join(".cache")), |dir| Some(dir.into()))?.join("ghostwrite"))
}

/// `program`, ready to run in the background: on Windows a console program opens
/// a window of its own unless it's told not to.
pub fn command(program: impl AsRef<std::ffi::OsStr>) -> Command {
    #[allow(unused_mut)]
    let mut command = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

/// The path with symlinks followed. On Windows that comes back in a form (`\\?\C:\…`)
/// that most programs can't read and that doesn't compare equal to a path as it's typed.
pub fn real(path: &Path) -> std::io::Result<PathBuf> {
    Ok(without_verbatim(path.canonicalize()?))
}

fn without_verbatim(path: PathBuf) -> PathBuf {
    match path.to_str().and_then(|text| text.strip_prefix(r"\\?\")) {
        // `\\?\UNC\server\share` is a network path, and stays as it is.
        Some(rest) if !rest.starts_with("UNC\\") => PathBuf::from(rest),
        _ => path,
    }
}

/// A path relative to a project as it's written to a model and to git: with `/`,
/// on every system, so that what's said about a file doesn't depend on where it's said.
pub fn with_slashes(path: &Path) -> String {
    let text = path.to_string_lossy();
    if cfg!(windows) { text.replace('\\', "/") } else { text.into_owned() }
}

/// Look `name` up on the PATH: the way a shell would, on Windows with its `.exe`, `.cmd` and the like.
pub fn on_path(name: &str, path: &std::ffi::OsStr) -> Option<PathBuf> {
    let endings: Vec<String> = if cfg!(windows) {
        let known = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into());
        std::iter::once(String::new()).chain(known.split(';').map(str::to_string)).collect()
    } else {
        vec![String::new()]
    };
    std::env::split_paths(path).find_map(|dir| endings.iter().map(|ending| dir.join(format!("{name}{ending}"))).find(|file| file.is_file()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_verbatim_path_is_written_the_ordinary_way() {
        assert_eq!(without_verbatim(PathBuf::from(r"\\?\C:\work\a.rs")), PathBuf::from(r"C:\work\a.rs"));
        assert_eq!(without_verbatim(PathBuf::from(r"\\?\UNC\server\share")), PathBuf::from(r"\\?\UNC\server\share"), "a network path has to stay");
        assert_eq!(without_verbatim(PathBuf::from("/usr/bin")), PathBuf::from("/usr/bin"));
    }

    #[test]
    fn the_folders_are_for_ghostwrite() {
        for dir in [config_dir(), data_dir(), cache_dir()] {
            let dir = dir.expect("there's a home");
            assert!(dir.to_string_lossy().to_lowercase().contains("ghostwrite"), "{dir:?}");
        }
    }

    #[test]
    fn a_program_is_found_on_the_path() {
        let dir = crate::scratch_dir("on-path");
        std::fs::write(dir.join("mine"), "").unwrap();
        assert_eq!(on_path("mine", dir.as_os_str()), Some(dir.join("mine")));
        assert_eq!(on_path("not-there", dir.as_os_str()), None);
    }
}
