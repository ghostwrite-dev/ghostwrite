//! A plugin from a GitHub release: `owner/repo` (its latest), `owner/repo@1.2.0` (that
//! one), and `owner/repo/name`, one of several a release has. The zip is
//! `<name>-<version>.zip` or `<name>.zip`, and it's checked against the `.sha256` beside
//! it or the release's `SHA256SUMS`. GitHub's API is asked without a token, unless
//! `GITHUB_TOKEN` gives one.

use std::io::Read as _;
use std::time::Duration;

use serde_json::Value;
use sha2::Digest as _;

use crate::manifest;

/// Where GitHub is: its API, and where repositories are cloned from.
#[derive(Clone, Debug)]
pub struct Hosts {
    pub api: String,
    pub web: String,
}

impl Default for Hosts {
    fn default() -> Self {
        Self { api: "https://api.github.com".into(), web: "https://github.com".into() }
    }
}

/// A repository on GitHub, as it was named.
#[derive(Debug, PartialEq, Eq)]
pub struct Repo {
    pub owner: String,
    pub repo: String,
    /// One plugin of the several a release has.
    pub name: Option<String>,
    /// A release's version; otherwise the latest.
    pub version: Option<String>,
}

impl Repo {
    /// `owner/repo[/name][@version]`, or the same after `https://github.com/`.
    pub fn parse(source: &str) -> Option<Self> {
        let rest = source.strip_prefix("https://github.com/").unwrap_or(source);
        let (path, version) = match rest.rsplit_once('@') {
            Some((path, version)) => (path, Some(version)),
            None => (rest, None),
        };
        let version_fine = |v: &str| !v.is_empty() && v.chars().all(|c| c.is_ascii_alphanumeric() || "-_.+".contains(c)) && !v.starts_with(['.', '-']);
        if version.is_some_and(|v| !version_fine(v)) {
            return None;
        }
        let parts: Vec<&str> = path.trim_end_matches('/').split('/').collect();
        let repo_fine = |repo: &str| !repo.is_empty() && !repo.starts_with('.') && repo.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c));
        let (owner, repo, name) = match parts[..] {
            [owner, repo] => (owner, repo, None),
            [owner, repo, name] if manifest::proper_name(name) => (owner, repo, Some(name.to_string())),
            _ => return None,
        };
        let repo = repo.strip_suffix(".git").unwrap_or(repo);
        (manifest::proper_name(owner) && repo_fine(repo)).then(|| Self { owner: owner.into(), repo: repo.into(), name, version: version.map(Into::into) })
    }

    /// As it's recorded, to be updated from: without a version, which an update leaves behind.
    pub fn followed(&self) -> String {
        let Self { owner, repo, name, .. } = self;
        match name {
            Some(name) => format!("{owner}/{repo}/{name}"),
            None => format!("{owner}/{repo}"),
        }
    }
}

/// What a release has for one plugin.
pub struct Release {
    pub tag: String,
    /// The zip, by name, and what it was checked against, if anything.
    pub zip: String,
    pub bytes: Vec<u8>,
    pub checked: bool,
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder().http_status_as_error(false).timeout_global(Some(Duration::from_secs(120))).user_agent("ghostwrite").build().into()
}

/// What's at `url`, as long as it's not more than a plugin could be.
fn download(url: &str) -> Result<Vec<u8>, String> {
    let mut response = agent().get(url).call().map_err(|e| format!("couldn't download {url}: {e}"))?;
    if !response.status().is_success() {
        return Err(format!("couldn't download {url}: {}", response.status()));
    }
    let mut bytes = vec![];
    response.body_mut().as_reader().take(1 << 30).read_to_end(&mut bytes).map_err(|e| format!("couldn't download {url}: {e}"))?;
    Ok(bytes)
}

/// The release `path` asks the API for, or None if there's none.
fn release(hosts: &Hosts, path: &str) -> Result<Option<Value>, String> {
    let mut request = agent().get(format!("{}{path}", hosts.api)).header("Accept", "application/vnd.github+json").header("X-GitHub-Api-Version", "2022-11-28");
    if let Some(token) = std::env::var("GITHUB_TOKEN").ok().filter(|token| !token.trim().is_empty()) {
        request = request.header("Authorization", &format!("Bearer {}", token.trim()));
    }
    let mut response = request.call().map_err(|e| format!("couldn't ask GitHub: {e}"))?;
    let body = response.body_mut().read_to_string().unwrap_or_default();
    match response.status().as_u16() {
        200 => serde_json::from_str(&body).map(Some).map_err(|e| format!("GitHub's answer isn't JSON: {e}")),
        404 => Ok(None),
        403 | 429 => Err("GitHub won't answer so many questions for now: try again later, or set GITHUB_TOKEN".into()),
        status => Err(format!("GitHub answered {status}")),
    }
}

