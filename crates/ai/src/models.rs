//! Every model that can be asked: Claude's, if Claude can be reached, and the
//! local ones on this Mac (Ghostwrite's, and those Hugging Face's tools or Ollama
//! downloaded). The one chosen is remembered.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use ghostwrite_project::store;
use serde_json::Value;

use crate::backend::Backend;
use crate::{local, settings};

/// Claude's models: their ids, names, and what `:model` calls them.
pub const CLAUDE: [(&str, &str, &str); 4] = [
    ("claude-opus-5-5", "Claude Opus 5.5", "opus-5.5"),
    ("claude-sonnet-5-5", "Claude Sonnet 5.5", "sonnet-5.5"),
    ("claude-haiku-5-5", "Claude Haiku 5.5", "haiku-5.5"),
    ("claude-fable-5-1", "Claude Fable 5.1", "fable-5.1"),
];

const CHOSEN: &str = "chosen-model";

/// A model that can be asked, and what `:model` calls it.
pub struct Available {
    pub backend: Backend,
    pub short: String,
}

/// What `:model` calls a model.
fn short(backend: &Backend) -> String {
    let id = backend.id();
    CLAUDE.iter().find(|(claude, ..)| *claude == id).map_or_else(|| backend.name().to_string(), |(.., short)| short.to_string())
}

/// `names` that `typed` could mean, ignoring case and punctuation (`opus55`,
/// `QWEN3`). A name it starts is likelier than one it's only in.
pub fn matching<'a>(names: &'a [String], typed: &str) -> Vec<&'a str> {
    let plain = |text: &str| text.chars().filter(char::is_ascii_alphanumeric).collect::<String>().to_lowercase();
    let typed = plain(typed);
    let mut hits: Vec<&str> = names.iter().map(String::as_str).filter(|name| plain(name).contains(&typed)).collect();
    hits.sort_by_key(|name| !plain(name).starts_with(&typed));
    hits
}

/// What `:model` can complete to.
pub fn names() -> Vec<String> {
    available().into_iter().map(|a| a.short).collect()
}

/// The model `typed` means, or why there isn't just one.
pub fn find(typed: &str) -> Result<Backend, String> {
    let all = available();
    let names: Vec<String> = all.iter().map(|a| a.short.clone()).collect();
    let exact = names.iter().find(|name| name.eq_ignore_ascii_case(typed));
    let hits = matching(&names, typed);
    let name = match (exact, hits.as_slice()) {
        (Some(name), _) => name.as_str(),
        (None, [one]) => *one,
        (None, []) => return Err(format!("no model {typed}. There's {}", names.join("  "))),
        (None, several) => return Err(format!("{typed} could be {}", several.join("  "))),
    };
    let found = all.into_iter().find(|a| a.short == name).map(|a| a.backend);
    found.ok_or_else(|| format!("no model {typed}"))
}

/// Everything that can be asked: Claude's first, then the local ones.
pub fn available() -> Vec<Available> {
    let mut all = vec![];
    if Backend::claude(&settings().model).is_some() {
        for id in CLAUDE.iter().map(|(id, ..)| *id) {
            all.extend(Backend::claude(id).map(|backend| Available { short: short(&backend), backend }));
        }
    }
    let on_disk = std::env::home_dir().map_or(vec![], |home| on_disk(&home));
    if let Some(configured) = &settings().local_model {
        // Named in the config, and maybe not downloaded yet.
        let label = local_label(configured);
        all.push(Available { short: label.clone(), backend: Backend::Local { model: configured.clone(), label } });
    }
    let shown: HashSet<String> = all.iter().map(|a| a.backend.name().to_string()).collect();
    all.extend(
        on_disk
            .into_iter()
            .filter(|m| !shown.contains(&m.label))
            .map(|m| Available { short: m.label.clone(), backend: Backend::Local { model: m.path, label: m.label } }),
    );
    all
}

/// What a local model is called: `user/Name-GGUF:Q4_K_M` is `Name Q4_K_M`.
pub fn local_label(model: &str) -> String {
    let name = local::short_name(model);
    match model.rsplit_once(':') {
        Some((_, quantization)) if !model.starts_with('/') => format!("{name}-{quantization}"),
        _ => name.to_string(),
    }
}

struct OnDisk {
    label: String,
    path: String,
}

