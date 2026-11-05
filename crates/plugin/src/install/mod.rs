//! `:plugin install`, `:plugin update`, `:plugin link` and `:plugin remove`: plugins in
//! the plugins folder, each in a folder of its own. One comes from a folder here, a
//! GitHub release ([`github`]: `owner/repo`, `owner/repo@1.2.0`, `owner/repo/name`), or
//! a git repository with its built `.wasm` in it (a repository with no release is
//! cloned). What's fetched is checked before it's put in place, and what's there is
//! only replaced by what's been checked, so failing leaves things as they were. Nothing
//! updates unless it's asked to, except a linked folder, which is read where it is and
//! read again when it changes.

pub mod github;
pub mod unzip;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::SystemTime;

pub use github::Hosts;

use crate::manifest::{self, Manifest};

/// Beside what's installed: where it came from, and which commit or release that was.
const SOURCE: &str = ".source";
/// What a linked folder is known by in the plugins folder: `<name>.link`, holding where it is.
pub const LINK: &str = "link";

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

/// The plugin a release's zip unzips to: its one folder, or all of it.
fn unzipped(release: &github::Release, to: &Path, folder: &Path) -> Result<(), String> {
    let scratch = Incoming::new(folder)?;
    unzip::unzip(&release.bytes, &scratch.0).map_err(|e| format!("{}: {e}", release.zip))?;
    let top: Vec<PathBuf> = std::fs::read_dir(&scratch.0).map_err(|e| e.to_string())?.flatten().map(|entry| entry.path()).collect();
    let plugin = match &top[..] {
        [only] if only.is_dir() => only.clone(),
        _ => scratch.0.clone(),
    };
    std::fs::rename(plugin, to).map_err(|e| format!("couldn't unzip it: {e}"))
}

/// A plugin fetched and checked, not yet put in place: what it can do is told before it is.
pub struct Staged {
    incoming: Incoming,
    pub manifest: Manifest,
    /// Why it couldn't be checked against a checksum, if it couldn't.
    pub unchecked: Option<String>,
}

impl Staged {
    /// Put it in place, unless one by its name is there already.
    pub fn install(self, folder: &Path) -> Result<Manifest, String> {
        self.installable(folder)?;
        let name = self.manifest.name;
        std::fs::rename(&self.incoming.0, folder.join(name)).map_err(|e| format!("couldn't put it in place: {e}"))?;
        installed(name, folder)
    }

    /// Whether it can be installed, or why not: one by its name is there already.
    pub fn installable(&self, folder: &Path) -> Result<(), String> {
        let name = self.manifest.name;
        if is_installed(name, folder) {
            return Err(format!("{name} is already installed: :plugin update {name}, or :plugin remove {name}"));
        }
        Ok(())
    }

    /// Put it in place of the one installed by its name ([`fetch_update`]).
    pub fn replace(self, folder: &Path) -> Result<Manifest, String> {
        let name = self.manifest.name;
        let old = Incoming::new(folder)?;
        std::fs::rename(folder.join(name), &old.0).map_err(|e| format!("couldn't move the old one: {e}"))?;
        if let Err(e) = std::fs::rename(&self.incoming.0, folder.join(name)) {
            // Put back as it was.
            let _ = std::fs::rename(&old.0, folder.join(name));
            return Err(format!("couldn't put it in place: {e}"));
        }
        installed(name, folder)
    }
}

fn is_installed(name: &str, folder: &Path) -> bool {
    folder.join(name).exists() || folder.join(format!("{name}.json")).exists() || folder.join(format!("{name}.{LINK}")).exists()
}

/// `source` fetched and checked, ready to be put in place: [`Staged::install`].
pub fn fetch(source: &str, folder: &Path) -> Result<Staged, String> {
    fetch_from(source, folder, &Hosts::default())
}

