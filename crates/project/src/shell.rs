//! `:sh`: running a command in the project through the user's login shell,
//! and finding the places in the code its output points at.

use std::io::{BufRead as _, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use regex::Regex;

/// The user's login shell: a Finder-launched app still gets their PATH through it.
pub fn login_shell() -> Command {
    let shell = std::env::var("SHELL").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "/bin/zsh".into());
    let mut command = Command::new(shell);
    command.arg("-lc");
    command
}

/// A running command; dropping it kills the process.
pub struct Run {
    child: Arc<Mutex<Child>>,
    cancel: Arc<AtomicBool>,
}

impl Run {
    /// Start `command` in `dir`, stdout and stderr together.
    pub fn start(command: &str, dir: &Path) -> std::io::Result<Self> {
        let child = login_shell()
            .arg(format!("{{ {command}\n}} 2>&1"))
            .current_dir(dir)
            // The output is read as plain text.
            .env("TERM", "dumb")
            .env("NO_COLOR", "1")
            .env("CARGO_TERM_COLOR", "never")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        Ok(Self { child: Arc::new(Mutex::new(child)), cancel: Arc::default() })
    }

    /// Feed the output to `on_line` until the process exits or is stopped;
    /// returns the exit code. Blocks: run it on a background thread.
    pub fn stream(&self, on_line: &mut dyn FnMut(String)) -> Option<i32> {
        let stdout = self.child.lock().ok()?.stdout.take()?;
        for line in BufReader::new(stdout).lines() {
            if self.cancel.load(Ordering::SeqCst) {
                break;
            }
            let Ok(line) = line else { break };
            on_line(clean_line(&line));
        }
        let mut child = self.child.lock().ok()?;
        if self.cancel.load(Ordering::SeqCst) {
            child.kill().ok();
        }
        child.wait().ok()?.code()
    }

    /// Stop it: the stream ends and the process is killed.
    pub fn stop(&self) {
        self.cancel.store(true, Ordering::SeqCst);
        if let Ok(mut child) = self.child.try_lock() {
            child.kill().ok();
        }
    }

    pub fn handle(&self) -> Self {
        Self { child: self.child.clone(), cancel: self.cancel.clone() }
    }
}

/// A line as it ends up on screen: escape codes removed, and only what's
/// after the last carriage return (progress bars redraw with `\r`).
pub fn clean_line(line: &str) -> String {
    static ANSI: OnceLock<Regex> = OnceLock::new();
    let ansi = ANSI.get_or_init(|| Regex::new(r"\x1b(\[[0-9;?]*[ -/]*[@-~]|\][^\x07]*\x07|.)").expect("valid regex"));
    let line = ansi.replace_all(line, "");
    let line = line.rsplit('\r').find(|part| !part.is_empty()).unwrap_or("");
    line.replace('\t', "    ")
}

/// How a command ended (`None`: it was stopped).
pub fn exit_label(code: Option<i32>) -> String {
    match code {
        Some(0) => "✓ exit 0".into(),
        Some(code) => format!("✗ exit {code}"),
        None => "✗ stopped".into(),
    }
}

/// A place in the code, and what's said about it there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Location {
    pub path: PathBuf,
    /// Zero-based.
    pub line: usize,
    pub message: Vec<String>,
}

/// The errors (and warnings, failures, tracebacks) in `output` that point at
/// files under `root`, in order. Each blank-line-separated block is one stop,
/// at its first reference to an existing file; a Python traceback stops at
/// its deepest frame in the project.
pub fn locations(output: &str, root: &Path) -> Vec<Location> {
    static REFERENCE: OnceLock<Regex> = OnceLock::new();
    static TRACEBACK: OnceLock<Regex> = OnceLock::new();
    let reference = REFERENCE.get_or_init(|| Regex::new(r"(?:^|[\s(\x22'`])((?:[\w.@~+-]+/)*[\w.@+-]+\.[A-Za-z0-9]+):(\d+)(?::\d+)?").expect("valid regex"));
    let traceback = TRACEBACK.get_or_init(|| Regex::new(r#"File "([^"]+)", line (\d+)"#).expect("valid regex"));
    let resolve = |path: &str| {
        let p = Path::new(path.trim_start_matches("./"));
        let full = if p.is_absolute() { p.to_path_buf() } else { root.join(p) };
        (full.starts_with(root) && full.is_file()).then_some(full)
    };
    let mut found: Vec<Location> = vec![];
    let lines: Vec<&str> = output.lines().collect();
    for block in lines.split(|l| l.trim().is_empty()).filter(|b| !b.is_empty()) {
        let text = block.join("\n");
        let python = traceback.captures_iter(&text).filter_map(|c| Some((resolve(&c[1])?, c[2].parse::<usize>().ok()?))).last();
        let hit = python.or_else(|| reference.captures_iter(&text).find_map(|c| Some((resolve(&c[1])?, c[2].parse::<usize>().ok()?))));
        let Some((path, line)) = hit else { continue };
        let line = line.saturating_sub(1);
        if found.iter().any(|l| l.path == path && l.line == line) {
            continue;
        }
        // Start at the first line that says what's wrong, past progress lines like `Compiling …`.
        let telling = ["error", "warning", "panicked", "traceback", "fail", "exception"];
        let start = block.iter().position(|l| telling.iter().any(|w| l.to_lowercase().contains(w))).unwrap_or(0);
        found.push(Location { path, line, message: block[start..].iter().map(|l| l.to_string()).collect() });
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleans_colors_and_progress() {
        assert_eq!(clean_line("\x1b[1;31merror\x1b[0m: bad"), "error: bad");
        assert_eq!(clean_line("Compiling 1/3\rCompiling 2/3\rDone"), "Done");
        assert_eq!(clean_line("a\tb"), "a    b");
    }

    #[test]
    fn runs_and_streams() {
        let run = Run::start("echo one; echo two; exit 3", &std::env::temp_dir()).unwrap();
        let mut lines = vec![];
        assert_eq!(run.stream(&mut |l| lines.push(l)), Some(3));
        assert_eq!(lines, ["one", "two"]);
    }

    #[test]
    fn finds_rust_python_and_generic_locations() {
        let root = crate::scratch_dir("shell");
        std::fs::create_dir_all(root.join("src")).unwrap();
        for f in ["src/main.rs", "app.py", "lib.py"] {
            std::fs::write(root.join(f), "x\n").unwrap();
        }
        let output = "\
   Compiling demo v0.1.0
error[E0308]: mismatched types
  --> src/main.rs:4:9
   |
4  |     let x: i32 = \"a\";

warning: unused variable
 --> src/main.rs:7:5

Traceback (most recent call last):
  File \"/usr/lib/python3/runpy.py\", line 1, in <module>
  File \"app.py\", line 3, in <module>
  File \"lib.py\", line 9, in helper
ValueError: nope

see vendor/other.rs:3 (not a file here)
";
        let found = locations(output, &root);
        let at: Vec<(String, usize)> = found.iter().map(|l| (l.path.strip_prefix(&root).unwrap().display().to_string(), l.line)).collect();
        assert_eq!(at, [("src/main.rs".into(), 3), ("src/main.rs".into(), 6), ("lib.py".into(), 8)]);
        assert_eq!(found[0].message[0], "error[E0308]: mismatched types");
        assert!(found[2].message.last().unwrap().contains("ValueError"));
    }
}
