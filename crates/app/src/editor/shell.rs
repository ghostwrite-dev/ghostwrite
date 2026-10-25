//! `:sh <command>`: run it in the project, in the background. Editing goes
//! on as usual; the status line says it started and how it ended. Claude is
//! told the last command and its output with every request, and `:output`
//! goes through its errors (or pages) once it's done.

use std::collections::VecDeque;

use futures::StreamExt as _;
use ghostwrite_project::shell::{Run, exit_label, locations};
use gpui::{AppContext as _, Context, Task};

use super::Editor;

/// Bytes of output Claude is told: the end, where the errors usually are.
const MAX_CONTEXT: usize = 12_000;
/// Lines of output kept (the last ones), so a server left running can't fill memory.
const MAX_LINES: usize = 10_000;

pub(super) struct Shell {
    command: String,
    /// The last [`MAX_LINES`] lines, and how many came before them.
    output: VecDeque<String>,
    dropped: usize,
    /// `None` while it runs; then its exit code (`None`: stopped).
    exit: Option<Option<i32>>,
    /// The process, while it runs.
    run: Option<Run>,
    _task: Task<()>,
}

impl Drop for Shell {
    fn drop(&mut self) {
        self.stop();
    }
}

impl Shell {
    pub(super) fn running(&self) -> bool {
        self.exit.is_none()
    }

    fn stop(&self) {
        if let Some(run) = &self.run {
            run.stop();
        }
    }

    /// What Claude is told: the command, how it went, and the end of its output.
    pub(super) fn context(&self) -> String {
        let state = self.exit.map_or("still running".to_string(), exit_label);
        let mut tail = vec![];
        let mut bytes = 0;
        let mut lines = self.output.iter().rev();
        loop {
            let Some(line) = lines.next() else {
                if self.dropped > 0 {
                    tail.push("… (earlier output cut)");
                }
                break;
            };
            bytes += line.len() + 1;
            if bytes > MAX_CONTEXT {
                tail.push("… (earlier output cut)");
                break;
            }
            tail.push(line);
        }
        tail.reverse();
        let output = if self.output.is_empty() { "(no output)".to_string() } else { tail.join("\n") };
        format!("$ {} ({state})\n{output}", self.command)
    }

    /// The finished command: what it was, its output, and its exit code (`None`: stopped).
    pub(super) fn finished(&self) -> Result<(String, Vec<String>, Option<i32>), String> {
        let Some(exit) = self.exit else { return Err(format!("`{}` is still running (:stop stops it)", self.command)) };
        Ok((self.command.clone(), self.output.iter().cloned().collect(), exit))
    }
}

impl Editor {
    /// `:sh`: start a command in the background (stopping any still running).
    pub(super) fn start_shell(&mut self, command: String, cx: &mut Context<Self>) -> Result<(), String> {
        self.last_shell = Some(command.clone());
        self.shell = None;
        let run = Run::start(&command, &self.project.root).map_err(|e| format!("couldn't run `{command}`: {e}"))?;
        let (tx, mut rx) = futures::channel::mpsc::unbounded::<Result<String, Option<i32>>>();
        let reader = run.handle();
        cx.background_spawn(async move {
            let lines = tx.clone();
            let code = reader.stream(&mut |line| drop(lines.unbounded_send(Ok(line))));
            tx.unbounded_send(Err(code)).ok();
        })
        .detach();
        let task = cx.spawn(async move |this, cx| {
            while let Some(message) = rx.next().await {
                let done = message.is_err();
                let alive = this.update(cx, |this, cx| this.shell_output(message, cx)).unwrap_or(false);
                if done || !alive {
                    break;
                }
            }
        });
        self.project.say(format!("running `{command}`…"));
        self.shell = Some(Shell { command, output: VecDeque::new(), dropped: 0, exit: None, run: Some(run), _task: task });
        cx.notify();
        Ok(())
    }

    /// A line of output, or the exit. `false` once there's no command to feed.
    fn shell_output(&mut self, message: Result<String, Option<i32>>, cx: &mut Context<Self>) -> bool {
        let Some(shell) = self.shell.as_mut() else { return false };
        match message {
            Ok(line) => {
                if shell.output.len() == MAX_LINES {
                    shell.output.pop_front();
                    shell.dropped += 1;
                }
                shell.output.push_back(line);
            }
            Err(code) => {
                shell.run = None;
                shell.exit = Some(code);
                let text = shell.output.iter().map(String::as_str).collect::<Vec<_>>().join("\n");
                let errors = locations(&text, &self.project.root).len();
                let more = match errors {
                    0 if shell.output.is_empty() => String::new(),
                    0 => " · :output to read it".into(),
                    n => format!(", {n} location{} · :output to go through", if n == 1 { "" } else { "s" }),
                };
                let said = format!("`{}`: {}{more}", shell.command, exit_label(code));
                self.project.say(said);
                cx.notify();
            }
        }
        true
    }

    /// `:stop`: stop the running command.
    pub(super) fn stop_shell(&mut self) -> Result<(), String> {
        match self.shell.as_ref().filter(|s| s.running()) {
            Some(shell) => {
                shell.stop();
                Ok(())
            }
            None => Err("no command is running".into()),
        }
    }
}
