//! The servers Ghostwrite knows, and the ones running for a project.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use futures::channel::mpsc;

use crate::client::{Client, Event, Launch};
use crate::python;

/// A language server Ghostwrite knows how to start. Which languages each
/// serves is in `ghostwrite_core`'s language table, by `name`.
pub struct Spec {
    pub name: &'static str,
    command: &'static [&'static str],
    /// Files that mark the folder it's started in, nearest the file up
    /// (a Lake project's); without them, or none found, the project's.
    roots: &'static [&'static str],
}

const SPECS: &[Spec] = &[
    Spec { name: "rust-analyzer", command: &["rust-analyzer"], roots: &[] },
    Spec { name: "basedpyright", command: &["basedpyright-langserver", "--stdio"], roots: &[] },
    Spec { name: "pyright", command: &["pyright-langserver", "--stdio"], roots: &[] },
    Spec { name: "pylsp", command: &["pylsp"], roots: &[] },
    Spec { name: "typescript-language-server", command: &["typescript-language-server", "--stdio"], roots: &[] },
    Spec { name: "gopls", command: &["gopls"], roots: &[] },
    Spec { name: "clangd", command: &["clangd"], roots: &[] },
    Spec { name: "tinymist", command: &["tinymist"], roots: &[] },
    // Lake picks the project's Lean (elan reads its lean-toolchain) and its packages.
    Spec { name: "lean", command: &["lake", "serve"], roots: &["lakefile.lean", "lakefile.toml", "lean-toolchain"] },
];

const PYTHON_SERVERS: &[&str] = &["basedpyright", "pyright", "pylsp"];

/// Where each server's binary is, looked up once.
fn installed() -> &'static HashMap<&'static str, PathBuf> {
    static FOUND: OnceLock<HashMap<&'static str, PathBuf>> = OnceLock::new();
    FOUND.get_or_init(|| {
        let names: Vec<&str> = SPECS.iter().map(|s| s.command[0]).collect();
        SPECS.iter().zip(python::which_all(&names)).filter_map(|(spec, path)| Some((spec.name, path?))).collect()
    })
}

/// The first of `servers` that's installed.
fn first_installed(servers: &[&str]) -> Option<&'static Spec> {
    servers.iter().find_map(|name| SPECS.iter().find(|spec| spec.name == *name && installed().contains_key(spec.name)))
}

/// Look up the servers and the login shell's environment now, off the UI
/// thread, so the first file opened doesn't wait on the shell.
pub fn warm() {
    std::thread::spawn(|| {
        installed();
        python::login_env();
    });
}

/// How many times a server that keeps stopping is started again.
const RESTARTS: u32 = 3;

/// The servers of one project, started on first use, and again if they stop.
pub struct Registry {
    root: PathBuf,
    clients: HashMap<(&'static str, PathBuf), Option<Arc<Client>>>,
    /// How many times each was started again.
    restarts: HashMap<(&'static str, PathBuf), u32>,
    events: mpsc::UnboundedSender<Event>,
    enabled: bool,
}

impl Registry {
    pub fn new(root: PathBuf, events: mpsc::UnboundedSender<Event>) -> Self {
        Self { root, clients: HashMap::new(), restarts: HashMap::new(), events, enabled: true }
    }

    /// No servers at all (tests open .rs and .py files without wanting them).
    pub fn disabled(root: PathBuf) -> Self {
        Self { enabled: false, ..Self::new(root, mpsc::unbounded().0) }
    }

    /// The first installed of `servers` (a language's, most preferred
    /// first) for `path`, starting it if needed.
    pub fn start(&mut self, servers: &[&str], path: &Path) -> Option<Arc<Client>> {
        if !self.enabled {
            return None;
        }
        let spec = first_installed(servers)?;
        let marked = path.ancestors().skip(1).find(|dir| spec.roots.iter().any(|marker| dir.join(marker).is_file()));
        let root = marked.unwrap_or(&self.root).to_path_buf();
        let key = (spec.name, root.clone());
        // One that stopped is started again, a few times.
        if self.clients.get(&key).is_some_and(|c| c.as_ref().is_some_and(|c| !c.alive())) {
            let restarts = self.restarts.entry(key.clone()).or_default();
            if *restarts >= RESTARTS {
                return None;
            }
            *restarts += 1;
            self.clients.remove(&key);
        }
        self.clients
            .entry(key)
            .or_insert_with(|| {
                let settings_root = root.clone();
                // The login shell's environment; a Python server's has the project's virtualenv on top.
                let mut env = python::login_env().to_vec();
                if PYTHON_SERVERS.contains(&spec.name) {
                    env.extend(python::server_env(&root));
                }
                let launch = Launch {
                    name: spec.name,
                    binary: installed()[spec.name].clone(),
                    args: &spec.command[1..],
                    root: &root,
                    env,
                    settings: Box::new(move |section| python::settings(section, &settings_root)),
                };
                Client::start(launch, self.events.clone()).inspect_err(|e| eprintln!("couldn't start {}: {e}", spec.name)).ok()
            })
            .clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_language_server_is_known() {
        for language in &ghostwrite_core::language::LANGUAGES {
            for server in language.servers {
                assert!(SPECS.iter().any(|s| s.name == *server), "{} names {server}", language.name);
            }
        }
    }
}
