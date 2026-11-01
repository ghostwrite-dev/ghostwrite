//! Asking the local model: it runs in a worker, this same program started again
//! (see `ghostwrite_model`), which holds its memory only while it's used. Here's
//! the app's end of that: starting it, asking it, and ending it. The worker ends
//! by itself when the app does, or after a while idle; the next request starts
//! another.

use std::io::{BufRead as _, BufReader, Write as _};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Mutex, TryLockError};
use std::time::Duration;

use serde_json::{Value, json};

use crate::backend::Listener;

/// How often a request looks to see if it's been stopped.
const CHECK: Duration = Duration::from_millis(20);

static WORKER: Mutex<Option<Worker>> = Mutex::new(None);

/// What a model is called: `user/Name-GGUF:Q4_K_M` or `path/Name.gguf` is `Name`.
pub fn short_name(model: &str) -> &str {
    let name = model.split(':').next().unwrap_or(model);
    let name = name.rsplit('/').next().unwrap_or(name);
    name.strip_suffix("-GGUF").or_else(|| name.strip_suffix(".gguf")).unwrap_or(name)
}

/// Start loading `model`, if it isn't: it's about to be asked.
pub fn warm(model: &str) {
    let mut worker = WORKER.lock().unwrap_or_else(|e| e.into_inner());
    ensure(&mut worker, model).ok();
}

/// End the worker at once: the model isn't to be asked now, so its memory goes back.
pub fn unload() {
    WORKER.lock().unwrap_or_else(|e| e.into_inner()).take();
}

/// Ask `model`, its reply going to `listener`. Blocks, and a request ahead of
/// this one finishes first: it's one model.
pub fn ask(model: &str, request: Value, listener: &mut dyn Listener, cancel: &AtomicBool) -> Result<(), String> {
    let mut slot = loop {
        match WORKER.try_lock() {
            Ok(slot) => break slot,
            Err(TryLockError::Poisoned(e)) => break e.into_inner(),
            Err(TryLockError::WouldBlock) if cancel.load(Ordering::SeqCst) => return Ok(()),
            Err(TryLockError::WouldBlock) => std::thread::sleep(CHECK),
        }
    };
    ensure(&mut slot, model)?;
    let Some(worker) = slot.as_mut() else { return Err("the local model didn't start".into()) };
    let result = worker.ask(request, listener, cancel);
    if !worker.alive() {
        slot.take();
    }
    result
}

/// A worker for `model` in the slot, the one there already if it's still up.
fn ensure(slot: &mut Option<Worker>, model: &str) -> Result<(), String> {
    if !slot.as_mut().is_some_and(|worker| worker.model == model && worker.alive()) {
        *slot = None;
        *slot = Some(Worker::start(model, &program()?)?);
    }
    Ok(())
}

/// This program, unless a test says otherwise.
fn program() -> Result<PathBuf, String> {
    match std::env::var_os("GHOSTWRITE_MODEL_WORKER") {
        Some(path) => Ok(path.into()),
        None => std::env::current_exe().map_err(|e| e.to_string()),
    }
}

fn log_path() -> PathBuf {
    std::env::temp_dir().join("ghostwrite-model.log")
}

fn last_logged() -> String {
    let log = std::fs::read_to_string(log_path()).unwrap_or_default();
    log.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("it said nothing").trim().to_string()
}

struct Worker {
    model: String,
    child: Child,
    stdin: ChildStdin,
    incoming: Receiver<Value>,
    asked: u64,
}

impl Worker {
    fn start(model: &str, program: &std::path::Path) -> Result<Self, String> {
        let log = std::fs::File::create(log_path()).map_err(|e| e.to_string())?;
        let mut child = ghostwrite_project::platform::command(program)
            .args(["--model-worker", model])
            .current_dir(std::env::temp_dir())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(log)
            .spawn()
            .map_err(|e| format!("couldn't start the local model: {e}"))?;
        let stdin = child.stdin.take().ok_or("no stdin")?;
        let stdout = child.stdout.take().ok_or("no stdout")?;
        let (messages, incoming) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if let Ok(message) = serde_json::from_str::<Value>(&line)
                    && messages.send(message).is_err()
                {
                    break;
                }
            }
        });
        Ok(Self { model: model.to_string(), child, stdin, incoming, asked: 0 })
    }

    fn alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    fn send(&mut self, message: &Value) -> Result<(), String> {
        writeln!(self.stdin, "{message}").and_then(|()| self.stdin.flush()).map_err(|_| format!("the local model stopped: {}", last_logged()))
    }

    fn ask(&mut self, mut request: Value, listener: &mut dyn Listener, cancel: &AtomicBool) -> Result<(), String> {
        self.asked += 1;
        request["id"] = json!(self.asked);
        self.send(&request)?;
        let mut cancelled = false;
        loop {
            let message = match self.incoming.recv_timeout(CHECK) {
                Ok(message) => message,
                Err(RecvTimeoutError::Timeout) => {
                    // Told once; it answers `done`, and is ready for the next.
                    if !cancelled && cancel.load(Ordering::SeqCst) {
                        cancelled = true;
                        self.send(&json!({ "cancel": self.asked }))?;
                    }
                    continue;
                }
                Err(RecvTimeoutError::Disconnected) if cancelled => return Ok(()),
                Err(RecvTimeoutError::Disconnected) => return Err(format!("the local model stopped: {}", last_logged())),
            };
            if let Some(status) = message["status"].as_str() {
                listener.looking(status.to_string());
            } else if let Some(text) = message["text"].as_str() {
                if !cancelled {
                    listener.text(text);
                }
            } else if message["done"] == true {
                return Ok(());
            } else if let Some(why) = message["error"].as_str() {
                return Err(why.to_string());
            } else if let Some(why) = message["fatal"].as_str() {
                return Err(why.to_string());
            }
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.child.kill().ok();
        self.child.wait().ok();
    }
}