/// [`fetch`], with GitHub where `hosts` says.
pub fn fetch_from(source: &str, folder: &Path, hosts: &Hosts) -> Result<Staged, String> {
    let incoming = Incoming::new(folder)?;
    let local = ghostwrite_project::resolve(source, Path::new("."));
    let mut unchecked = None;
    let (source, commit) = if local.is_dir() {
        let local = local.canonicalize().map_err(|e| e.to_string())?;
        copy_folder(&local, &incoming.0).map_err(|e| format!("couldn't copy it: {e}"))?;
        (local.display().to_string(), String::new())
    } else if let Some(repo) = github::Repo::parse(source) {
        match github::fetch(&repo, hosts)? {
            Some(release) => {
                unzipped(&release, &incoming.0, folder)?;
                if !release.checked {
                    unchecked = Some(format!("{} has no checksum beside it to check it against", release.zip));
                }
                (repo.followed(), release.tag)
            }
            None if repo.version.is_some() || repo.name.is_some() => {
                let which = repo.version.as_deref().map_or_else(|| "release".to_string(), |version| format!("release {version}"));
                return Err(format!("{}/{} has no {which}", repo.owner, repo.repo));
            }
            // No release: what's in the repository, as it is.
            None => {
                let commit = clone(&format!("{}/{}/{}", hosts.web, repo.owner, repo.repo), &incoming.0)?;
                (repo.followed(), commit)
            }
        }
    } else {
        let commit = clone(&repository(source)?, &incoming.0)?;
        (source.to_string(), commit)
    };
    let manifest = Manifest::in_own_folder(&incoming.0)?;
    manifest.check()?;
    std::fs::write(incoming.0.join(SOURCE), format!("{source}\n{commit}\n")).map_err(|e| e.to_string())?;
    Ok(Staged { incoming, manifest, unchecked })
}

/// The installed plugin called `name`, read from where it's now been put.
fn installed(name: &str, folder: &Path) -> Result<Manifest, String> {
    Manifest::in_own_folder(&folder.join(name))
}

/// Install from `source` (a folder, a release, a git URL, or GitHub's `owner/repo`),
/// whether or not it could be checked against a checksum.
pub fn install(source: &str, folder: &Path) -> Result<Manifest, String> {
    fetch(source, folder)?.install(folder)
}

/// Fetch `name` again from where it came from (a release's latest), and replace it.
pub fn update(name: &str, folder: &Path) -> Result<Manifest, String> {
    update_from(name, folder, &Hosts::default())
}

/// [`update`], with GitHub where `hosts` says.
pub fn update_from(name: &str, folder: &Path, hosts: &Hosts) -> Result<Manifest, String> {
    fetch_update_from(name, folder, hosts)?.replace(folder)
}

/// `name` fetched again from where it came from and checked, ready to put in place of
/// the one installed: [`Staged::replace`].
pub fn fetch_update(name: &str, folder: &Path) -> Result<Staged, String> {
    fetch_update_from(name, folder, &Hosts::default())
}

/// [`fetch_update`], with GitHub where `hosts` says.
pub fn fetch_update_from(name: &str, folder: &Path, hosts: &Hosts) -> Result<Staged, String> {
    if !manifest::proper_name(name) {
        return Err(format!("no plugin {name}"));
    }
    if folder.join(format!("{name}.{LINK}")).is_file() {
        return Err(format!("{name} is linked: it's read again by itself when it changes"));
    }
    let recorded = std::fs::read_to_string(folder.join(name).join(SOURCE)).map_err(|_| format!("{name} wasn't installed with :plugin install"))?;
    let source = recorded.lines().next().unwrap_or_default();
    let staged = fetch_from(source, folder, hosts)?;
    if let Some(why) = &staged.unchecked {
        return Err(format!("{why}, so it wasn't updated: :plugin remove {name}, then install it, to take it anyway"));
    }
    if staged.manifest.name != name {
        return Err(format!("{source} is now called {}: :plugin remove {name}, then install it", staged.manifest.name));
    }
    Ok(staged)
}

