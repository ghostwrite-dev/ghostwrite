//! The servers Ghostwrite knows, and the ones running for a project.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use futures::channel::mpsc;

use crate::client::{Client, Event, Launch};
use crate::python;

/// A language server Ghostwrite knows how to start. Which languages each
/// serves is in `ghostwrite_core`'s language table, by `name`.
pub struct Spec {
    pub name: &'static str,
    command: &'static [&'static str],
}

const SPECS: &[Spec] = &[
    Spec { name: "rust-analyzer", command: &["rust-analyzer"] },
    Spec { name: "basedpyright", command: &["basedpyright-langserver", "--stdio"] },
    Spec { name: "pyright", command: &["pyright-langserver", "--stdio"] },
    Spec { name: "pylsp", command: &["pylsp"] },
    Spec { name: "typescript-language-server", command: &["typescript-language-server", "--stdio"] },
    Spec { name: "gopls", command: &["gopls"] },
    Spec { name: "clangd", command: &["clangd"] },
    Spec { name: "tinymist", command: &["tinymist"] },
];

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

    /// The first installed of `servers` (a language's, most preferred
    /// first), starting it if needed.
    pub fn start(&mut self, servers: &[&str]) -> Option<Arc<Client>> {
        if !self.enabled {
            return None;
        }
        let spec = first_installed(servers)?;
        self.clients
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