// They stand in for the worker with a shell script.
#[cfg(all(test, unix))]
mod tests {
    use std::time::Instant;

    use super::*;

    #[derive(Default)]
    struct Collect(String, Vec<String>);

    impl Listener for Collect {
        fn text(&mut self, chunk: &str) {
            self.0.push_str(chunk);
        }
        fn looking(&mut self, what: String) {
            self.1.push(what);
        }
    }

    /// A stand-in for the worker: this shell script.
    fn stand_in(name: &str, body: &str) -> Worker {
        let path = ghostwrite_project::scratch_dir(name).join("worker");
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::process::Command::new("chmod").arg("+x").arg(&path).status().unwrap();
        Worker::start("the model", &path).unwrap()
    }

    #[test]
    fn it_is_called_by_the_models_name() {
        assert_eq!(short_name("bartowski/Qwen2.5-Coder-14B-Instruct-GGUF:Q4_K_M"), "Qwen2.5-Coder-14B-Instruct");
        assert_eq!(short_name("user/Model"), "Model");
        assert_eq!(short_name("/models/tiny.gguf"), "tiny");
    }

    #[test]
    fn a_reply_streams_with_what_it_is_doing() {
        let mut worker = stand_in(
            "worker-reply",
            r#"echo '{"status":"loading it"}'
while read line; do echo '{"text":"hel"}'; echo '{"text":"lo"}'; echo '{"done":true}'; done"#,
        );
        let mut out = Collect::default();
        worker.ask(json!({}), &mut out, &AtomicBool::new(false)).unwrap();
        assert_eq!((out.0.as_str(), out.1.as_slice()), ("hello", ["loading it".to_string()].as_slice()));
        let mut again = Collect::default();
        worker.ask(json!({}), &mut again, &AtomicBool::new(false)).unwrap();
        assert_eq!(again.0, "hello", "the same worker goes on");
    }

    #[test]
    fn what_goes_wrong_is_said() {
        let mut worker = stand_in("worker-error", r#"while read line; do echo '{"error":"out of room"}'; done"#);
        assert_eq!(worker.ask(json!({}), &mut Collect::default(), &AtomicBool::new(false)).unwrap_err(), "out of room");
        let mut worker = stand_in("worker-fatal", r#"echo '{"fatal":"memory is nearly full"}'; exit 1"#);
        assert_eq!(worker.ask(json!({}), &mut Collect::default(), &AtomicBool::new(false)).unwrap_err(), "memory is nearly full");
        let mut worker = stand_in("worker-crash", "read line; exit 3");
        let error = worker.ask(json!({}), &mut Collect::default(), &AtomicBool::new(false)).unwrap_err();
        assert!(error.contains("stopped"), "{error}");
    }

    #[test]
    fn stopping_tells_it_which_request_and_waits_for_it() {
        let dir = ghostwrite_project::scratch_dir("worker-cancel-dir");
        let told = dir.join("told");
        let mut worker = stand_in(
            "worker-cancel",
            &format!(r#"read line; echo '{{"text":"a"}}'; read cancel; echo "$cancel" > {}; echo '{{"done":true}}'; sleep 100"#, told.display()),
        );
        let cancel = AtomicBool::new(true);
        let mut out = Collect::default();
        worker.ask(json!({}), &mut out, &cancel).unwrap();
        assert_eq!(std::fs::read_to_string(&told).unwrap().trim(), r#"{"cancel":1}"#);
    }

    #[cfg(unix)]
    #[test]
    fn ending_the_worker_ends_it_at_once() {
        let dir = ghostwrite_project::scratch_dir("worker-end-dir");
        let pid_file = dir.join("pid");
        let worker = stand_in("worker-end", &format!("trap '' TERM\necho $$ > {}\nexec sleep 100", pid_file.display()));
        let start = Instant::now();
        while !std::fs::read_to_string(&pid_file).is_ok_and(|s| s.ends_with('\n')) {
            assert!(start.elapsed() < Duration::from_secs(10));
            std::thread::sleep(Duration::from_millis(10));
        }
        let pid: libc::pid_t = std::fs::read_to_string(&pid_file).unwrap().trim().parse().unwrap();
        // SAFETY: signal 0 only asks whether the process is there.
        assert_eq!(unsafe { libc::kill(pid, 0) }, 0);
        drop(worker);
        // SAFETY: as above; it's been waited for, so it's gone, not a zombie.
        assert_ne!(unsafe { libc::kill(pid, 0) }, 0, "even one that ignores a polite signal");
    }
}
