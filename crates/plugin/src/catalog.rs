//! Plugins there are to install, that don't come with the app: the catalog kept with
//! those that do (`catalog.json`), by the extensions each handles. It's fetched at most
//! once a day, and kept, so a file whose extension no plugin handles can say which
//! would, and how to install it.

use std::path::Path;
use std::time::{Duration, SystemTime};

use serde_json::Value;

/// Where the catalog is.
pub const URL: &str = "https://raw.githubusercontent.com/ghostwrite-dev/default-plugins/main/catalog.json";
/// How long what was fetched is kept before it's fetched again.
const KEPT: Duration = Duration::from_secs(24 * 60 * 60);

/// A plugin in the catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    /// What `:plugin install` takes: `owner/repo`.
    pub repo: String,
    pub description: String,
    pub extensions: Vec<String>,
}

impl Entry {
    /// What to say of it: what it is, and how to install it.
    pub fn suggestion(&self) -> String {
        format!("{}: `:plugin install {}`", self.description, self.repo)
    }
}

/// The plugins `text` lists; any that don't say what they are and how to get them, it doesn't.
pub fn parse(text: &str) -> Vec<Entry> {
    let json: Value = serde_json::from_str(text).unwrap_or_default();
    let entries = json["plugins"].as_array().map_or(&[][..], Vec::as_slice);
    entries
        .iter()
        .filter_map(|entry| {
            let text = |key: &str| entry[key].as_str().filter(|text| !text.is_empty()).map(str::to_string);
            let extensions = entry["extensions"].as_array()?.iter().filter_map(|e| e.as_str().map(|e| e.trim_start_matches('.').to_string())).collect();
            Some(Entry { name: text("name")?, repo: text("repo")?, description: text("description").or_else(|| text("name"))?, extensions })
        })
        .collect()
}

/// The catalog at `url`, as it was kept in the file `kept` if it was fetched less than a
/// day ago; otherwise fetched again, and kept. If it can't be, what was kept will do.
pub fn read(url: &str, kept: &Path) -> Result<Vec<Entry>, String> {
    let fresh = std::fs::metadata(kept).and_then(|meta| meta.modified()).is_ok_and(|at| SystemTime::now().duration_since(at).unwrap_or_default() < KEPT);
    if fresh && let Ok(text) = std::fs::read_to_string(kept) {
        return Ok(parse(&text));
    }
    let fetched = (|| {
        let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(10))).user_agent("ghostwrite").build().into();
        let text = agent.get(url).call().map_err(|e| e.to_string())?.body_mut().read_to_string().map_err(|e| e.to_string())?;
        let entries = parse(&text);
        if let Some(parent) = kept.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(kept, &text);
        Ok::<_, String>(entries)
    })();
    fetched.or_else(|why| std::fs::read_to_string(kept).map(|text| parse(&text)).map_err(|_| format!("couldn't fetch the catalog of plugins: {why}")))
}

/// The plugin in `entries` for files with `extension`, if one is.
pub fn for_extension<'a>(entries: &'a [Entry], extension: &str) -> Option<&'a Entry> {
    entries.iter().find(|entry| entry.extensions.iter().any(|e| e.eq_ignore_ascii_case(extension)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stand_in::StandIn;

    const CATALOG: &str = r#"{ "plugins": [
        { "name": "typst", "repo": "ghostwrite-dev/typst", "description": "Typst files", "extensions": ["typ"] },
        { "name": "broken" },
        { "name": "lean", "repo": "ghostwrite-dev/lean", "description": "Lean 4", "extensions": [".lean"] }
    ] }"#;

    #[test]
    fn the_catalog_says_which_plugin_handles_an_extension() {
        let entries = parse(CATALOG);
        assert_eq!(entries.len(), 2, "what doesn't say how to get it isn't there");
        assert_eq!(for_extension(&entries, "typ").map(Entry::suggestion).as_deref(), Some("Typst files: `:plugin install ghostwrite-dev/typst`"));
        assert_eq!(for_extension(&entries, "lean").map(|e| e.name.as_str()), Some("lean"));
        assert!(for_extension(&entries, "rs").is_none());
        assert!(parse("not json").is_empty());
    }

    #[test]
    fn it_is_fetched_at_most_once_a_day_and_kept() {
        let server = StandIn::start();
        server.serve("/catalog.json", CATALOG);
        let kept = ghostwrite_project::scratch_dir("catalog").join("catalog.json");
        let url = format!("{}/catalog.json", server.url);
        assert_eq!(read(&url, &kept).unwrap().len(), 2);
        server.serve("/catalog.json", r#"{ "plugins": [] }"#);
        assert_eq!(read(&url, &kept).unwrap().len(), 2, "it's kept for a day");
        assert_eq!(server.asked().len(), 1, "and only fetched once");

        let a_day_ago = SystemTime::now() - KEPT - Duration::from_secs(60);
        std::fs::File::options().write(true).open(&kept).unwrap().set_modified(a_day_ago).unwrap();
        assert!(read(&url, &kept).unwrap().is_empty(), "after a day, it's fetched again");
        assert_eq!(server.asked().len(), 2);

        std::fs::File::options().write(true).open(&kept).unwrap().set_modified(a_day_ago).unwrap();
        assert!(read(&format!("{}/gone", server.url), &kept).unwrap().is_empty(), "what was kept will do when it can't be fetched");
        assert!(read(&format!("{}/gone", server.url), &kept.with_extension("none")).is_err());
    }
}
