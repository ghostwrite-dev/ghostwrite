//! The login shell, for what a Finder-launched app lacks: its PATH (servers
//! run `node`, `go`, `cargo`…) and the rest of its environment.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::OnceLock;

/// What `script` prints, run by the login shell: only
/// that, not what the shell's startup files print before it (an ssh-agent
/// banner, say), which would shift every line.
fn run(script: &str) -> Option<String> {
    // There's no login shell to ask on Windows: what a program gets is what's set for the user.
    if cfg!(windows) {
        return None;
    }
    const MARK: &str = "\u{1e}ghostwrite\u{1e}";
    let shell =
        std::env::var("SHELL").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| if cfg!(target_os = "macos") { "/bin/zsh".into() } else { "/bin/sh".into() });
    let mut command = Command::new(shell);
    command.args(["-lc", &format!("printf '%s' '{MARK}'; {script}")]).stderr(Stdio::null());
    let out = command.output().ok()?;
    let out = String::from_utf8_lossy(&out.stdout);
    Some(out.rsplit_once(MARK)?.1.to_string())
}

/// The login shell's environment, read once.
pub(crate) fn env() -> &'static [(String, String)] {
    static ENV: OnceLock<Vec<(String, String)>> = OnceLock::new();
    ENV.get_or_init(|| {
        if cfg!(windows) {
            return std::env::vars().collect();
        }
        let out = run("env -0").unwrap_or_default();
        out.split('\0').filter_map(|pair| pair.split_once('=')).map(|(k, v)| (k.to_string(), v.to_string())).collect()
    })
}

/// `command -v` for each name through the login shell: the absolute path, or `None`.
pub(crate) fn which_all(names: &[&str]) -> Vec<Option<PathBuf>> {
    if cfg!(windows) {
        let path = std::env::var_os("PATH").unwrap_or_default();
        return names.iter().map(|name| ghostwrite_project::platform::on_path(name, &path)).collect();
    }
    let script: String = names.iter().map(|n| format!("command -v {n} 2>/dev/null || echo; ")).collect();
    let out = run(&script).unwrap_or_default();
    let mut lines = out.lines();
    names.iter().map(|_| lines.next().filter(|p| p.starts_with('/')).map(PathBuf::from)).collect()
}