/// The local models under `home`.
fn on_disk(home: &Path) -> Vec<OnDisk> {
    let mut models = vec![];
    let mut seen = HashSet::new();
    let mut add = |label: String, path: PathBuf| {
        // A file may be there under several names.
        let real = ghostwrite_project::platform::real(&path).unwrap_or_else(|_| path.clone());
        if real.is_file() && seen.insert(real) {
            models.push(OnDisk { label, path: path.display().to_string() });
        }
    };
    let gguf_in = |dir: &Path| -> Vec<PathBuf> {
        let files = std::fs::read_dir(dir).into_iter().flatten().flatten().map(|e| e.path());
        let mut files: Vec<PathBuf> = files
            .filter(|p| p.extension().is_some_and(|e| e == "gguf"))
            .filter(|p| !p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("mmproj")))
            .collect();
        files.sort();
        files
    };
    let subfolders = |dir: &Path| -> Vec<PathBuf> {
        let mut dirs: Vec<PathBuf> = std::fs::read_dir(dir).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
        dirs.sort();
        dirs
    };
    let cache = if cfg!(test) {
        home.join("Library/Caches/ghostwrite")
    } else {
        ghostwrite_project::platform::cache_dir().unwrap_or_else(|| home.join(".cache/ghostwrite"))
    };
    for folder in subfolders(&cache.join("models")) {
        for file in gguf_in(&folder) {
            add(file.file_stem().unwrap_or_default().to_string_lossy().into_owned(), file);
        }
    }
    for folder in subfolders(&home.join(".cache/huggingface/hub")) {
        for snapshot in subfolders(&folder.join("snapshots")) {
            for file in gguf_in(&snapshot) {
                add(file.file_stem().unwrap_or_default().to_string_lossy().into_owned(), file);
            }
        }
    }
    let ollama = home.join(".ollama/models");
    let manifests = ollama.join("manifests/registry.ollama.ai");
    // Ollama's own models first: one published again by someone else is the same file.
    let mut namespaces = subfolders(&manifests);
    namespaces.sort_by_key(|n| n.file_name().is_none_or(|n| n != "library"));
    for namespace in namespaces {
        for model in subfolders(&namespace) {
            let mut tags: Vec<PathBuf> = std::fs::read_dir(&model).into_iter().flatten().flatten().map(|e| e.path()).collect();
            tags.sort();
            for tag in tags {
                let name = model.file_name().unwrap_or_default().to_string_lossy();
                let tag_name = tag.file_name().unwrap_or_default().to_string_lossy().into_owned();
                // Base models continue text: they can't follow a conversation.
                if tag_name.ends_with("-base") {
                    continue;
                }
                let Some(blob) = ollama_blob(&tag) else { continue };
                let owner = namespace.file_name().unwrap_or_default().to_string_lossy();
                let label = if owner == "library" { format!("{name}:{tag_name}") } else { format!("{owner}/{name}:{tag_name}") };
                add(label, ollama.join("blobs").join(blob));
            }
        }
    }
    models
}

/// The file with an Ollama manifest's model in it.
fn ollama_blob(manifest: &Path) -> Option<String> {
    let manifest: Value = serde_json::from_str(&std::fs::read_to_string(manifest).ok()?).ok()?;
    let layers = manifest["layers"].as_array()?;
    let model = layers.iter().find(|l| l["mediaType"].as_str().is_some_and(|t| t.ends_with(".model")))?;
    Some(model["digest"].as_str()?.replace(':', "-"))
}

/// Keep `backend` as the one to ask from now on.
pub fn remember(backend: &Backend) {
    if let Some(saved) = saved(backend) {
        store::write(CHOSEN, &saved);
    }
}

fn saved(backend: &Backend) -> Option<String> {
    match backend {
        Backend::Api { model, .. } | Backend::ClaudeCode { model } => Some(format!("claude\n{model}\n")),
        Backend::Local { model, label } => Some(format!("local\n{model}\n{label}\n")),
        Backend::Scripted(_) => None,
    }
}

/// The one chosen before, if it can still be asked.
pub fn remembered() -> Option<Backend> {
    from_saved(&store::read(CHOSEN)?)
}

