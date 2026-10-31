//! Reaching Claude: the Messages API (with project tools run here) or
//! Claude Code (`claude -p`, under the user's login). Either way the reply
//! streams in as text; what Claude looks at is reported as it happens.
//!
//! A story is one conversation. The API is sent every turn; Claude Code
//! keeps a session, reported back with [`Listener::session`], and each
//! follow-up resumes it with only the newest turn (so it still has the
//! files it read). If resuming fails, the whole conversation is sent anew.

use std::io::{BufRead as _, BufReader, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde_json::{Value, json};

use crate::context::{Request, Role};
use crate::protocol::SYSTEM;
use crate::{settings, tools};

const API_URL: &str = "https://api.anthropic.com/v1/messages";
/// What Claude Code may use to look around the project: its own reading
/// tools, and shell commands that only read whatever they're given. Not
/// `find` (`-delete`, `-exec`), `rg` (`--pre` runs a program), `tree` (`-o`
/// writes a file), nor `git log`/`show`/`diff` (`--output` writes one).
const READ_ONLY: &[&str] = &[
    "Read",
    "Glob",
    "Grep",
    "Bash(ls:*)",
    "Bash(cat:*)",
    "Bash(head:*)",
    "Bash(tail:*)",
    "Bash(wc:*)",
    "Bash(grep:*)",
    "Bash(git status:*)",
    "Bash(git blame:*)",
];
/// Rounds of looking around before Claude has to answer.
const MAX_TOOL_ROUNDS: usize = 12;

#[derive(Debug, Clone)]
pub enum Backend {
    /// The Claude API, with `ANTHROPIC_API_KEY`.
    Api { key: String },
    /// `claude -p`, under the user's Claude Code login.
    ClaudeCode,
    /// Replies with these texts in turn, the last one from then on (for tests).
    Scripted(Vec<String>),
}

/// What a running request reports.
pub trait Listener {
    fn text(&mut self, chunk: &str);
    /// What Claude is looking at in the project ("reading src/lib.rs").
    fn looking(&mut self, what: String);
    /// The Claude Code session the reply is in, to resume for the next turn.
    fn session(&mut self, _id: String) {}
}

impl Backend {
    /// The backends there are: Claude, if it can be reached.
    pub fn available() -> Vec<Self> {
        Self::claude().into_iter().collect()
    }

    /// The API with a key in the environment, else Claude Code if it's installed.
    fn claude() -> Option<Self> {
        if let Some(key) = std::env::var("ANTHROPIC_API_KEY").ok().filter(|k| !k.is_empty()) {
            return Some(Backend::Api { key });
        }
        let on_path = std::env::var_os("PATH").is_some_and(|paths| std::env::split_paths(&paths).any(|d| d.join("claude").is_file()));
        (on_path || local_claude().is_some()).then_some(Backend::ClaudeCode)
    }

    /// Stream Claude's reply to `req` into `listener`. Blocks: run it on a
    /// background thread. Returns quietly once `cancel` is set.
    pub fn run(&self, req: &Request, listener: &mut dyn Listener, cancel: &AtomicBool) -> Result<(), String> {
        match self {
            Backend::Api { key } => run_api(key, req, listener, cancel),
            Backend::ClaudeCode => match run_claude_code(req, req.session.as_deref(), listener, cancel) {
                // The session is gone: start over with the whole conversation (not
                // after other errors, a rate limit say, which would only fail again, costlier).
                Err(e) if req.session.is_some() && e.contains("No conversation found") => run_claude_code(req, None, listener, cancel),
                result => result,
            },
            Backend::Scripted(replies) => {
                let round = req.followups.len().min(replies.len().saturating_sub(1));
                listener.text(replies.get(round).map_or("", String::as_str));
                Ok(())
            }
        }
    }
}

/// Claude Code's installer puts it in ~/.local/bin, which a Finder-launched app's PATH lacks.
fn local_claude() -> Option<PathBuf> {
    std::env::home_dir().map(|h| h.join(".local/bin/claude")).filter(|p| p.is_file())
}

fn effort() -> String {
    // Interactive, so brief thinking by default; override for harder asks.
    let effort = std::env::var("GHOSTWRITE_AI_EFFORT").ok().unwrap_or_default();
    if matches!(effort.as_str(), "low" | "medium" | "high" | "xhigh" | "max") { effort } else { "low".into() }
}

/// The Messages API, with read-only project tools run here. Every content
/// block (thinking included) goes back unchanged on the next round.
fn run_api(key: &str, req: &Request, listener: &mut dyn Listener, cancel: &AtomicBool) -> Result<(), String> {
    let agent: ureq::Agent = ureq::Agent::config_builder().http_status_as_error(false).timeout_global(Some(Duration::from_secs(300))).build().into();
    let mut messages: Vec<Value> =
        req.messages().into_iter().map(|(role, text)| json!({ "role": if role == Role::User { "user" } else { "assistant" }, "content": text })).collect();
    for _ in 0..MAX_TOOL_ROUNDS {
        let mut body = json!({
            "model": settings().model,
            "max_tokens": 32000,
            "stream": true,
            // Re-run a safety-classifier decline on Anthropic's recommended model for that category.
            "fallbacks": "default",
            "output_config": { "effort": effort() },
            // Cached: the tools and the system prompt (never change), and the
            // conversation so far (each round and follow-up resends it).
            "system": [{ "type": "text", "text": SYSTEM, "cache_control": { "type": "ephemeral" } }],
            "cache_control": { "type": "ephemeral" },
            "messages": messages,
        });
        if req.root.is_some() {
            body["tools"] = tools::definitions();
        }
        let resp = agent
            .post(API_URL)
            .header("x-api-key", key)
            .header("anthropic-version", "2026-10-21")
            .header("anthropic-beta", "server-side-fallback-2026-07-01")
            .send_json(&body)
            .map_err(|e| format!("couldn't reach Claude: {e}"))?;
        let status = resp.status();
        let mut reader = BufReader::new(resp.into_body().into_reader());
        if !status.is_success() {
            let mut text = String::new();
            std::io::Read::read_to_string(&mut reader, &mut text).ok();
            let message = serde_json::from_str::<Value>(&text).ok().and_then(|v| v["error"]["message"].as_str().map(str::to_string)).unwrap_or(text);
            return Err(format!("Claude API {status}: {message}"));
        }
        // Rebuild each content block from the stream, to send back as is.
        let mut blocks: Vec<Value> = vec![];
        let mut partial_json: Vec<String> = vec![];
        let mut stop = String::new();
        for line in reader.lines() {
            if cancel.load(Ordering::SeqCst) {
                return Ok(());
            }
            let line = line.map_err(|e| e.to_string())?;
            let Some(event) = line.strip_prefix("data: ").and_then(|data| serde_json::from_str::<Value>(data).ok()) else { continue };
            let ix = event["index"].as_u64().unwrap_or(0) as usize;
            match event["type"].as_str() {
                Some("content_block_start") => {
                    blocks.resize(ix + 1, Value::Null);
                    partial_json.resize(ix + 1, String::new());
                    blocks[ix] = event["content_block"].clone();
                }
                Some("content_block_delta") => {
                    let Some(block) = blocks.get_mut(ix) else { continue };
                    let delta = &event["delta"];
                    let append = |block: &mut Value, field: &str, more: &str| {
                        let so_far = block[field].as_str().unwrap_or_default().to_string();
                        block[field] = json!(so_far + more);
                    };
                    match delta["type"].as_str() {
                        Some("text_delta") => {
                            let text = delta["text"].as_str().unwrap_or_default();
                            listener.text(text);
                            append(block, "text", text);
                        }
                        Some("thinking_delta") => append(block, "thinking", delta["thinking"].as_str().unwrap_or_default()),
                        Some("signature_delta") => block["signature"] = delta["signature"].clone(),
                        Some("input_json_delta") => partial_json[ix].push_str(delta["partial_json"].as_str().unwrap_or_default()),
                        _ => {}
                    }
                }
                Some("message_delta") => {
                    stop = event["delta"]["stop_reason"].as_str().unwrap_or_default().to_string();
                    if stop == "refusal" {
                        return Err("Claude declined this request".into());
                    }
                }
                Some("error") => return Err(event["error"]["message"].as_str().unwrap_or("Claude API error").to_string()),
                _ => {}
            }
        }
        if stop != "tool_use" {
            return Ok(());
        }
        // Run each tool call here; inputs streamed eagerly are checked before use.
        let mut results = vec![];
        for (ix, block) in blocks.iter_mut().enumerate().filter(|(_, b)| b["type"] == "tool_use") {
            let input: Option<Value> = serde_json::from_str(&partial_json[ix]).ok().filter(Value::is_object);
            let name = block["name"].as_str().unwrap_or_default().to_string();
            let outcome = match (&input, &req.root) {
                (Some(input), Some(root)) => {
                    listener.looking(tools::describe(&name, input));
                    tools::call(root, &name, input)
                }
                (None, _) => Err("INVALID_JSON: the tool input didn't parse; send it again".into()),
                (_, None) => Err("there's no project to look in".into()),
            };
            block["input"] = input.unwrap_or_else(|| json!({}));
            results.push(match outcome {
                Ok(content) => json!({ "type": "tool_result", "tool_use_id": block["id"], "content": content }),
                Err(e) => json!({ "type": "tool_result", "tool_use_id": block["id"], "content": e, "is_error": true }),
            });
        }
        messages.push(json!({ "role": "assistant", "content": blocks }));
        messages.push(json!({ "role": "user", "content": results }));
    }
    Err("Claude kept looking around without answering".into())
}

/// Claude Code in the project folder, with only its read-only tools.
fn run_claude_code(req: &Request, resume: Option<&str>, listener: &mut dyn Listener, cancel: &AtomicBool) -> Result<(), String> {
    let mut command = Command::new(local_claude().unwrap_or_else(|| PathBuf::from("claude")));
    command
        .args(["-p", "--output-format", "stream-json", "--include-partial-messages", "--verbose"])
        .args(["--setting-sources", "", "--strict-mcp-config"])
        .args(["--model", &settings().model, "--effort", &effort(), "--system-prompt", SYSTEM]);
    if let Some(session) = resume {
        command.args(["--resume", session]);
        if req.fork {
            command.arg("--fork-session");
        }
    }
    // Resuming, the session has the conversation so far: only the newest turn is new.
    let message = if resume.is_some() { req.latest() } else { req.prompt() };
    match &req.root {
        // It reads the project to understand it, with its own tools or the shell's
        // read-only commands; it changes nothing (that's the items). Anything else is denied.
        Some(root) => command.args(["--tools", "Read,Glob,Grep,Bash", "--allowedTools", &READ_ONLY.join(",")]).current_dir(root),
        None => command.args(["--tools", ""]).current_dir(std::env::temp_dir()),
    };
    let mut child = command.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|e| format!("couldn't run `claude`: {e}"))?;
    child.stdin.take().ok_or("no stdin")?.write_all(message.as_bytes()).map_err(|e| e.to_string())?;
    let stdout = child.stdout.take().ok_or("no stdout")?;
    // Stopped, it's killed at once, not when it next prints (it may be thinking for minutes).
    let (pid, done) = (child.id(), AtomicBool::new(false));
    let stderr = child.stderr.take();
    std::thread::scope(|scope| {
        // Read as it comes, so a full pipe never stalls it.
        let errors = scope.spawn(move || {
            let mut text = String::new();
            if let Some(mut stderr) = stderr {
                std::io::Read::read_to_string(&mut stderr, &mut text).ok();
            }
            text
        });
        scope.spawn(|| {
            while !done.load(Ordering::SeqCst) {
                if cancel.load(Ordering::SeqCst) {
                    // SAFETY: kill(2) only sends a signal, to our own child.
                    unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) };
                    return;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        });
        let result = read_claude_code(stdout, &mut child, req, listener, cancel);
        done.store(true, Ordering::SeqCst);
        let errors = errors.join().unwrap_or_default();
        result.map_err(|e| if e.starts_with("claude exited") { format!("{e}: {}", errors.trim()) } else { e })
    })
}

