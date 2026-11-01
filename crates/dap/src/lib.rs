//! A debug adapter, as a language server is a language client's: a process that's
//! talked to in messages, here the Debug Adapter Protocol. Requests block, so they're
//! made off the UI thread, and what it says of its own accord comes as [`Event`]s.
//!
//! Getting started takes an order the protocol insists on: `initialize`, then `launch`
//! (whose answer doesn't come yet), then on the `initialized` event the breakpoints,
//! and `configurationDone`, after which `launch` is answered and the program runs.

use std::collections::{BTreeSet, HashMap};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};

/// How long an answer is waited for. Starting a program can take a while.
const PATIENCE: Duration = Duration::from_secs(30);

/// What the debugger says of its own accord.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// The program stopped: why, and in which thread.
    Stopped {
        reason: String,
        thread: i64,
    },
    Continued,
    /// What it printed.
    Output(String),
    /// It's over, with this exit code if that's known.
    Exited(Option<i64>),
    /// The debugger itself is gone.
    Gone,
}

/// A place in the program.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub id: i64,
    pub name: String,
    pub path: Option<PathBuf>,
    /// Zero-based.
    pub line: Option<usize>,
}

/// A variable, and how to look inside it if there's anything to see (non-zero).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Variable {
    pub name: String,
    pub value: String,
    pub children: i64,
}

type Answers = Arc<Mutex<HashMap<u64, Sender<Result<Value, String>>>>>;

pub struct Debugger {
    stdin: Mutex<ChildStdin>,
    next: AtomicU64,
    answers: Answers,
    child: Mutex<Child>,
    /// `initialized` as it comes, for [`Debugger::launch`] to wait for.
    ready: Mutex<Option<std::sync::mpsc::Receiver<()>>>,
}

fn write_message(to: &mut impl Write, message: &Value) -> std::io::Result<()> {
    let body = message.to_string();
    write!(to, "Content-Length: {}\r\n\r\n{body}", body.len())?;
    to.flush()
}

/// The next message, or `None` when there are no more.
fn read_message(from: &mut impl BufRead) -> Option<Value> {
    let mut length = None;
    loop {
        let mut line = String::new();
        if from.read_line(&mut line).ok()? == 0 {
            return None;
        }
        let line = line.trim();
        if line.is_empty() {
            break;
        }
        if let Some(value) = line.strip_prefix("Content-Length:") {
            length = value.trim().parse::<usize>().ok();
        }
    }
    let mut body = vec![0; length?];
    from.read_exact(&mut body).ok()?;
    serde_json::from_slice(&body).ok()
}

/// What an event from the adapter means here, if anything.
fn event_of(message: &Value) -> Option<Event> {
    let body = &message["body"];
    match message["event"].as_str()? {
        "stopped" => Some(Event::Stopped { reason: body["reason"].as_str().unwrap_or("stopped").to_string(), thread: body["threadId"].as_i64().unwrap_or(0) }),
        "continued" => Some(Event::Continued),
        "output" if body["category"] != "telemetry" => Some(Event::Output(body["output"].as_str()?.to_string())),
        "exited" => Some(Event::Exited(body["exitCode"].as_i64())),
        "terminated" => Some(Event::Exited(None)),
        _ => None,
    }
}