fn from_saved(text: &str) -> Option<Backend> {
    let mut lines = text.lines();
    match (lines.next()?, lines.next()?) {
        ("claude", model) => Backend::claude(model),
        ("local", model) => {
            // A file that's gone can't be asked; a Hugging Face name is downloaded again.
            let gone = model.starts_with('/') && !Path::new(model).exists();
            (!gone).then(|| Backend::Local { model: model.to_string(), label: lines.next().map_or_else(|| local_label(model), str::to_string) })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: PathBuf, bytes: usize) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, vec![0; bytes]).unwrap();
    }

    #[test]
    fn it_finds_every_local_model_once() {
        let home = ghostwrite_project::scratch_dir("models-home");
        file(home.join("Library/Caches/ghostwrite/models/user--A-GGUF/A-Q3_K_M.gguf"), 10);
        file(home.join("Library/Caches/ghostwrite/models/user--A-GGUF/A-Q3_K_M.gguf.part"), 10);
        file(home.join(".cache/huggingface/hub/models--user--B-GGUF/snapshots/abc/B-Q4.gguf"), 10);
        file(home.join(".cache/huggingface/hub/models--user--B-GGUF/snapshots/abc/mmproj-B.gguf"), 10);
        file(home.join(".cache/huggingface/hub/models--gpt2/snapshots/abc/model.safetensors"), 10);
        let blobs = home.join(".ollama/models/blobs");
        file(blobs.join("sha256-one"), 10);
        file(blobs.join("sha256-two"), 10);
        let manifest = |digest: &str| {
            format!(
                r#"{{"layers":[{{"mediaType":"application/vnd.ollama.image.template","digest":"sha256:t"}},{{"mediaType":"application/vnd.ollama.image.model","digest":"sha256:{digest}"}}]}}"#
            )
        };
        let manifests = home.join(".ollama/models/manifests/registry.ollama.ai");
        for (path, digest) in [("library/qwen3/4b", "one"), ("library/qwen3/latest", "two"), ("library/coder/7b-base", "two"), ("zz/copy/8b", "two")] {
            std::fs::create_dir_all(manifests.join(path).parent().unwrap()).unwrap();
            std::fs::write(manifests.join(path), manifest(digest)).unwrap();
        }
        let found: Vec<String> = on_disk(&home).into_iter().map(|m| m.label).collect();
        assert_eq!(found, ["A-Q3_K_M", "B-Q4", "qwen3:4b", "qwen3:latest"], "not parts, projectors, base models, or the same file again");
        let qwen = on_disk(&home).into_iter().find(|m| m.label == "qwen3:4b").unwrap();
        assert!(qwen.path.ends_with("blobs/sha256-one"), "{}", qwen.path);
    }

    #[test]
    fn a_model_is_found_however_its_written() {
        let names: Vec<String> = ["opus-5.5", "sonnet-5.5", "qwen3:4b", "qwen3:latest", "Qwen2.5-Coder-14B-Q3_K_M", "gemma3:270m"].map(str::to_string).into();
        assert_eq!(matching(&names, "opus55"), ["opus-5.5"]);
        assert_eq!(matching(&names, "OPUS-5.5"), ["opus-5.5"]);
        assert_eq!(matching(&names, "14b"), ["Qwen2.5-Coder-14B-Q3_K_M"]);
        assert_eq!(matching(&names, "qwen"), ["qwen3:4b", "qwen3:latest", "Qwen2.5-Coder-14B-Q3_K_M"]);
        assert_eq!(matching(&names, "").len(), 6, "nothing typed is all of them");
        assert!(matching(&names, "llama").is_empty());
    }

    /// What there is on this Mac.
    #[test]
    #[ignore]
    fn it_lists_this_macs_models() {
        println!("{}", names().join("\n"));
        for typed in ["opus55", "qwen", "gemma", "nope"] {
            println!("{typed}: {:?}", find(typed).map(|b| b.name().to_string()));
        }
    }

    #[test]
    fn nothing_there_is_no_models() {
        assert!(on_disk(&ghostwrite_project::scratch_dir("models-empty")).is_empty());
    }

    #[test]
    fn a_local_model_is_called_by_its_name_and_quantization() {
        assert_eq!(local_label("bartowski/Coder-14B-GGUF:Q3_K_M"), "Coder-14B-Q3_K_M");
        assert_eq!(local_label("/models/tiny.gguf"), "tiny");
    }

    #[test]
    fn a_choice_is_remembered_unless_its_gone() {
        let dir = ghostwrite_project::scratch_dir("models-saved");
        let there = dir.join("there");
        file(there.clone(), 1);
        let local = Backend::Local { model: there.display().to_string(), label: "the model".into() };
        let back = from_saved(&saved(&local).unwrap()).unwrap();
        assert_eq!((back.id(), back.name()), (local.id(), "the model"));
        std::fs::remove_file(&there).unwrap();
        assert!(from_saved(&saved(&local).unwrap()).is_none(), "a file that's gone");
        let hub = Backend::Local { model: "user/repo:Q4".into(), label: "repo".into() };
        assert!(from_saved(&saved(&hub).unwrap()).is_some(), "it can be downloaded again");
        assert!(from_saved("nonsense\n").is_none());
        assert!(saved(&Backend::Scripted(vec![])).is_none());
    }
}
