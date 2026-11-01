//! The servers Ghostwrite knows, and the ones running for a project.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock, RwLock};

use futures::channel::mpsc;

use serde_json::Value;

use crate::client::{Client, Event, Launch};
use crate::{python, shell};

/// A language server Ghostwrite knows how to start. Which languages each
/// serves is in `ghostwrite_core`'s language table, by `name`; what's
/// particular to a server is here, as its hooks.
pub struct Spec {
    pub name: &'static str,
    pub command: &'static [&'static str],
    /// Files that mark the folder it's started in, nearest the file up
    /// (a build tool's, say); without them, or none found, the project's.
    pub roots: &'static [&'static str],
    /// Environment variables it starts with, for the folder it's started
    /// in, over the login shell's.
    pub env: fn(&Path) -> Vec<(String, String)>,
    /// Its answer to `workspace/configuration` for a section, in that folder.
    pub settings: fn(&str, &Path) -> Value,
}

impl Spec {
    pub const DEFAULT: Spec = Spec { name: "", command: &[], roots: &[], env: |_| vec![], settings: |_, _| Value::Null };
}

/// Python's servers see the project's virtualenv, and are told its interpreter.
const PYTHON: Spec = Spec { env: python::server_env, settings: python::settings, ..Spec::DEFAULT };

const SPECS: &[Spec] = &[
    Spec { name: "rust-analyzer", command: &["rust-analyzer"], ..Spec::DEFAULT },
    Spec { name: "basedpyright", command: &["basedpyright-langserver", "--stdio"], ..PYTHON },
    Spec { name: "pyright", command: &["pyright-langserver", "--stdio"], ..PYTHON },
    Spec { name: "pylsp", command: &["pylsp"], ..PYTHON },
    Spec { name: "typescript-language-server", command: &["typescript-language-server", "--stdio"], ..Spec::DEFAULT },
    Spec { name: "gopls", command: &["gopls"], ..Spec::DEFAULT },
    Spec { name: "clangd", command: &["clangd"], ..Spec::DEFAULT },
];

/// Servers plugins added.
static REGISTERED: RwLock<Vec<&'static Spec>> = RwLock::new(Vec::new());

/// Add a server a plugin knows.
pub fn register(spec: &'static Spec) {
    REGISTERED.write().unwrap_or_else(|e| e.into_inner()).push(spec);
}

/// A server a plugin knew is gone (those running go on until they're done with).
pub fn unregister(spec: &'static Spec) {
    REGISTERED.write().unwrap_or_else(|e| e.into_inner()).retain(|known| !std::ptr::eq(*known, spec));
}

/// Every server.
fn specs() -> Vec<&'static Spec> {
    SPECS.iter().chain(REGISTERED.read().unwrap_or_else(|e| e.into_inner()).iter().copied()).collect()
}

/// Where each server's binary is, looked up once.
fn installed() -> &'static HashMap<&'static str, PathBuf> {
    static FOUND: OnceLock<HashMap<&'static str, PathBuf>> = OnceLock::new();
    FOUND.get_or_init(|| {
        let specs = specs();
        let names: Vec<&str> = specs.iter().map(|s| s.command[0]).collect();
        specs.iter().zip(shell::which_all(&names)).filter_map(|(spec, path)| Some((spec.name, path?))).collect()
    })
}

/// The first of `servers` that's installed.
fn first_installed(servers: &[&str]) -> Option<&'static Spec> {
    servers.iter().find_map(|name| specs().into_iter().find(|spec| spec.name == *name && installed().contains_key(spec.name)))
}

/// Look up the servers and the login shell's environment now, off the UI
/// thread, so the first file opened doesn't wait on the shell.
pub fn warm() {
    std::thread::spawn(|| {
        installed();
        shell::env();
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
                let (settings, settings_root) = (spec.settings, root.clone());
                let launch = Launch {
                    name: spec.name,
                    binary: installed()[spec.name].clone(),
                    args: &spec.command[1..],
                    root: &root,
                    env: [shell::env().to_vec(), (spec.env)(&root)].concat(),
                    settings: Box::new(move |section| settings(section, &settings_root)),
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