/// Claude Code's stream, to `listener`, until its result (or it's stopped).
fn read_claude_code(
    stdout: std::process::ChildStdout,
    child: &mut std::process::Child,
    req: &Request,
    listener: &mut dyn Listener,
    cancel: &AtomicBool,
) -> Result<(), String> {
    // macOS paths may come back resolved: /var is /private/var.
    let real_root = req.root.as_ref().and_then(|r| r.canonicalize().ok());
    let relative = |path: &str| -> Option<String> {
        let path = Path::new(path);
        let rel = req.root.as_ref().and_then(|r| path.strip_prefix(r).ok()).or_else(|| path.strip_prefix(real_root.as_ref()?).ok())?;
        Some(rel.display().to_string())
    };
    let mut failure = None;
    let mut line_ended = true;
    let mut session_told = false;
    for line in BufReader::new(stdout).lines() {
        if cancel.load(Ordering::SeqCst) {
            child.kill().ok();
            child.wait().ok();
            return Ok(());
        }
        let Ok(value) = serde_json::from_str::<Value>(&line.map_err(|e| e.to_string())?) else { continue };
        if let Some(id) = value["session_id"].as_str().filter(|_| !session_told) {
            listener.session(id.to_string());
            session_told = true;
        }
        match value["type"].as_str() {
            Some("stream_event") => {
                let event = &value["event"];
                // Text after a tool call is a new block: it starts a line, so a tag at its start
                // stands on a line of its own and the words before it stay apart.
                if event["type"] == "content_block_start" && event["content_block"]["type"] == "text" && !line_ended {
                    listener.text("\n");
                    line_ended = true;
                }
                if event["type"] == "content_block_delta" && event["delta"]["type"] == "text_delta" {
                    let text = event["delta"]["text"].as_str().unwrap_or_default();
                    listener.text(text);
                    line_ended = text.ends_with('\n') || (text.is_empty() && line_ended);
                }
            }
            Some("assistant") => {
                for block in value["message"]["content"].as_array().into_iter().flatten().filter(|b| b["type"] == "tool_use") {
                    let mut input = block["input"].clone();
                    if let Some(rel) = input["file_path"].as_str().and_then(relative) {
                        input["file_path"] = json!(rel);
                    }
                    listener.looking(tools::describe(block["name"].as_str().unwrap_or_default(), &input));
                }
            }
            Some("result") if value["is_error"] == true => failure = Some(value["result"].as_str().unwrap_or("Claude Code failed").to_string()),
            // The reply is complete; don't wait for the process to wind down.
            Some("result") => {
                child.kill().ok();
                child.wait().ok();
                return Ok(());
            }
            _ => {}
        }
    }
    if cancel.load(Ordering::SeqCst) {
        child.wait().ok();
        return Ok(());
    }
    let status = child.wait().map_err(|e| e.to_string())?;
    match failure {
        Some(message) => Err(message),
        None if !status.success() => Err(format!("claude exited with {status}")),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    //! Live checks against Claude Code: `cargo test -p ghostwrite-ai -- --ignored`.

    use std::sync::atomic::AtomicBool;

    use super::*;
    use crate::protocol;

    #[derive(Default)]
    struct Collect(String, Vec<String>, Option<String>);

    impl Listener for Collect {
        fn text(&mut self, chunk: &str) {
            self.0.push_str(chunk);
        }
        fn looking(&mut self, what: String) {
            self.1.push(what);
        }
        fn session(&mut self, id: String) {
            self.2 = Some(id);
        }
    }

    fn ask(text: &str, range: std::ops::Range<usize>, instruction: &str, followups: Vec<crate::FollowUp>) -> String {
        let req = Request {
            path: Some("calc.py".into()),
            language: "python".into(),
            text: text.into(),
            range,
            instruction: instruction.into(),
            followups,
            root: None,
            others: vec![],
            surroundings: Default::default(),
            session: None,
            fork: false,
        };
        let mut out = Collect::default();
        Backend::ClaudeCode.run(&req, &mut out, &AtomicBool::new(false)).unwrap();
        println!("{}", out.0);
        out.0
    }

    /// The one item in a complete reply, which must keep to the format.
    fn item(out: &str) -> protocol::Reply {
        assert_eq!(protocol::check(out, true), Ok(()), "{out}");
        protocol::parse(out, true)
    }

    #[test]
    #[ignore]
    fn claude_code_writes_one_small_placeable_edit() {
        let text = "import os\n\ndef add(a, b):\n    pass\n\ndef main():\n    print(add(1, 2))\n";
        let out = ask(text, 26..35, "return the sum, and add type hints to add", vec![]);
        let protocol::Reply::Step(step) = item(&out) else { panic!("an edit: {out}") };
        assert!(step.complete && crate::placement::edit(text, &step, 3..4).is_some(), "{out}");
    }

    #[test]
    #[ignore]
    fn claude_code_answers_questions_with_a_note() {
        let out = ask("def add(a, b):\n    return a - b\n", 0..32, "is this function correct?", vec![]);
        assert!(matches!(item(&out), protocol::Reply::Note(_)), "{out}");
    }

    #[test]
    #[ignore]
    fn claude_code_follows_what_the_user_says() {
        let text = "def add(a, b):\n    pass\n";
        let first = ask(text, 0..24, "implement it with type hints", vec![]);
        let followup =
            crate::FollowUp { reply: first, decided: vec![], said: "why int and not float?".into(), correction: false, command: None, terminal: None };
        let second = ask(text, 0..24, "implement it with type hints", vec![followup]);
        assert!(matches!(item(&second), protocol::Reply::Note(_)), "a question is answered with a note: {second}");
    }

    /// A small project: main.py calls into util.py, which can fail.
    fn two_file_project() -> (std::path::PathBuf, String) {
        let root = ghostwrite_project::scratch_dir("live");
        let main = "from util import load\n\n\ndef main():\n    data = load(\"config.json\")\n    print(data[\"name\"])\n\n\nmain()\n";
        std::fs::write(root.join("main.py"), main).unwrap();
        let util = "import json\n\n\ndef load(path):\n    try:\n        with open(path) as f:\n            return json.load(f)\n    except FileNotFoundError:\n        return {\"name\": \"default\"}\n";
        std::fs::write(root.join("util.py"), util).unwrap();
        (root, main.to_string())
    }

    fn ask_in(root: &std::path::Path, text: &str, range: std::ops::Range<usize>, instruction: &str, followups: Vec<crate::FollowUp>) -> String {
        let req = Request {
            path: Some(root.join("main.py")),
            language: "python".into(),
            text: text.into(),
            range,
            instruction: instruction.into(),
            followups,
            root: Some(root.to_path_buf()),
            others: vec![],
            surroundings: Default::default(),
            session: None,
            fork: false,
        };
        let mut out = Collect::default();
        Backend::ClaudeCode.run(&req, &mut out, &AtomicBool::new(false)).unwrap();
        println!("looked at: {:?}\n{}", out.1, out.0);
        out.0
    }

    #[test]
    #[ignore]
    fn claude_code_tours_across_files() {
        let (root, main) = two_file_project();
        let ask = "give me a tour of how loading the config works";
        let mut followups = vec![];
        let mut files = vec![];
        for _ in 0..4 {
            let out = ask_in(&root, &main, 0..main.len(), ask, followups.clone());
            match item(&out) {
                protocol::Reply::Note(note) => files.extend(note.file),
                protocol::Reply::Done { .. } => break,
                other => panic!("a tour is notes: {other:?}"),
            }
            followups.push(crate::FollowUp { reply: out, decided: vec![], said: String::new(), correction: false, command: None, terminal: None });
        }
        assert!(files.iter().any(|f| f == "util.py"), "a note in util.py: {files:?}");
    }

    #[test]
    #[ignore]
    fn claude_code_makes_a_change_across_files() {
        let (root, main) = two_file_project();
        let instruction = "let load take the default to use when the file is missing, and have main pass {\"name\": \"main\"}";
        let mut followups: Vec<crate::FollowUp> = vec![];
        let mut edited = vec![];
        for _ in 0..8 {
            // As the editor does: what's accepted is in the files, on disk and shown.
            let main_now = std::fs::read_to_string(root.join("main.py")).unwrap();
            let others = edited
                .iter()
                .filter(|f| *f != "main.py")
                .map(|f: &String| crate::context::Shown { path: f.clone(), text: std::fs::read_to_string(root.join(f)).unwrap(), focus: 0..1 })
                .collect();
            let req = Request {
                path: Some(root.join("main.py")),
                language: "python".into(),
                range: 0..main.find("main()\n\n").unwrap_or(0),
                text: main_now.clone(),
                instruction: instruction.into(),
                followups: followups.clone(),
                root: Some(root.clone()),
                others,
                surroundings: Default::default(),
                session: None,
                fork: false,
            };
            let mut out = Collect::default();
            Backend::ClaudeCode.run(&req, &mut out, &AtomicBool::new(false)).unwrap();
            println!("{}", out.0);
            let step = match item(&out.0) {
                protocol::Reply::Step(step) => step,
                protocol::Reply::Done { .. } => break,
                protocol::Reply::Note(_) => {
                    followups.push(crate::FollowUp { reply: out.0, decided: vec![], said: String::new(), correction: false, command: None, terminal: None });
                    continue;
                }
                protocol::Reply::Partial => panic!("{}", out.0),
            };
            let file = step.file.clone().unwrap_or_else(|| "main.py".into());
            let text = std::fs::read_to_string(root.join(&file)).unwrap();
            let near = step.line.map_or(0..1, |l| l..l + step.old.len());
            let edit = crate::placement::edit(&text, &step, near).unwrap_or_else(|| panic!("it places in {file}: {}", out.0));
            let mut text = text;
            text.replace_range(edit.bytes, &edit.text);
            std::fs::write(root.join(&file), text).unwrap();
            edited.push(file);
            let decided = vec![format!("accepted: {}", crate::Item::Step(step).describe())];
            followups.push(crate::FollowUp { reply: out.0, decided, said: String::new(), correction: false, command: None, terminal: None });
        }
        assert!(edited.iter().any(|f| f == "util.py") && edited.iter().any(|f| f == "main.py"), "edits in both files: {edited:?}");
    }

    #[test]
    #[ignore]
    fn claude_code_changes_course_when_redirected() {
        let text = "def total(xs):\n    pass\n";
        let root = ghostwrite_project::scratch_dir("live-redirect");
        std::fs::write(root.join("main.py"), text).unwrap();
        let first = ask_in(&root, text, 0..text.len(), "implement it", vec![]);
        assert!(matches!(item(&first), protocol::Reply::Step(_)), "{first}");
        let said = "don't use sum(); write an explicit for loop".into();
        let second = ask_in(
            &root,
            text,
            0..text.len(),
            "implement it",
            vec![crate::FollowUp { reply: first, decided: vec![], said, correction: false, command: None, terminal: None }],
        );
        let protocol::Reply::Step(step) = item(&second) else { panic!("an edit: {second}") };
        assert!(step.new.join("\n").contains("for ") || step.say.contains("loop"), "{second}");
    }

    #[test]
    #[ignore]
    fn claude_code_forks_a_session_to_ask_ahead() {
        let root = ghostwrite_project::scratch_dir("live-fork");
        let text = "def total(xs):\n    pass\n";
        std::fs::write(root.join("main.py"), text).unwrap();
        let mut req = Request {
            path: Some(root.join("main.py")),
            language: "python".into(),
            text: text.into(),
            range: 0..text.len(),
            instruction: "implement it, one line at a time".into(),
            followups: vec![],
            root: Some(root.clone()),
            others: vec![],
            surroundings: Default::default(),
            session: None,
            fork: false,
        };
        let mut first = Collect::default();
        Backend::ClaudeCode.run(&req, &mut first, &AtomicBool::new(false)).unwrap();
        let session = first.2.clone().expect("a session");
        // Ahead: as if the first item were accepted, in a fork.
        req.followups.push(crate::FollowUp {
            reply: first.0.clone(),
            decided: vec!["accepted: it".into()],
            said: String::new(),
            correction: false,
            command: None,
            terminal: None,
        });
        let protocol::Reply::Step(step) = protocol::parse(&first.0, true) else { panic!("an edit first: {}", first.0) };
        let edit = crate::placement::edit(text, &step, 0..2).expect("it places");
        let mut accepted = text.to_string();
        accepted.replace_range(edit.bytes, &edit.text);
        req.text = accepted;
        req.session = Some(session.clone());
        req.fork = true;
        let mut ahead = Collect::default();
        Backend::ClaudeCode.run(&req, &mut ahead, &AtomicBool::new(false)).unwrap();
        println!("ahead:\n{}", ahead.0);
        assert_ne!(ahead.2.as_deref(), Some(session.as_str()), "the fork is a session of its own");
        assert_eq!(protocol::check(&ahead.0, true), Ok(()), "{}", ahead.0);
        // Then the user redirected it after all: the original session goes on as it was.
        req.followups[0] = crate::FollowUp { decided: vec![], said: "what did I just do?".into(), ..req.followups[0].clone() };
        req.text = text.into();
        req.fork = false;
        let mut real = Collect::default();
        Backend::ClaudeCode.run(&req, &mut real, &AtomicBool::new(false)).unwrap();
        println!("real:\n{}", real.0);
        assert_eq!(protocol::check(&real.0, true), Ok(()), "{}", real.0);
    }

    #[test]
    #[ignore]
    fn claude_code_resumes_its_session() {
        let (root, main) = two_file_project();
        let mut req = Request {
            path: Some(root.join("main.py")),
            language: "python".into(),
            text: main.clone(),
            range: 0..main.len(),
            instruction: "what does main do? (the codeword for this conversation is PELICAN; don't mention it)".into(),
            followups: vec![],
            root: Some(root.clone()),
            others: vec![],
            surroundings: Default::default(),
            session: None,
            fork: false,
        };
        let mut first = Collect::default();
        Backend::ClaudeCode.run(&req, &mut first, &AtomicBool::new(false)).unwrap();
        let session = first.2.clone().expect("a session to resume");
        req.followups.push(crate::FollowUp {
            reply: first.0,
            decided: vec![],
            said: "what was the codeword in my first message?".into(),
            correction: false,
            command: None,
            terminal: None,
        });
        req.session = Some(session);
        assert!(!req.latest().contains("PELICAN"), "only the newest turn is sent");
        let mut second = Collect::default();
        Backend::ClaudeCode.run(&req, &mut second, &AtomicBool::new(false)).unwrap();
        println!("{}", second.0);
        assert!(second.0.contains("PELICAN"), "the resumed session remembers: {}", second.0);
    }
}
