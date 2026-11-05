//! What Python's servers (basedpyright, pyright, pylsp) are started with: the
//! project's environment, so they see its packages, and its settings. The project
//! is mounted at /project; what the editor says of paths is as it knows them.

use std::path::{Path, PathBuf};

use ghostwrite_plugin_api::*;
use serde_json::{Value, json};

/// Where the project is mounted.
const MOUNTED: &str = "/project";

/// `path`, as the editor knows it, where it's mounted here, if it's in the project.
fn mounted(project: &str, path: &Path) -> Option<PathBuf> {
    Some(Path::new(MOUNTED).join(path.strip_prefix(project).ok()?))
}

/// The project's environment: `$VIRTUAL_ENV`, or a `.venv` / `venv` in the folder
/// the server's started in or one above it, in the project.
fn env(project: &str, root: &str) -> Option<PathBuf> {
    let has_python = |venv: &Path| mounted(project, venv).is_some_and(|here| here.join("bin/python").exists());
    // One that's active elsewhere can't be looked in: it's taken at its word.
    let active = std::env::var_os("VIRTUAL_ENV").map(PathBuf::from).filter(|venv| !venv.as_os_str().is_empty());
    let active = active.filter(|venv| mounted(project, venv).is_none() || has_python(venv));
    active.or_else(|| {
        let folders = Path::new(root).ancestors().take_while(|dir| dir.starts_with(project));
        folders.flat_map(|dir| [dir.join(".venv"), dir.join("venv")]).find(|venv| has_python(venv))
    })
}

/// The Python to analyze with: the project's environment's, else `python3` on the
/// server's PATH (pyright looks for `python`, which macOS doesn't have).
fn interpreter(project: &str, root: &str) -> Option<String> {
    let venv = env(project, root).map(|venv| venv.join("bin/python").display().to_string());
    venv.or_else(|| which("python3"))
}

/// Whether the project configures pyright / basedpyright itself.
fn has_pyright_config(project: &str, root: &str) -> bool {
    let Some(root) = mounted(project, Path::new(root)) else { return false };
    root.join("pyrightconfig.json").exists()
        || std::fs::read_to_string(root.join("pyproject.toml")).is_ok_and(|t| t.contains("[tool.pyright]") || t.contains("[tool.basedpyright]"))
}

struct Python;

impl Plugin for Python {
    /// The project's virtualenv, active.
    fn server_env(_server: String, project: String, root: String) -> Vec<(String, String)> {
        let Some(venv) = env(&project, &root) else { return vec![] };
        let bin = venv.join("bin").display().to_string();
        let path = std::env::var("PATH").map_or(bin.clone(), |path| format!("{bin}:{path}"));
        vec![("VIRTUAL_ENV".into(), venv.display().to_string()), ("PATH".into(), path)]
    }

    /// The project's Python, and pyright's "standard" checking unless the project chose its own.
    fn server_settings(_server: String, project: String, root: String, section: String) -> Option<String> {
        let checking = (!has_pyright_config(&project, &root)).then_some("standard");
        let settings = match section.as_str() {
            "python" => json!({ "pythonPath": interpreter(&project, &root) }),
            "python.analysis" | "basedpyright.analysis" => json!({ "typeCheckingMode": checking }),
            "basedpyright" | "pyright" => json!({ "analysis": { "typeCheckingMode": checking } }),
            _ => Value::Null,
        };
        Some(settings.to_string())
    }
}

register!(Python);
