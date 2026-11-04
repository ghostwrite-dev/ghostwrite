//! Claude Codes started ahead of a request, waiting to be taken by one that
//! would start them alike.

#[cfg(unix)]
use std::os::unix::process::CommandExt as _;
use std::path::PathBuf;
use std::process::{Child, Stdio};
use std::sync::Mutex;
use std::time::Duration;

use super::claude_code::stop;
use super::{effort, local_claude};
use crate::protocol;

/// How a Claude Code is started: two started alike are interchangeable.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Startup {
    model: String,
    effort: String,
    root: PathBuf,
    resume: Option<String>,
    fork: bool,
}

impl Startup {
    pub(super) fn new(model: &str, root: Option<&std::path::Path>, resume: Option<&str>, fork: bool) -> Self {
        let root = root.map_or_else(std::env::temp_dir, std::path::Path::to_path_buf);
        Self { model: model.to_string(), effort: effort(), root, resume: resume.map(str::to_string), fork: fork && resume.is_some() }
    }

    pub(super) fn spawn(&self) -> Result<Child, String> {
        let mut command = ghostwrite_project::platform::command(local_claude().unwrap_or_else(|| PathBuf::from("claude")));
        command
            .args(["-p", "--input-format", "stream-json", "--output-format", "stream-json", "--include-partial-messages", "--verbose"])
            .args(["--setting-sources", "", "--strict-mcp-config", "--tools", ""])
            .args(["--model", &self.model, "--effort", &self.effort, "--system-prompt", protocol::system()])
            .current_dir(&self.root);
        if let Some(session) = &self.resume {
            command.args(["--resume", session]);
            if self.fork {
                command.arg("--fork-session");
            }
        }
        // In a process group of its own, so stopping it stops what it started too.
        #[cfg(unix)]
        command.process_group(0);
        command.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|e| format!("couldn't run `claude`: {e}"))
    }
}

/// Claude Codes started before they're asked, waiting on their stdin for the
/// first message: by then what it takes to start (a few tenths of a second)
/// is out of the way. The user has opened the prompt, so a request is likely.
static STARTED: Mutex<Vec<(Startup, Child, std::time::Instant)>> = Mutex::new(Vec::new());
/// The most kept waiting, and for how long: they're cheap, but not free.
const STARTED_KEPT: usize = 2;
const STARTED_FOR: Duration = Duration::from_secs(10 * 60);

/// Start a Claude Code like `startup` to be taken by the next request like it.
pub(super) fn start_ahead(startup: Startup) {
    let mut started = STARTED.lock().unwrap_or_else(|e| e.into_inner());
    // Those gone, or waiting too long, go.
    for (waiting, mut child, at) in std::mem::take(&mut *started) {
        if at.elapsed() < STARTED_FOR && child_running(&mut child) {
            started.push((waiting, child, at));
        } else {
            stop(&Mutex::new(child));
        }
    }
    if started.iter().any(|(waiting, ..)| *waiting == startup) {
        return;
    }
    if let Ok(child) = startup.spawn() {
        started.push((startup, child, std::time::Instant::now()));
    }
    while started.len() > STARTED_KEPT {
        let (_, child, _) = started.remove(0);
        stop(&Mutex::new(child));
    }
}

/// A Claude Code started like `startup` and still waiting, if there is one.
pub(super) fn take_started(startup: &Startup) -> Option<Child> {
    let mut started = STARTED.lock().unwrap_or_else(|e| e.into_inner());
    let ix = started.iter_mut().position(|(waiting, child, at)| waiting == startup && at.elapsed() < STARTED_FOR && child_running(child))?;
    Some(started.remove(ix).1)
}

fn child_running(child: &mut Child) -> bool {
    matches!(child.try_wait(), Ok(None))
}