impl Debugger {
    /// Start `adapter`, which is the program and its arguments. Events go to `events`.
    pub fn start(adapter: &[String], events: impl Fn(Event) + Send + 'static) -> Result<Self, String> {
        let (program, args) = adapter.split_first().ok_or("no debugger to start")?;
        let mut child = ghostwrite_project::platform::command(program)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("couldn't start {program}: {e}"))?;
        let stdin = child.stdin.take().ok_or("the debugger has no input")?;
        let stdout = child.stdout.take().ok_or("the debugger has no output")?;
        let answers: Answers = Arc::default();
        let (ready, is_ready) = channel();
        let (theirs, stdin) = (answers.clone(), Mutex::new(stdin));
        std::thread::spawn(move || {
            let mut from = BufReader::new(stdout);
            let mut ready = Some(ready);
            while let Some(message) = read_message(&mut from) {
                match message["type"].as_str() {
                    Some("response") => {
                        let Some(seq) = message["request_seq"].as_u64() else { continue };
                        if let Some(answer) = theirs.lock().unwrap_or_else(|e| e.into_inner()).remove(&seq) {
                            let failed = || message["body"]["error"]["format"].as_str().or(message["message"].as_str()).unwrap_or("it failed").to_string();
                            answer.send(if message["success"] == true { Ok(message["body"].clone()) } else { Err(failed()) }).ok();
                        }
                    }
                    Some("event") if message["event"] == "initialized" => {
                        if let Some(ready) = ready.take() {
                            ready.send(()).ok();
                        }
                    }
                    Some("event") => event_of(&message).into_iter().for_each(&events),
                    _ => {}
                }
            }
            // Whoever's waiting isn't going to be answered.
            theirs.lock().unwrap_or_else(|e| e.into_inner()).clear();
            events(Event::Gone);
        });
        let debugger = Self { stdin, next: AtomicU64::new(1), answers, child: Mutex::new(child), ready: Mutex::new(Some(is_ready)) };
        debugger.request(
            "initialize",
            json!({ "clientID": "ghostwrite", "adapterID": "ghostwrite", "linesStartAt1": true, "columnsStartAt1": true, "pathFormat": "path" }),
        )?;
        Ok(debugger)
    }

    fn send(&self, command: &str, arguments: Value) -> Result<std::sync::mpsc::Receiver<Result<Value, String>>, String> {
        let seq = self.next.fetch_add(1, Ordering::SeqCst);
        let (answer, answered) = channel();
        self.answers.lock().unwrap_or_else(|e| e.into_inner()).insert(seq, answer);
        let message = json!({ "seq": seq, "type": "request", "command": command, "arguments": arguments });
        write_message(&mut *self.stdin.lock().unwrap_or_else(|e| e.into_inner()), &message).map_err(|e| format!("the debugger stopped listening: {e}"))?;
        Ok(answered)
    }

    /// Ask, and wait for the answer. Blocks.
    pub fn request(&self, command: &str, arguments: Value) -> Result<Value, String> {
        self.send(command, arguments)?.recv_timeout(PATIENCE).map_err(|_| format!("the debugger didn't answer `{command}`"))?
    }

    /// Run `program` to its first breakpoint in `breakpoints`, by file and zero-based line. Blocks.
    pub fn launch(&self, program: &Path, args: &[String], cwd: &Path, breakpoints: &HashMap<PathBuf, BTreeSet<usize>>) -> Result<(), String> {
        let launching = self.send("launch", json!({ "program": program, "args": args, "cwd": cwd, "stopOnEntry": false }))?;
        let is_ready = self.ready.lock().unwrap_or_else(|e| e.into_inner()).take().ok_or("this debugger has been launched already")?;
        is_ready.recv_timeout(PATIENCE).map_err(|_| "the debugger didn't get ready".to_string())?;
        for (path, lines) in breakpoints {
            self.set_breakpoints(path, lines)?;
        }
        self.request("configurationDone", json!({}))?;
        launching.recv_timeout(PATIENCE).map_err(|_| "the debugger didn't answer `launch`".to_string())?.map(|_| ())
    }

    /// These are the breakpoints in `path`. Blocks.
    pub fn set_breakpoints(&self, path: &Path, lines: &BTreeSet<usize>) -> Result<(), String> {
        let breakpoints: Vec<Value> = lines.iter().map(|line| json!({ "line": line + 1 })).collect();
        self.request("setBreakpoints", json!({ "source": { "path": path }, "breakpoints": breakpoints })).map(|_| ())
    }

    /// Where `thread` is, innermost first. Blocks.
    pub fn frames(&self, thread: i64) -> Result<Vec<Frame>, String> {
        let body = self.request("stackTrace", json!({ "threadId": thread, "levels": 50 }))?;
        Ok(body["stackFrames"]
            .as_array()
            .map_or(&[][..], Vec::as_slice)
            .iter()
            .map(|frame| Frame {
                id: frame["id"].as_i64().unwrap_or(0),
                name: frame["name"].as_str().unwrap_or("?").to_string(),
                path: frame["source"]["path"].as_str().map(PathBuf::from),
                line: frame["line"].as_u64().filter(|line| *line > 0).map(|line| line as usize - 1),
            })
            .collect())
    }

    /// The variables in view in `frame`: the first scope's, which is the locals. Blocks.
    pub fn locals(&self, frame: i64) -> Result<Vec<Variable>, String> {
        let body = self.request("scopes", json!({ "frameId": frame }))?;
        let Some(reference) = body["scopes"].as_array().and_then(|scopes| scopes.first()).and_then(|scope| scope["variablesReference"].as_i64()) else {
            return Ok(vec![]);
        };
        let body = self.request("variables", json!({ "variablesReference": reference }))?;
        Ok(body["variables"]
            .as_array()
            .map_or(&[][..], Vec::as_slice)
            .iter()
            .map(|variable| Variable {
                name: variable["name"].as_str().unwrap_or("?").to_string(),
                value: variable["value"].as_str().unwrap_or("").to_string(),
                children: variable["variablesReference"].as_i64().unwrap_or(0),
            })
            .collect())
    }

    /// Go on: `continue`, `next`, `stepIn` or `stepOut`. Blocks until it's been told, not until it stops.
    pub fn go(&self, how: &str, thread: i64) -> Result<(), String> {
        self.request(how, json!({ "threadId": thread })).map(|_| ())
    }
}

impl Drop for Debugger {
    fn drop(&mut self) {
        let mut child = self.child.lock().unwrap_or_else(|e| e.into_inner());
        child.kill().ok();
        child.wait().ok();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_go_there_and_back() {
        let mut bytes = vec![];
        write_message(&mut bytes, &json!({ "seq": 1, "text": "é" })).unwrap();
        write_message(&mut bytes, &json!({ "seq": 2 })).unwrap();
        let mut from = std::io::Cursor::new(bytes);
        assert_eq!(read_message(&mut from).unwrap()["text"], "é", "counted in bytes");
        assert_eq!(read_message(&mut from).unwrap()["seq"], 2);
        assert!(read_message(&mut from).is_none());
    }

    #[test]
    fn events_are_read_for_what_they_mean() {
        let event = |value: Value| event_of(&value);
        assert_eq!(
            event(json!({ "event": "stopped", "body": { "reason": "breakpoint", "threadId": 7 } })),
            Some(Event::Stopped { reason: "breakpoint".into(), thread: 7 })
        );
        assert_eq!(event(json!({ "event": "output", "body": { "category": "stdout", "output": "hi\n" } })), Some(Event::Output("hi\n".into())));
        assert_eq!(event(json!({ "event": "output", "body": { "category": "telemetry", "output": "x" } })), None);
        assert_eq!(event(json!({ "event": "exited", "body": { "exitCode": 3 } })), Some(Event::Exited(Some(3))));
        assert_eq!(event(json!({ "event": "module", "body": {} })), None);
    }
}
