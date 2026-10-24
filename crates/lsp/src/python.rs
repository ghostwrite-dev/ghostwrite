//! Finding the project's Python, so pyright & co. see its packages.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};

use serde_json::{Value, json};

fn login_shell(script: &str, dir: Option<&Path>) -> Option<String> {
    let shell = std::env::var("SHELL").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "/bin/zsh".into());
    let mut command = Command::new(shell);
    command.args(["-lc", script]).stderr(Stdio::null());
    if let Some(dir) = dir {
        command.current_dir(dir);
    }
    let out = command.output().ok()?;
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// `command -v` for each name through the login shell (a Finder-launched
/// app has a minimal PATH): the absolute path, or `None`.
pub(crate) fn which_all(names: &[&str]) -> Vec<Option<PathBuf>> {
    let script: String = names.iter().map(|n| format!("command -v {n} 2>/dev/null || echo; ")).collect();
    let out = login_shell(&script, None).unwrap_or_default();
    let mut lines = out.lines();
    names.iter().map(|_| lines.next().filter(|p| p.starts_with('/')).map(PathBuf::from)).collect()
}

/// The project's environment: `$VIRTUAL_ENV`, or a `.venv` / `venv` in the
/// project or a folder above it.
pub fn env(root: &Path) -> Option<PathBuf> {
    let active = std::env::var_os("VIRTUAL_ENV").map(PathBuf::from).filter(|p| p.join("bin/python").exists());
    active.or_else(|| root.ancestors().flat_map(|dir| [dir.join(".venv"), dir.join("venv")]).find(|venv| venv.join("bin/python").exists()))
}

/// The Python to analyze with: what `uv run` uses, else the project's
/// environment, else the login shell's `python3` (pyright looks for
/// `python`, which macOS doesn't have).
pub fn interpreter(root: &Path) -> Option<PathBuf> {
    static UV: OnceLock<Mutex<HashMap<PathBuf, Option<PathBuf>>>> = OnceLock::new();
    static SYSTEM: OnceLock<Option<PathBuf>> = OnceLock::new();
    let absolute = |out: Option<String>| out.map(|s| s.trim().to_string()).filter(|p| p.starts_with('/')).map(PathBuf::from);
    let uv = UV
        .get_or_init(Mutex::default)
        .lock()
        .unwrap()
        .entry(root.to_path_buf())
        .or_insert_with(|| absolute(login_shell("uv python find 2>/dev/null", Some(root))))
        .clone();
    uv.or_else(|| env(root).map(|venv| venv.join("bin/python"))).or_else(|| SYSTEM.get_or_init(|| absolute(login_shell("command -v python3", None))).clone())
}

/// The environment variables a server for `root` starts with: the project's
/// virtualenv active, for servers that look for one.
pub fn server_env(root: &Path) -> Vec<(String, String)> {
    let venv = interpreter(root).and_then(|p| p.parent()?.parent().map(Path::to_path_buf)).filter(|v| v.join("pyvenv.cfg").exists());
    let Some(venv) = venv.or_else(|| env(root)) else { return vec![] };
    let path = std::env::var("PATH").unwrap_or_default();
    vec![("VIRTUAL_ENV".into(), venv.display().to_string()), ("PATH".into(), format!("{}:{path}", venv.join("bin").display()))]
}

/// Whether the project configures pyright / basedpyright itself.
fn has_pyright_config(root: &Path) -> bool {
    root.join("pyrightconfig.json").exists()
        || std::fs::read_to_string(root.join("pyproject.toml")).is_ok_and(|t| t.contains("[tool.pyright]") || t.contains("[tool.basedpyright]"))
}

/// Answers to `workspace/configuration`, by section: the project's Python,
/// and pyright's "standard" checking unless the project chose its own.
pub fn settings(section: &str, root: &Path) -> Value {
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
