//! Reaching Claude: the Messages API (with project tools run here) or
//! Claude Code (`claude -p`, under the user's login). Either way the reply
//! streams in as text; what Claude looks at is reported as it happens.

use std::io::{BufRead as _, BufReader, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde_json::{Value, json};

use crate::context::Request;
use crate::protocol::SYSTEM;
use crate::{MODEL, tools};

const API_URL: &str = "https://api.anthropic.com/v1/messages";
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
}

impl Backend {
    /// The API with a key in the environment, else Claude Code if it's installed.
    pub fn from_env() -> Option<Self> {
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
            Backend::ClaudeCode => run_claude_code(req, listener, cancel),
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
    let mut messages = vec![json!({ "role": "user", "content": req.prompt() })];
    for _ in 0..MAX_TOOL_ROUNDS {
        let mut body = json!({
            "model": MODEL,
            "max_tokens": 32000,
            "stream": true,
            // Re-run a safety-classifier decline on Anthropic's recommended model for that category.
            "fallbacks": "default",
            "output_config": { "effort": effort() },
            "system": SYSTEM,
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
fn run_claude_code(req: &Request, listener: &mut dyn Listener, cancel: &AtomicBool) -> Result<(), String> {
    let mut command = Command::new(local_claude().unwrap_or_else(|| PathBuf::from("claude")));
    command
        .args(["-p", "--output-format", "stream-json", "--include-partial-messages", "--verbose"])
        .args(["--setting-sources", "", "--strict-mcp-config", "--no-session-persistence"])
        .args(["--model", MODEL, "--effort", &effort(), "--system-prompt", SYSTEM]);
    match &req.root {
        // It reads the project to understand it; it changes nothing (that's the steps).
        Some(root) => command.args(["--tools", "Read,Glob,Grep", "--allowedTools", "Read,Glob,Grep"]).current_dir(root),
        None => command.args(["--tools", ""]).current_dir(std::env::temp_dir()),
    };
    let mut child = command.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|e| format!("couldn't run `claude`: {e}"))?;
    child.stdin.take().ok_or("no stdin")?.write_all(req.prompt().as_bytes()).map_err(|e| e.to_string())?;
    let stdout = child.stdout.take().ok_or("no stdout")?;
    // macOS paths may come back resolved: /var is /private/var.
    let real_root = req.root.as_ref().and_then(|r| r.canonicalize().ok());
    let relative = |path: &str| -> Option<String> {
        let path = Path::new(path);
        let rel = req.root.as_ref().and_then(|r| path.strip_prefix(r).ok()).or_else(|| path.strip_prefix(real_root.as_ref()?).ok())?;
        Some(rel.display().to_string())
    };
    let mut failure = None;
    for line in BufReader::new(stdout).lines() {
        if cancel.load(Ordering::SeqCst) {
            child.kill().ok();
            return Ok(());
        }
        let Ok(value) = serde_json::from_str::<Value>(&line.map_err(|e| e.to_string())?) else { continue };
        match value["type"].as_str() {
            Some("stream_event") => {
                let event = &value["event"];
                if event["type"] == "content_block_delta" && event["delta"]["type"] == "text_delta" {
                    listener.text(event["delta"]["text"].as_str().unwrap_or_default());
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
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    match failure {
        Some(message) => Err(message),
        None if !out.status.success() => Err(format!("claude exited with {}: {}", out.status, String::from_utf8_lossy(&out.stderr).trim())),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    //! Live checks against Claude Code: `cargo test -p ghostwrite-ai -- --ignored`.

    use std::sync::atomic::AtomicBool;

    use super::*;
    use crate::protocol::{self, Kind, Piece};

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

    fn ask(text: &str, range: std::ops::Range<usize>, instruction: &str, followups: Vec<crate::FollowUp>) -> String {
        let req = Request {
            path: Some("calc.py".into()),
            language: "python".into(),
            text: text.into(),
            range,
            instruction: instruction.into(),
            followups,
            output: None,
            root: None,
        };
        let mut out = Collect::default();
        Backend::ClaudeCode.run(&req, &mut out, &AtomicBool::new(false)).unwrap();
        println!("{}", out.0);
        out.0
    }

    #[test]
    #[ignore]
    fn claude_code_writes_small_placeable_steps() {
        let text = "import os\n\ndef add(a, b):\n    pass\n\ndef main():\n    print(add(1, 2))\n";
        let out = ask(text, 26..35, "return the sum, and add type hints to add", vec![]);
        let steps: Vec<_> = protocol::parse_pieces(&out).into_iter().filter_map(|p| if let Piece::Step(s) = p { Some(s) } else { None }).collect();
        assert!(!steps.is_empty() && steps.iter().all(|s| s.complete && protocol::locate(text, &s.old, 3..4).is_some()));
    }

    #[test]
    #[ignore]
    fn claude_code_answers_questions() {
        let out = ask("def add(a, b):\n    return a - b\n", 0..32, "is this function correct?", vec![]);
        assert_eq!(protocol::reply_kind(&out), Some(Kind::Question));
    }

    #[test]
    #[ignore]
    fn claude_code_answers_mid_plan() {
        let text = "def add(a, b):\n    pass\n";
        let first = ask(text, 0..24, "implement it with type hints", vec![]);
        let followup = crate::FollowUp { reply: first, at: None, said: "why int and not float?".into() };
        let second = ask(text, 0..24, "implement it with type hints", vec![followup]);
        assert_eq!(protocol::reply_kind(&second), Some(Kind::Question));
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
            output: None,
            root: Some(root.to_path_buf()),
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
        let out = ask_in(&root, &main, 0..main.len(), "give me a tour of how loading the config works", vec![]);
        assert_eq!(protocol::reply_kind(&out), Some(Kind::Question), "{out}");
        let notes = protocol::parse_notes(&out);
        assert!(notes.iter().any(|n| n.file.as_deref() == Some("util.py")), "a note in util.py, named with <file>: {notes:?}");
    }

    #[test]
    #[ignore]
    fn claude_code_revises_the_rest_of_a_walkthrough() {
        let (root, main) = two_file_project();
        let first = ask_in(&root, &main, 0..main.len(), "walk me through this program step by step", vec![]);
        let notes = protocol::parse_notes(&first);
        assert!(notes.len() >= 2, "a walkthrough to steer: {first}");
        let at = crate::Item::Note(notes[0].clone()).describe();
        let said = "Skip the rest of that. Only show me how a missing config file is handled.";
        let second = ask_in(&root, &main, 0..main.len(), "walk me through this program step by step", vec![crate::FollowUp { reply: first, at: Some(at), said: said.into() }]);
        let (_, rest) = protocol::split_rest(&second);
        assert!(rest.is_some(), "the walkthrough is revised with <rest>: {second}");
    }

    #[test]
    #[ignore]
    fn claude_code_redirects_a_step_with_a_new_rest() {
        let text = "def total(xs):\n    pass\n";
        let root = ghostwrite_project::scratch_dir("live-redirect");
        std::fs::write(root.join("main.py"), text).unwrap();
        let first = ask_in(&root, text, 0..text.len(), "implement it", vec![]);
        let step = protocol::parse_pieces(&first).into_iter().find_map(|p| if let Piece::Step(s) = p { Some(s) } else { None }).expect("a step");
        let at = crate::Item::Step(step).describe();
        let second = ask_in(&root, text, 0..text.len(), "implement it", vec![crate::FollowUp { reply: first, at: Some(at), said: "don't use sum(); write an explicit for loop".into() }]);
        let (_, rest) = protocol::split_rest(&second);
        let steps: Vec<String> = protocol::parse_pieces(&rest.unwrap_or_default()).into_iter().filter_map(|p| if let Piece::Step(s) = p { Some(s.new.join("\n")) } else { None }).collect();
        assert!(steps.iter().any(|s| s.contains("for ")), "the corrected steps come back in <rest>: {second}");
    }
}
