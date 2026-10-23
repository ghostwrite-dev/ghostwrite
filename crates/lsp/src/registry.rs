//! The servers Ghostwrite knows, and the ones running for a project.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use futures::channel::mpsc;

use crate::client::{Client, Event, Launch};
use crate::python;

/// A language server Ghostwrite knows how to start, and what it serves.
pub struct Spec {
    pub name: &'static str,
    command: &'static [&'static str],
    /// File extension → LSP language id.
    languages: &'static [(&'static str, &'static str)],
}

const SPECS: &[Spec] = &[
    Spec { name: "rust-analyzer", command: &["rust-analyzer"], languages: &[("rs", "rust")] },
    Spec { name: "basedpyright", command: &["basedpyright-langserver", "--stdio"], languages: &[("py", "python")] },
    Spec { name: "pyright", command: &["pyright-langserver", "--stdio"], languages: &[("py", "python")] },
    Spec { name: "pylsp", command: &["pylsp"], languages: &[("py", "python")] },
    Spec {
        name: "typescript-language-server",
        command: &["typescript-language-server", "--stdio"],
        languages: &[("ts", "typescript"), ("tsx", "typescriptreact"), ("js", "javascript"), ("jsx", "javascriptreact")],
    },
    Spec { name: "gopls", command: &["gopls"], languages: &[("go", "go")] },
    Spec { name: "clangd", command: &["clangd"], languages: &[("c", "c"), ("h", "c"), ("cc", "cpp"), ("cpp", "cpp"), ("hpp", "cpp"), ("cxx", "cpp")] },
];

/// Where each server's binary is, looked up once.
fn installed() -> &'static HashMap<&'static str, PathBuf> {
    static FOUND: OnceLock<HashMap<&'static str, PathBuf>> = OnceLock::new();
    FOUND.get_or_init(|| {
        let names: Vec<&str> = SPECS.iter().map(|s| s.command[0]).collect();
        SPECS.iter().zip(python::which_all(&names)).filter_map(|(spec, path)| Some((spec.name, path?))).collect()
    })
}

/// The installed server for a file, and the file's language id.
fn spec_for(path: &Path) -> Option<(&'static Spec, &'static str)> {
    let ext = path.extension()?.to_str()?;
    SPECS.iter().find_map(|spec| {
        let language = spec.languages.iter().find(|(e, _)| *e == ext)?.1;
        installed().contains_key(spec.name).then_some((spec, language))
    })
}

/// The servers of one project, started on first use.
pub struct Registry {
    root: PathBuf,
    clients: HashMap<&'static str, Option<Arc<Client>>>,
    events: mpsc::UnboundedSender<Event>,
    enabled: bool,
}

impl Registry {
    pub fn new(root: PathBuf, events: mpsc::UnboundedSender<Event>) -> Self {
        Self { root, clients: HashMap::new(), events, enabled: true }
    }

    /// No servers at all (tests open .rs and .py files without wanting them).
    pub fn disabled(root: PathBuf) -> Self {
        Self { enabled: false, ..Self::new(root, mpsc::unbounded().0) }
    }

    /// The server for `path` (starting it if needed), and the file's language id.
    pub fn for_path(&mut self, path: &Path) -> Option<(Arc<Client>, &'static str)> {
        if !self.enabled {
            return None;
        }
        let (spec, language) = spec_for(path)?;
        let client = self
            .clients
            .entry(spec.name)
            .or_insert_with(|| {
                let root = self.root.clone();
                let launch = Launch {
                    name: spec.name,
                    binary: installed()[spec.name].clone(),
                    args: &spec.command[1..],
                    root: &self.root,
                    env: python::server_env(&self.root),
                    settings: Box::new(move |section| python::settings(section, &root)),
                };
                Client::start(launch, self.events.clone()).inspect_err(|e| eprintln!("couldn't start {}: {e}", spec.name)).ok()
            })
            .clone()?;
        Some((client, language))
    }
}