/// The plugins that [`update`] can fetch again.
pub fn updatable(folder: &Path) -> Vec<String> {
    let found = std::fs::read_dir(folder).into_iter().flatten().flatten();
    let mut names: Vec<String> =
        found.filter(|entry| entry.path().join(SOURCE).is_file()).map(|entry| entry.file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    names
}

/// Use the plugin in the folder `path` where it is, not a copy: it's read again when its
/// manifest or its code changes ([`Watch`]), for working on it.
pub fn link(path: &Path, folder: &Path) -> Result<Manifest, String> {
    let path = path.canonicalize().map_err(|_| format!("{} isn't a folder", path.display()))?;
    let manifest = Manifest::in_own_folder(&path)?;
    manifest.check()?;
    let name = manifest.name;
    if is_installed(name, folder) {
        return Err(format!("{name} is already installed: :plugin remove {name} first"));
    }
    std::fs::create_dir_all(folder).map_err(|e| format!("couldn't make {}: {e}", folder.display()))?;
    std::fs::write(folder.join(format!("{name}.{LINK}")), format!("{}\n", path.display())).map_err(|e| format!("couldn't link it: {e}"))?;
    Ok(manifest)
}

/// The plugins linked in `folder`: their names, and where they are.
pub fn linked(folder: &Path) -> Vec<(String, PathBuf)> {
    let found = std::fs::read_dir(folder).into_iter().flatten().flatten().map(|entry| entry.path());
    let mut links: Vec<(String, PathBuf)> = found
        .filter(|path| path.extension().is_some_and(|e| e == LINK))
        .filter_map(|path| {
            let name = path.file_stem()?.to_string_lossy().into_owned();
            let to = std::fs::read_to_string(&path).ok()?;
            Some((name, PathBuf::from(to.trim())))
        })
        .collect();
    links.sort();
    links
}

/// What a linked plugin was, the last time it was looked at: when its manifest and its
/// `.wasm` files were changed, and how big they were.
type Stamp = Vec<(PathBuf, Option<SystemTime>, u64)>;

fn stamp(path: &Path) -> Stamp {
    let files = std::fs::read_dir(path).into_iter().flatten().flatten().map(|entry| entry.path());
    let mut watched: Vec<PathBuf> = files.filter(|file| file.extension().is_some_and(|e| e == "wasm")).collect();
    watched.push(path.join(manifest::FILE));
    watched.sort();
    watched
        .into_iter()
        .map(|file| {
            let meta = std::fs::metadata(&file).ok();
            let modified = meta.as_ref().and_then(|meta| meta.modified().ok());
            (file, modified, meta.map_or(0, |meta| meta.len()))
        })
        .collect()
}

/// The linked plugins, watched for changes.
#[derive(Default)]
pub struct Watch(Option<HashMap<String, Stamp>>);

impl Watch {
    pub const fn new() -> Self {
        Self(None)
    }

    /// Those that have changed since it last looked, read again (or what's wrong with
    /// them now). The first time, it only looks.
    pub fn changed(&mut self, folder: &Path) -> Vec<(String, Result<Manifest, String>)> {
        let mut changed = vec![];
        let links = linked(folder);
        let seen = self.0.get_or_insert_default();
        seen.retain(|name, _| links.iter().any(|(linked, _)| linked == name));
        for (name, path) in links {
            let now = stamp(&path);
            match seen.insert(name.clone(), now.clone()) {
                Some(before) if before != now => changed.push((name, Manifest::in_own_folder(&path))),
                _ => {}
            }
        }
        changed
    }
}

/// Take `name` out of the plugins folder (a link, only the link).
pub fn remove(name: &str, folder: &Path) -> Result<(), String> {
    if !manifest::proper_name(name) {
        return Err(format!("no plugin {name}"));
    }
    let (own, file, link) = (folder.join(name), folder.join(format!("{name}.json")), folder.join(format!("{name}.{LINK}")));
    if own.is_dir() {
        std::fs::remove_dir_all(own).map_err(|e| format!("couldn't remove it: {e}"))
    } else if file.is_file() {
        std::fs::remove_file(file).map_err(|e| format!("couldn't remove it: {e}"))
    } else if link.is_file() {
        std::fs::remove_file(link).map_err(|e| format!("couldn't unlink it: {e}"))
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

    /// A GitHub of its own, with releases served as GitHub's API serves them.
    struct Github {
        server: crate::stand_in::StandIn,
        hosts: Hosts,
    }

    impl Github {
        fn new() -> Self {
            let server = crate::stand_in::StandIn::start();
            let hosts = Hosts { api: server.url.clone(), web: "file:///nowhere".into() };
            Self { server, hosts }
        }

        /// `repo`'s release `tag`, with `assets`; the latest, if it's `latest`.
        fn release(&self, repo: &str, tag: &str, latest: bool, assets: &[(&str, Vec<u8>)]) {
            let listed: Vec<serde_json::Value> = assets
                .iter()
                .map(|(name, bytes)| {
                    let path = format!("/download/{repo}/{tag}/{name}");
                    self.server.serve(&path, bytes.clone());
                    serde_json::json!({ "name": name, "browser_download_url": format!("{}{path}", self.server.url) })
                })
                .collect();
            let release = serde_json::json!({ "tag_name": tag, "assets": listed }).to_string();
            self.server.serve(&format!("/repos/{repo}/releases/tags/{tag}"), release.clone());
            if latest {
                self.server.serve(&format!("/repos/{repo}/releases/latest"), release);
            }
        }

        fn install(&self, source: &str, folder: &Path) -> Result<Manifest, String> {
            fetch_from(source, folder, &self.hosts)?.install(folder)
        }
    }

    /// A zip of the plugin `name` at `version`, in a folder of its own.
    fn zipped(name: &str, version: &str) -> Vec<u8> {
        let json = format!(r#"{{ "name": "{name}", "version": "{version}", "extensions": ["{name}"] }}"#);
        unzip::zip(&[(&format!("{name}/"), b""), (&format!("{name}/plugin.json"), json.as_bytes()), (&format!("{name}/notes.txt"), b"notes")])
    }

    fn sha256(bytes: &[u8]) -> String {
        use sha2::Digest as _;
        sha2::Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
    }

    /// A release of one plugin, as plugin-build makes them: `<name>-<version>.zip` and its `.sha256`.
    fn one(github: &Github, version: &str, latest: bool) {
        let zip = zipped("shelf", version);
        let file = format!("shelf-{version}.zip");
        let sum = format!("{}  {file}\n", sha256(&zip));
        github.release("felix/shelf", &format!("v{version}"), latest, &[(file.as_str(), zip), (&format!("{file}.sha256"), sum.into_bytes())]);
    }

    fn left(folder: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(folder).unwrap().flatten().map(|entry| entry.file_name().to_string_lossy().into_owned()).collect();
        names.sort();
        names
    }

    #[test]
    fn the_latest_release_is_installed_or_the_one_asked_for_and_updates_follow_releases() {
        let (github, folder) = (Github::new(), ghostwrite_project::scratch_dir("plugins"));
        one(&github, "1.0.0", false);
        one(&github, "2.0.0", true);
        let latest = github.install("felix/shelf", &folder).unwrap();
        assert_eq!(latest.version, Some("2.0.0"));
        assert!(folder.join("shelf/notes.txt").is_file(), "its folder, unzipped");
        assert_eq!(std::fs::read_to_string(folder.join("shelf").join(SOURCE)).unwrap(), "felix/shelf\nv2.0.0\n");
        remove("shelf", &folder).unwrap();

        let pinned = github.install("https://github.com/felix/shelf@1.0.0", &folder).unwrap();
        assert_eq!(pinned.version, Some("1.0.0"), "a version, by its tag v1.0.0");
        assert!(fetch_from("felix/shelf@9.9.9", &folder, &github.hosts).err().unwrap_or_default().contains("felix/shelf has no release 9.9.9"));
        assert_eq!(update_from("shelf", &folder, &github.hosts).unwrap().version, Some("2.0.0"), "an update follows the releases");
        one(&github, "3.0.0", true);
        assert_eq!(update_from("shelf", &folder, &github.hosts).unwrap().version, Some("3.0.0"));
        assert_eq!(left(&folder), ["shelf"], "nothing's left lying about");
        assert!(
            github.server.asked().iter().all(|asked| !asked.contains("Authorization") || std::env::var("GITHUB_TOKEN").is_ok()),
            "no token unless there's one"
        );
    }

    #[test]
    fn one_plugin_is_picked_from_a_release_of_several() {
        let (github, folder) = (Github::new(), ghostwrite_project::scratch_dir("plugins"));
        let (a, b) = (zipped("alpha", "1.0.0"), zipped("beta", "1.0.0"));
        let sums = format!("{}  alpha.zip\n{}  beta.zip\n", sha256(&a), sha256(&b));
        github.release("felix/several", "v1.0.0", true, &[("alpha.zip", a), ("beta.zip", b), ("SHA256SUMS", sums.into_bytes())]);
        let said = github.install("felix/several", &folder).err().unwrap_or_default();
        assert!(said.contains("several plugins") && said.contains("felix/several/‹name›") && said.contains("alpha, beta"), "{said}");
        assert!(github.install("felix/several/gamma", &folder).err().unwrap_or_default().contains("no plugin gamma"));
        let staged = fetch_from("felix/several/beta", &folder, &github.hosts).unwrap();
        assert!(staged.unchecked.is_none(), "checked against SHA256SUMS");
        assert_eq!(staged.install(&folder).unwrap().name, "beta");
        assert_eq!(std::fs::read_to_string(folder.join("beta").join(SOURCE)).unwrap(), "felix/several/beta\nv1.0.0\n");
        assert_eq!(update_from("beta", &folder, &github.hosts).unwrap().name, "beta");
        assert_eq!(left(&folder), ["beta"]);
    }

    #[test]
    fn a_zip_that_isnt_what_its_checksum_says_changes_nothing() {
        let (github, folder) = (Github::new(), ghostwrite_project::scratch_dir("plugins"));
        one(&github, "1.0.0", true);
        github.install("felix/shelf", &folder).unwrap();
        let zip = zipped("shelf", "2.0.0");
        let wrong = format!("{}  shelf-2.0.0.zip\n", sha256(b"something else"));
        github.release("felix/shelf", "v2.0.0", true, &[("shelf-2.0.0.zip", zip), ("shelf-2.0.0.zip.sha256", wrong.into_bytes())]);
        let said = update_from("shelf", &folder, &github.hosts).err().unwrap_or_default();
        assert!(said.contains("isn't what its checksum says"), "{said}");
        assert_eq!(installed("shelf", &folder).unwrap().version, Some("1.0.0"), "it's left as it was");
        remove("shelf", &folder).unwrap();
        assert!(github.install("felix/shelf", &folder).is_err());
        assert!(left(&folder).is_empty(), "nothing's left lying about: {:?}", left(&folder));
    }

    #[test]
    fn a_release_without_a_checksum_is_said_to_be_so() {
        let (github, folder) = (Github::new(), ghostwrite_project::scratch_dir("plugins"));
        one(&github, "1.0.0", true);
        github.install("felix/shelf", &folder).unwrap();
        github.release("felix/shelf", "v2.0.0", true, &[("shelf-2.0.0.zip", zipped("shelf", "2.0.0"))]);
        let said = update_from("shelf", &folder, &github.hosts).err().unwrap_or_default();
        assert!(said.contains("no checksum"), "an update doesn't take it: {said}");
        assert_eq!(installed("shelf", &folder).unwrap().version, Some("1.0.0"));
        remove("shelf", &folder).unwrap();
        let staged = fetch_from("felix/shelf", &folder, &github.hosts).unwrap();
        assert!(staged.unchecked.as_deref().is_some_and(|why| why.contains("shelf-2.0.0.zip has no checksum")));
        assert!(!folder.join("shelf").exists(), "it's only installed when it's asked to be");
        assert_eq!(staged.install(&folder).unwrap().version, Some("2.0.0"));
    }

    #[test]
    fn a_repository_with_no_release_is_cloned() {
        let (mut github, folder) = (Github::new(), ghostwrite_project::scratch_dir("plugins"));
        let web = ghostwrite_project::scratch_dir("web").canonicalize().unwrap();
        let source = web.join("felix/shelf");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::write(source.join(manifest::FILE), r#"{ "name": "shelf", "version": "0.1" }"#).unwrap();
        let git = |args: &[&str]| std::process::Command::new("git").arg("-C").arg(&source).args(args).output().map(|out| out.status.success()).unwrap_or(false);
        if !git(&["init", "-q"]) {
            return eprintln!("no git: skipped");
        }
        assert!(git(&["add", manifest::FILE]) && git(&["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-qm", "first"]));
        github.hosts.web = format!("file://{}", web.display());
        assert_eq!(github.install("felix/shelf", &folder).unwrap().version, Some("0.1"));
        let recorded = std::fs::read_to_string(folder.join("shelf").join(SOURCE)).unwrap();
        assert_eq!(recorded.lines().next(), Some("felix/shelf"));
        assert!(fetch_from("felix/shelf@1.0", &folder, &github.hosts).err().unwrap_or_default().contains("has no release 1.0"));
        assert!(fetch_from("felix/shelf/other", &folder, &github.hosts).is_err(), "a plugin of several needs a release");
    }

    #[test]
    fn a_linked_folder_is_read_where_it_is_and_again_when_it_changes() {
        let (source, folder) = (plugin("1"), ghostwrite_project::scratch_dir("plugins"));
        let linked_one = link(&source, &folder).unwrap();
        assert_eq!(linked_one.version, Some("1"));
        assert!(!folder.join("shelf").exists() && folder.join("shelf.link").is_file(), "not a copy");
        let (found, wrong) = Manifest::in_folder(&folder);
        assert!(wrong.is_empty(), "{wrong:?}");
        assert_eq!(found.iter().map(|m| (m.name, m.version)).collect::<Vec<_>>(), [("shelf", Some("1"))]);
        assert!(link(&source, &folder).err().unwrap_or_default().contains("already installed"));
        assert!(update("shelf", &folder).err().unwrap_or_default().contains("linked"));
        assert!(updatable(&folder).is_empty());

        let mut watch = Watch::default();
        assert!(watch.changed(&folder).is_empty(), "the first look only looks");
        assert!(watch.changed(&folder).is_empty(), "nor has anything changed");
        std::fs::write(source.join(manifest::FILE), r#"{ "name": "shelf", "version": "two" }"#).unwrap();
        let changed = watch.changed(&folder);
        assert_eq!(changed.len(), 1);
        assert_eq!(changed[0].1.as_ref().map(|m| m.version), Ok(Some("two")));
        std::fs::write(source.join("code.wasm"), "not yet").unwrap();
        assert_eq!(watch.changed(&folder).into_iter().map(|(name, _)| name).collect::<Vec<_>>(), ["shelf"], "and when its code does");
        std::fs::write(source.join(manifest::FILE), "broken").unwrap();
        assert!(watch.changed(&folder)[0].1.is_err(), "what's wrong with it, when it is");

        remove("shelf", &folder).unwrap();
        assert!(left(&folder).is_empty(), "unlinked");
        assert!(source.join(manifest::FILE).is_file(), "and the folder is left alone");
        assert!(watch.changed(&folder).is_empty());
    }
}
