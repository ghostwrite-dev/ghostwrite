//! What Python's servers (pyright, basedpyright, pylsp) are started with:
//! the project's environment, so they see its packages, and its settings.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use serde_json::{Value, json};

/// The project's environment: `$VIRTUAL_ENV`, or a `.venv` / `venv` in the
/// project or a folder above it.
pub(crate) fn env(root: &Path) -> Option<PathBuf> {
    let active = std::env::var_os("VIRTUAL_ENV").map(PathBuf::from).filter(|p| p.join("bin/python").exists());
    active.or_else(|| root.ancestors().flat_map(|dir| [dir.join(".venv"), dir.join("venv")]).find(|venv| venv.join("bin/python").exists()))
}

/// The Python to analyze with: what `uv run` uses, else the project's
/// environment, else the login shell's `python3` (pyright looks for
/// `python`, which macOS doesn't have).
pub(crate) fn interpreter(root: &Path) -> Option<PathBuf> {
    static UV: OnceLock<Mutex<HashMap<PathBuf, Option<PathBuf>>>> = OnceLock::new();
    static SYSTEM: OnceLock<Option<PathBuf>> = OnceLock::new();
    let absolute = |out: Option<String>| out.map(|s| s.trim().to_string()).filter(|p| p.starts_with('/')).map(PathBuf::from);
    let uv = UV
        .get_or_init(Mutex::default)
        .lock()
        .unwrap()
        .entry(root.to_path_buf())
        .or_insert_with(|| absolute(crate::shell::run("uv python find 2>/dev/null", Some(root))))
        .clone();
    uv.or_else(|| env(root).map(|venv| venv.join("bin/python")))
        .or_else(|| SYSTEM.get_or_init(|| absolute(crate::shell::run("command -v python3", None))).clone())
}

/// The environment variables a server for `root` starts with: the project's
/// virtualenv active, for servers that look for one.
pub(crate) fn server_env(root: &Path) -> Vec<(String, String)> {
    let venv = interpreter(root).and_then(|p| p.parent()?.parent().map(Path::to_path_buf)).filter(|v| v.join("pyvenv.cfg").exists());
    let Some(venv) = venv.or_else(|| env(root)) else { return vec![] };
    let path = crate::shell::path();
    vec![("VIRTUAL_ENV".into(), venv.display().to_string()), ("PATH".into(), format!("{}:{path}", venv.join("bin").display()))]
}

/// Whether the project configures pyright / basedpyright itself.
fn has_pyright_config(root: &Path) -> bool {
    root.join("pyrightconfig.json").exists()
        || std::fs::read_to_string(root.join("pyproject.toml")).is_ok_and(|t| t.contains("[tool.pyright]") || t.contains("[tool.basedpyright]"))
}

/// Answers to `workspace/configuration`, by section: the project's Python,
/// and pyright's "standard" checking unless the project chose its own.
pub(crate) fn settings(section: &str, root: &Path) -> Value {
    let checking = (!has_pyright_config(root)).then_some("standard");
    match section {
        "python" => json!({ "pythonPath": interpreter(root) }),
        "python.analysis" | "basedpyright.analysis" => json!({ "typeCheckingMode": checking }),
        "basedpyright" | "pyright" => json!({ "analysis": { "typeCheckingMode": checking } }),
        _ => Value::Null,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_follow_the_project() {
        let root = std::env::temp_dir().join(format!("ghostwrite-py-{}", std::process::id()));
        std::fs::create_dir_all(root.join("app/.venv/bin")).unwrap();
        std::fs::write(root.join("app/.venv/bin/python"), "").unwrap();
        let app = root.join("app");
        assert_eq!(env(&app.join("pkg")), Some(app.join(".venv")), "found from a folder below");
        assert_eq!(settings("basedpyright.analysis", &app)["typeCheckingMode"], "standard");
        std::fs::write(app.join("pyproject.toml"), "[tool.basedpyright]\ntypeCheckingMode = \"strict\"\n").unwrap();
        assert!(settings("basedpyright.analysis", &app)["typeCheckingMode"].is_null(), "the project's own choice wins");
        std::fs::remove_dir_all(root).ok();
    }
}
