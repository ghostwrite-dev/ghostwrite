//! `:plugin install`, `:plugin update` and `:plugin remove`: plugins in the plugins
//! folder, each in a folder of its own, from a folder here or a git repository with
//! its built `.wasm` in it. What's fetched is checked before it's put in place, and
//! what's there is only replaced by what's been checked, so failing leaves things
//! as they were. Nothing updates unless it's asked to.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::manifest::{self, Manifest};

/// Beside what's installed: where it came from, and which commit that was.
const SOURCE: &str = ".source";

/// Somewhere to fetch to, gone when it's dropped unless it's been moved.
struct Incoming(PathBuf);

impl Incoming {
    fn new(folder: &Path) -> Result<Self, String> {
        static N: AtomicUsize = AtomicUsize::new(0);
        std::fs::create_dir_all(folder).map_err(|e| format!("couldn't make {}: {e}", folder.display()))?;
        let name = format!(".incoming-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed));
        Ok(Self(folder.join(name)))
    }
}

impl Drop for Incoming {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// What `source` says to clone, if it's not a folder: a URL, or GitHub's `owner/repo`.
fn repository(source: &str) -> Result<String, String> {
    if ["https://", "ssh://", "git://", "file://", "git@"].iter().any(|start| source.starts_with(start)) {
        return Ok(source.to_string());
    }
    let parts: Vec<&str> = source.split('/').collect();
    if let [owner, repo] = parts[..]
        && manifest::proper_name(owner)
        && repo.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
        && !repo.is_empty()
        && !repo.starts_with('.')
    {
        return Ok(format!("https://github.com/{source}"));
    }
    Err(format!("{source} isn't a folder or a git repository"))
}

fn copy_folder(from: &Path, to: &Path) -> Result<(), String> {
    std::fs::create_dir_all(to).map_err(|e| e.to_string())?;
    for entry in std::fs::read_dir(from).map_err(|e| e.to_string())?.flatten() {
        let name = entry.file_name();
        let kind = entry.file_type().map_err(|e| e.to_string())?;
        // What a build leaves, and links that could lead anywhere, aren't the plugin.
        if name == ".git" || name == "target" || kind.is_symlink() {
            continue;
        }
        let target = to.join(&name);
        if kind.is_dir() {
            copy_folder(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), &target).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

/// Clone `url` to `to`, leaving no git behind: what commit it was, for the record.
fn clone(url: &str, to: &Path) -> Result<String, String> {
    let git = |args: &[&std::ffi::OsStr]| {
        let mut command = ghostwrite_project::platform::command("git");
        // Only what fetches files, and it can't stop to ask for anything.
        command.env("GIT_ALLOW_PROTOCOL", "https:ssh:git:file").env("GIT_TERMINAL_PROMPT", "0").args(args);
        command.output().map_err(|e| format!("couldn't run git: {e}"))
    };
    let cloned = git(&["clone".as_ref(), "--depth".as_ref(), "1".as_ref(), "--".as_ref(), url.as_ref(), to.as_os_str()])?;
    if !cloned.status.success() {
        let why = String::from_utf8_lossy(&cloned.stderr);
        return Err(format!("couldn't clone {url}: {}", why.lines().last().unwrap_or("git says nothing").trim()));
    }
    let head = git(&["-C".as_ref(), to.as_os_str(), "rev-parse".as_ref(), "HEAD".as_ref()])?;
    let commit = String::from_utf8_lossy(&head.stdout).trim().to_string();
    let _ = std::fs::remove_dir_all(to.join(".git"));
    Ok(commit)
}

/// `source` fetched and checked, ready to be put in place.
fn fetch(source: &str, folder: &Path) -> Result<(Incoming, Manifest), String> {
    let incoming = Incoming::new(folder)?;
    let local = ghostwrite_project::resolve(source, Path::new("."));
    let (source, commit) = if local.is_dir() {
        let local = local.canonicalize().map_err(|e| e.to_string())?;
        copy_folder(&local, &incoming.0).map_err(|e| format!("couldn't copy it: {e}"))?;
        (local.display().to_string(), String::new())
    } else {
        let commit = clone(&repository(source)?, &incoming.0)?;
        (source.to_string(), commit)
    };
    let manifest = Manifest::in_own_folder(&incoming.0)?;
    manifest.check()?;
    std::fs::write(incoming.0.join(SOURCE), format!("{source}\n{commit}\n")).map_err(|e| e.to_string())?;
    Ok((incoming, manifest))
}

/// The installed plugin called `name`, read from where it's now been put.
fn installed(name: &str, folder: &Path) -> Result<Manifest, String> {
    Manifest::in_own_folder(&folder.join(name))
}

/// Install from `source` (a folder, a git URL, or GitHub's `owner/repo`).
pub fn install(source: &str, folder: &Path) -> Result<Manifest, String> {
    let (incoming, manifest) = fetch(source, folder)?;
    let name = manifest.name;
    if folder.join(name).exists() || folder.join(format!("{name}.json")).exists() {
        return Err(format!("{name} is already installed: :plugin update {name}, or :plugin remove {name}"));
    }
    std::fs::rename(&incoming.0, folder.join(name)).map_err(|e| format!("couldn't put it in place: {e}"))?;
    installed(name, folder)
}

/// Fetch `name` again from where it came from, and replace it.
pub fn update(name: &str, folder: &Path) -> Result<Manifest, String> {
    if !manifest::proper_name(name) {
        return Err(format!("no plugin {name}"));
    }
    let recorded = std::fs::read_to_string(folder.join(name).join(SOURCE)).map_err(|_| format!("{name} wasn't installed with :plugin install"))?;
    let source = recorded.lines().next().unwrap_or_default();
    let (incoming, manifest) = fetch(source, folder)?;
    if manifest.name != name {
        return Err(format!("{source} is now called {}: :plugin remove {name}, then install it", manifest.name));
    }
    let old = Incoming::new(folder)?;
    std::fs::rename(folder.join(name), &old.0).map_err(|e| format!("couldn't move the old one: {e}"))?;
    if let Err(e) = std::fs::rename(&incoming.0, folder.join(name)) {
        // Put back as it was.
        let _ = std::fs::rename(&old.0, folder.join(name));
        return Err(format!("couldn't put it in place: {e}"));
    }
    installed(name, folder)
}

/// The plugins that [`update`] can fetch again.
pub fn updatable(folder: &Path) -> Vec<String> {
    let found = std::fs::read_dir(folder).into_iter().flatten().flatten();
    let mut names: Vec<String> =
        found.filter(|entry| entry.path().join(SOURCE).is_file()).map(|entry| entry.file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    names
}

/// Take `name` out of the plugins folder.
pub fn remove(name: &str, folder: &Path) -> Result<(), String> {
    if !manifest::proper_name(name) {
        return Err(format!("no plugin {name}"));
    }
    let (own, file) = (folder.join(name), folder.join(format!("{name}.json")));
    if own.is_dir() {
        std::fs::remove_dir_all(own).map_err(|e| format!("couldn't remove it: {e}"))
    } else if file.is_file() {
        std::fs::remove_file(file).map_err(|e| format!("couldn't remove it: {e}"))
    } else {
        Err(format!("no plugin {name} in {}", folder.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Plugin as _;

    fn plugin(version: &str) -> PathBuf {
        let source = ghostwrite_project::scratch_dir("plugin-source");
        std::fs::write(source.join(manifest::FILE), format!(r#"{{ "name": "shelf", "version": "{version}", "extensions": ["shelf"] }}"#)).unwrap();
        std::fs::create_dir(source.join("target")).unwrap();
        std::fs::write(source.join("target/big"), "what a build leaves").unwrap();
        source
    }

    fn at(path: &Path) -> String {
        path.display().to_string()
    }

    #[test]
    fn a_plugin_is_installed_updated_and_removed_from_a_folder() {
        let (source, folder) = (plugin("1"), ghostwrite_project::scratch_dir("plugins"));
        let first = install(&at(&source), &folder).unwrap();
        assert_eq!((first.name, first.version), ("shelf", Some("1")));
        assert!(!folder.join("shelf/target").exists(), "what a build leaves stays behind");
        assert!(install(&at(&source), &folder).err().unwrap_or_default().contains("already installed"));

        std::fs::write(source.join(manifest::FILE), r#"{ "name": "shelf", "version": "2" }"#).unwrap();
        assert_eq!(installed("shelf", &folder).unwrap().version, Some("1"), "nothing changes until it's asked to");
        assert_eq!(update("shelf", &folder).unwrap().version, Some("2"));

        std::fs::write(source.join(manifest::FILE), "not json").unwrap();
        assert!(update("shelf", &folder).is_err());
        assert_eq!(installed("shelf", &folder).unwrap().version, Some("2"), "a broken update leaves it as it was");

        std::fs::write(source.join(manifest::FILE), r#"{ "name": "renamed" }"#).unwrap();
        assert!(update("shelf", &folder).err().unwrap_or_default().contains("now called renamed"));
        assert_eq!(updatable(&folder), ["shelf"]);
        assert_eq!(installed("shelf", &folder).unwrap().version, Some("2"));

        remove("shelf", &folder).unwrap();
        assert!(!folder.join("shelf").exists());
        assert!(remove("shelf", &folder).is_err());
        let left: Vec<_> = std::fs::read_dir(&folder).unwrap().flatten().map(|entry| entry.file_name()).collect();
        assert!(left.is_empty(), "nothing's left lying about: {left:?}");
    }

    #[test]
    fn a_plugin_is_installed_from_git_without_its_history() {
        let (source, folder) = (plugin("1"), ghostwrite_project::scratch_dir("plugins"));
        let git = |args: &[&str]| std::process::Command::new("git").arg("-C").arg(&source).args(args).output().map(|out| out.status.success()).unwrap_or(false);
        if !git(&["init", "-q"]) {
            return eprintln!("no git: skipped");
        }
        assert!(git(&["add", manifest::FILE]) && git(&["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-qm", "first"]));
        let url = format!("file://{}", at(&source.canonicalize().unwrap()));
        assert_eq!(install(&url, &folder).unwrap().version, Some("1"));
        assert!(!folder.join("shelf/.git").exists());
        let recorded = std::fs::read_to_string(folder.join("shelf").join(SOURCE)).unwrap();
        assert_eq!(recorded.lines().next(), Some(url.as_str()));
        assert_eq!(recorded.lines().nth(1).map(str::len), Some(40), "which commit: {recorded}");
        assert!(install("file:///nowhere/at/all", &folder).err().unwrap_or_default().contains("couldn't clone"));
    }

    #[test]
    fn what_isnt_a_source_or_a_name_goes_nowhere() {
        let folder = ghostwrite_project::scratch_dir("plugins");
        assert_eq!(repository("felix/words").as_deref(), Ok("https://github.com/felix/words"));
        assert!(repository("https://example.com/x.git").is_ok());
        for wrong in ["words", "../../x", "ext::sh -c x", "--upload-pack=x", "a/b/c", "a/.."] {
            assert!(repository(wrong).is_err(), "{wrong}");
        }
        for wrong in ["..", "../x", "a/b", ""] {
            assert!(remove(wrong, &folder).is_err() && update(wrong, &folder).is_err(), "{wrong:?}");
        }
    }

    #[test]
    fn it_says_all_it_can_do() {
        let (source, folder) = (plugin("3"), ghostwrite_project::scratch_dir("plugins"));
        let described = install(&at(&source), &folder).unwrap().describe();
        assert!(described.contains("version 3") && described.contains(".shelf"), "{described}");
    }
}