/// `repo`'s release (its latest, or the version it names), or None if it has none.
pub fn fetch(repo: &Repo, hosts: &Hosts) -> Result<Option<Release>, String> {
    let Repo { owner, repo: name_of_repo, name, version } = repo;
    let found = match version {
        None => release(hosts, &format!("/repos/{owner}/{name_of_repo}/releases/latest"))?,
        Some(version) => {
            let tagged = |tag: &str| release(hosts, &format!("/repos/{owner}/{name_of_repo}/releases/tags/{tag}"));
            match version.strip_prefix('v') {
                Some(_) => tagged(version)?,
                None => match tagged(&format!("v{version}"))? {
                    Some(found) => Some(found),
                    None => tagged(version)?,
                },
            }
        }
    };
    let Some(found) = found else { return Ok(None) };
    let tag = found["tag_name"].as_str().unwrap_or_default().to_string();
    let bare = tag.strip_prefix('v').unwrap_or(&tag);
    let assets: Vec<(&str, &str)> = found["assets"]
        .as_array()
        .map(|assets| assets.iter().filter_map(|asset| Some((asset["name"].as_str()?, asset["browser_download_url"].as_str()?))).collect())
        .unwrap_or_default();
    let zips: Vec<&str> = assets.iter().map(|(name, _)| *name).filter(|name| name.ends_with(".zip")).collect();
    let plugin_of = |zip: &str| zip.strip_suffix(&format!("-{bare}.zip")).or_else(|| zip.strip_suffix(".zip")).unwrap_or(zip).to_string();
    let named = |plugin: &str| zips.iter().find(|zip| plugin_of(zip) == plugin).copied();
    let all = zips.iter().map(|zip| plugin_of(zip)).collect::<Vec<_>>().join(", ");
    let zip = match name {
        Some(name) => named(name).ok_or_else(|| format!("{owner}/{name_of_repo} {tag} has no plugin {name}; it has {all}"))?,
        None => match zips[..] {
            [] => return Err(format!("{owner}/{name_of_repo} {tag} has no plugin in it (no zip)")),
            [only] => only,
            _ => named(name_of_repo)
                .ok_or_else(|| format!("{owner}/{name_of_repo} {tag} has several plugins: say which, as {owner}/{name_of_repo}/‹name›, of {all}"))?,
        },
    };
    let url_of = |asset: &str| assets.iter().find(|(name, _)| *name == asset).map(|(_, url)| url.to_string());

    // What it should be: beside it, or in the release's list.
    let expected = if let Some(url) = url_of(&format!("{zip}.sha256")) {
        let text = String::from_utf8_lossy(&download(&url)?).into_owned();
        Some(text.split_whitespace().next().unwrap_or_default().to_lowercase())
    } else if let Some(url) = url_of("SHA256SUMS") {
        let text = String::from_utf8_lossy(&download(&url)?).into_owned();
        let listed = text.lines().find_map(|line| {
            let (sum, file) = line.split_once(char::is_whitespace)?;
            (file.trim().trim_start_matches('*') == zip).then(|| sum.to_lowercase())
        });
        Some(listed.ok_or_else(|| format!("{zip} isn't in the release's SHA256SUMS"))?)
    } else {
        None
    };
    let bytes = download(&url_of(zip).unwrap_or_default())?;
    if let Some(expected) = &expected {
        let sum: String = sha2::Sha256::digest(&bytes).iter().map(|b| format!("{b:02x}")).collect();
        if &sum != expected {
            return Err(format!("{zip} isn't what its checksum says it is ({sum}, not {expected})"));
        }
    }
    Ok(Some(Release { tag, zip: zip.to_string(), bytes, checked: expected.is_some() }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_repository_is_named_with_a_plugin_and_a_version() {
        let parsed = |source: &str| Repo::parse(source).map(|r| (r.owner, r.repo, r.name, r.version));
        let s = |s: &str| s.to_string();
        assert_eq!(parsed("felix/words"), Some((s("felix"), s("words"), None, None)));
        assert_eq!(parsed("felix/words@1.2.0"), Some((s("felix"), s("words"), None, Some(s("1.2.0")))));
        assert_eq!(parsed("felix/words/one@v2"), Some((s("felix"), s("words"), Some(s("one")), Some(s("v2")))));
        assert_eq!(parsed("https://github.com/felix/words.git"), Some((s("felix"), s("words"), None, None)));
        assert_eq!(parsed("https://github.com/felix/words/"), Some((s("felix"), s("words"), None, None)));
        for wrong in ["words", "../../x", "a/b/c/d", "a/..", "a/b@", "a/b@../x", "a/b@-x", "a/b/../c", "https://example.com/a/b"] {
            assert!(Repo::parse(wrong).is_none(), "{wrong}");
        }
        assert_eq!(Repo::parse("felix/words/one@1.0.0").unwrap().followed(), "felix/words/one");
    }
}
