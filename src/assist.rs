//! Inline edits with Claude. You point at a line or selection and say what you
//! want; Claude answers with small whole-line steps, each reviewable the moment
//! it's written, and nothing changes until you accept a step. Claude gets no
//! tools and sees only this file.

use std::io::{BufRead as _, BufReader, Write as _};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde_json::{Value, json};

pub const MODEL: &str = "claude-opus-5-5";
const API_URL: &str = "https://api.anthropic.com/v1/messages";
/// The file around the region is context; very large files get a window.
const CONTEXT_BYTES: usize = 200_000;

const SYSTEM: &str = "You are an assistant inside a text editor. The user points at some lines of their \
file, marked with <cursor> tags, and either asks a question or asks for a change. Decide which it is.

You can read the project with your tools (read, search, list) to understand it: look at what the request \
needs, briefly, then reply. You can't change files with tools. Every change, in any file, is a step in your \
reply, which the user reviews and accepts one at a time. Don't narrate what you're doing; reply only in the \
format below.

If it's a question (explain, why, what, how, is this right, ...), don't change anything. Answer as a few \
short notes, each pinned to the lines of the file it talks about, in the order you'd walk someone through \
the code; the user jumps from note to note:

<answer>
<note>
<at>
the exact existing lines this note is about, copied exactly, usually one or two
</at>
one to three sentences
</note>
</answer>

Leave out <at> for a note about the marked lines as a whole. Be brief and direct, plain text without \
markdown headings, bold or bullets. Put a short code snippet on its own lines, indented four spaces, \
without fences.

If it asks for a change, make only that change, only in this file, matching the surrounding style and \
indentation, as steps (and no <answer>).

Your steps are shown to the user like autocomplete suggestions, one at a time, and accepted with Tab, so \
each step is about one line: the next line they would type, or one line changed. Use two or three lines only \
for something that can't be split, like a short closing block. Write the steps in the order the user would \
type them, top to bottom. A step may build on the steps before it: its <old> can be a line an earlier step \
added. Each step is one edit of whole lines:

<step>
<old>
the existing lines to replace, copied exactly, enough of them to be unique
</old>
<new>
the lines that replace them
</new>
</step>

Put each tag on its own line. To insert, include one neighbouring existing line in <old> and repeat it in \
<new>, followed or preceded by the new lines. An empty <old> means the marked lines themselves. Never copy the \
<cursor> tags into a step. Reply with steps only: no explanation, no markdown fences.

If the change needs a new file or folder, propose it as its own step, in the order it would be made, with a \
path relative to the project (end it with / for a folder):

<create>
src/util.rs
</create>

Then write the new file's contents as steps in that file, a line or two at a time, naming it:

<step>
<file>
src/util.rs
</file>
<old>
</old>
<new>
its first lines
</new>
</step>

In another file an empty <old> means: append at the end. Leave out <file> for steps in the marked file.

To delete a file or folder the change makes unnecessary, propose it the same way:

<remove>
src/old_util.rs
</remove>

To rename or move one, the old path then the new:

<rename>
src/util.rs
src/helpers.rs
</rename>

Steps after a rename name the file by its new path.";

/// What Claude made of the request: steps that edit, or an answer that doesn't.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Edit,
    Question,
}

/// Which kind of reply this is, once its first tag has arrived.
pub fn reply_kind(reply: &str) -> Option<Kind> {
    // From the first tag: Claude may say a word before looking at files.
    let first = |tag: &str| reply.find(tag);
    let answer = first("<answer>");
    let edit = ["<step>", "<create>", "<remove>", "<rename>"].iter().filter_map(|t| first(t)).min();
    match (answer, edit) {
        (Some(a), Some(e)) => Some(if a < e { Kind::Question } else { Kind::Edit }),
        (Some(_), None) => Some(Kind::Question),
        (None, Some(_)) => Some(Kind::Edit),
        (None, None) => None,
    }
}

/// The answer so far, without its tags (a half-written closing tag is hidden).
pub fn answer_text(reply: &str) -> String {
    tagged_lines(reply, "<answer>", "</answer>").map(|(lines, _)| lines.join("\n").trim_end().to_string()).unwrap_or_default()
}

/// One note of an answer: the lines it's about (empty: the marked lines) and what it says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Note {
    pub at: Vec<String>,
    pub text: String,
}

/// The answer's notes so far; a note appears once its `<at>` is complete and
/// its text grows as it streams. An answer without notes is one note.
pub fn parse_notes(reply: &str) -> Vec<Note> {
    let answer = answer_text(reply);
    // An answer without notes: each paragraph is a note about the marked lines.
    if !answer.contains("<note>") {
        return answer
            .split("\n\n")
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .map(|p| Note { at: vec![], text: p.to_string() })
            .collect();
    }
    answer
        .split("<note>")
        .skip(1)
        .filter_map(|chunk| {
            let (at, body) = match chunk.find("<at>") {
                Some(_) => {
                    let (at, closed) = tagged_lines(chunk, "<at>", "</at>")?;
                    if !closed {
                        return None;
                    }
                    (at, &chunk[chunk.find("</at>")? + "</at>".len()..])
                }
                None => (vec![], chunk),
            };
            let body = body.split("</note>").next().unwrap_or(body);
            let cut = (1.."</note>".len()).rev().find(|&n| body.ends_with(&"</note>"[..n])).unwrap_or(0);
            Some(Note { at, text: body[..body.len() - cut].trim().to_string() })
        })
        .collect()
}

#[derive(Debug, Clone)]
pub enum Backend {
    /// The Claude API, with `ANTHROPIC_API_KEY`.
    Api { key: String },
    /// `claude -p`, under the user's Claude Code login.
    ClaudeCode,
    /// Replies with fixed text, for tests.
    #[cfg(test)]
    Fixed(String),
    /// Replies with the text for this round (first ask, then each redirect), for tests.
    #[cfg(test)]
    Rounds(Vec<String>),
}

impl Backend {
    pub fn from_env() -> Option<Self> {
        if let Some(key) = std::env::var("ANTHROPIC_API_KEY").ok().filter(|k| !k.is_empty()) {
            return Some(Backend::Api { key });
        }
        let on_path = std::env::var_os("PATH")
            .is_some_and(|paths| std::env::split_paths(&paths).any(|d| d.join("claude").is_file()));
        // Apps opened from Finder get a minimal PATH; Claude Code's installer uses ~/.local/bin.
        let local = std::env::home_dir().is_some_and(|h| h.join(".local/bin/claude").is_file());
        (on_path || local).then_some(Backend::ClaudeCode)
    }

    fn claude_binary() -> PathBuf {
        let local = std::env::home_dir().map(|h| h.join(".local/bin/claude"));
        local.filter(|p| p.is_file()).unwrap_or_else(|| PathBuf::from("claude"))
    }
}

fn effort() -> String {
    // Interactive, so brief thinking by default; override for harder asks.
    let effort = std::env::var("GHOSTWRITE_AI_EFFORT").ok().unwrap_or_default();
    if matches!(effort.as_str(), "low" | "medium" | "high" | "xhigh" | "max") { effort } else { "low".into() }
}

pub struct EditRequest {
    pub path: Option<PathBuf>,
    pub language: String,
    pub text: String,
    /// The lines the user pointed at, in bytes, whole lines.
    pub range: Range<usize>,
    pub instruction: String,
    /// Earlier rounds: Claude's reply, then the user's redirection.
    pub followups: Vec<(String, String)>,
    /// Command output the request is about (an error from `:sh`), if any.
    pub context: Option<String>,
    /// The project, which paths are relative to.
    pub root: Option<PathBuf>,
}

fn floor_char(text: &str, mut i: usize) -> usize {
    while !text.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn user_prompt(req: &EditRequest) -> String {
    let mut prompt = first_prompt(req);
    if let Some(context) = &req.context {
        prompt = prompt.replacen(
            "\n\nRequest: ",
            &format!("\n\nCommand output about the marked lines:\n<output>\n{}\n</output>\n\nRequest: ", context.trim()),
            1,
        );
    }
    for (reply, feedback) in &req.followups {
        prompt.push_str(&format!(
            "\n\nYour earlier reply:\n<earlier>\n{}\n</earlier>\nAny steps the user accepted are already in the \
             file above. The user now says: {}\nIf that's a question, answer it with <answer> only: your \
             remaining steps stay as they are and the user goes back to them afterwards. Otherwise reply again, \
             with only what's still to do.",
            reply.trim(),
            feedback.trim()
        ));
    }
    prompt
}

fn first_prompt(req: &EditRequest) -> String {
    let (text, range) = (&req.text, &req.range);
    let start = floor_char(text, range.start.saturating_sub(CONTEXT_BYTES / 2));
    let end = floor_char(text, (range.end + CONTEXT_BYTES / 2).min(text.len()));
    let name = req.path.as_ref().map_or("untitled".into(), |p| {
        req.root.as_ref().and_then(|r| p.strip_prefix(r).ok()).unwrap_or(p).display().to_string()
    });
    let files = req.root.as_deref().map(crate::project::listing).map(|l| format!("Project files:\n{l}\n\n")).unwrap_or_default();
    format!(
        "{files}File: {name} ({})\n\n<file>\n{}<cursor>{}</cursor>{}\n</file>\n\nRequest: {}",
        req.language,
        &text[start..range.start],
        &text[range.clone()],
        &text[range.end..end],
        req.instruction.trim()
    )
}

/// One step of Claude's answer: whole lines out, whole lines in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Step {
    /// The file it's in, relative to the project; `None`: the file being edited.
    pub file: Option<String>,
    pub old: Vec<String>,
    pub new: Vec<String>,
    /// `</new>` has arrived; until then `new` is still being written.
    pub complete: bool,
}

/// The lines between `open` and `close` tag lines; `None` until `open` has
/// arrived, and the partial lines so far until `close` has.
fn tagged_lines(text: &str, open: &str, close: &str) -> Option<(Vec<String>, bool)> {
    let start = text.find(open)? + open.len();
    let body = text[start..].strip_prefix('\n').unwrap_or(&text[start..]);
    let (body, closed) = match body.find(close) {
        Some(end) => (&body[..end], true),
        // Hide a closing tag that's only partly written.
        None => {
            let cut = (1..close.len()).rev().find(|&n| body.ends_with(&close[..n])).unwrap_or(0);
            (&body[..body.len() - cut], false)
        }
    };
    let body = if closed { body.strip_suffix('\n').unwrap_or(body) } else { body };
    let lines = if body.is_empty() && closed { vec![] } else { body.split('\n').map(str::to_string).collect() };
    Some((lines, closed))
}

/// The steps in Claude's reply so far. A step shows up once its `<old>` is
/// complete, and its `new` lines grow as they stream in.
#[cfg(test)]
pub fn parse_steps(reply: &str) -> Vec<Step> {
    parse_pieces(reply).into_iter().filter_map(|p| if let Piece::Step(s) = p { Some(s) } else { None }).collect()
}

/// A new file or folder Claude proposes (a path ending in `/` is a folder).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Create {
    pub path: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Piece {
    Step(Step),
    Create(Create),
    /// A file or folder Claude proposes removing (to the Trash).
    Remove(Create),
    /// A file or folder Claude proposes moving to a new path.
    Rename { from: String, to: String },
}

/// Steps and file creations, in the order Claude wrote them. A step shows
/// up once its `<old>` is complete; a creation once its path line is.
pub fn parse_pieces(reply: &str) -> Vec<Piece> {
    // Where each piece starts, and its tag.
    let starts: Vec<(usize, &str)> = ["<step>", "<create>", "<remove>", "<rename>"]
        .iter()
        .flat_map(|tag| reply.match_indices(tag).map(move |(i, _)| (i, *tag)))
        .collect::<std::collections::BTreeMap<_, _>>()
        .into_iter()
        .collect();
    starts
        .iter()
        .enumerate()
        .filter_map(|(n, &(at, tag))| {
            let end = starts.get(n + 1).map_or(reply.len(), |(next, _)| *next);
            let chunk = &reply[at..end];
            if tag == "<rename>" {
                let (lines, done) = tagged_lines(chunk, "<rename>", "</rename>")?;
                let mut paths = lines.iter().map(|l| l.trim()).filter(|l| !l.is_empty());
                let (from, to) = (paths.next()?, paths.next()?);
                return (done || lines.len() > 2).then(|| Piece::Rename { from: from.to_string(), to: to.to_string() });
            }
            if tag != "<step>" {
                let close = if tag == "<create>" { "</create>" } else { "</remove>" };
                let (lines, done) = tagged_lines(chunk, tag, close)?;
                let path = lines.iter().map(|l| l.trim()).find(|l| !l.is_empty())?;
                let target = Create { path: path.to_string() };
                return (done || lines.len() > 1).then(|| if tag == "<create>" { Piece::Create(target) } else { Piece::Remove(target) });
            }
            let file = tagged_lines(chunk, "<file>", "</file>")
                .filter(|(_, done)| *done)
                .and_then(|(lines, _)| lines.into_iter().map(|l| l.trim().to_string()).find(|l| !l.is_empty()));
            let (old, old_done) = tagged_lines(chunk, "<old>", "</old>")?;
            if !old_done {
                return None;
            }
            let after_old = &chunk[chunk.find("</old>")?..];
            let (new, complete) = tagged_lines(after_old, "<new>", "</new>").unwrap_or((vec![], false));
            Some(Piece::Step(Step { file, old, new, complete }))
        })
        .collect()
}

/// Where `step.old` is in `text`, as a range of whole lines: the occurrence
/// nearest the lines the user pointed at (`near`), ignoring trailing spaces.
/// An empty `old` is those lines themselves.
pub fn locate(text: &str, old: &[String], near: Range<usize>) -> Option<Range<usize>> {
    if old.is_empty() {
        return Some(near);
    }
    let lines: Vec<&str> = text.split('\n').collect();
    let find = |same: &dyn Fn(&str, &str) -> bool| {
        (0..lines.len().saturating_sub(old.len() - 1))
            .filter(|&i| old.iter().enumerate().all(|(j, o)| same(lines[i + j], o)))
            .min_by_key(|&i| i.abs_diff(near.start))
            .map(|i| i..i + old.len())
    };
    // Exactly (but for trailing spaces); failing that, ignoring indentation.
    find(&|a, b| a.trim_end() == b.trim_end()).or_else(|| find(&|a, b| a.trim() == b.trim()))
}

/// The visible part of a step: lines that change, after dropping the lines
/// `old` and `new` share at either end.
#[derive(Debug, PartialEq, Eq)]
pub struct Change {
    /// Buffer lines struck out.
    pub removed: Range<usize>,
    /// Lines shown as ghost text after `removed` (or after the line above).
    pub added: Vec<String>,
}

pub fn change(at: Range<usize>, old: &[String], new: &[String]) -> Change {
    let old: Vec<&String> = if old.is_empty() { vec![] } else { old.iter().collect() };
    let prefix = old.iter().zip(new).take_while(|(a, b)| a.trim_end() == b.trim_end()).count();
    let rest = old.len().min(new.len()) - prefix;
    let suffix = old.iter().rev().zip(new.iter().rev()).take(rest).take_while(|(a, b)| a.trim_end() == b.trim_end()).count();
    let (keep_old, keep_new) = if old.is_empty() { (0, 0) } else { (prefix, suffix) };
    let removed_len = if old.is_empty() { at.len() } else { old.len() - keep_old - keep_new };
    let removed_start = at.start + keep_old;
    Change {
        removed: removed_start..removed_start + removed_len,
        added: new[prefix.min(new.len())..new.len() - suffix.min(new.len() - prefix.min(new.len()))].to_vec(),
    }
}

/// Word-wrap an answer to `cols` columns, keeping its blank lines and indentation.
pub fn wrap(text: &str, cols: usize) -> Vec<String> {
    let cols = cols.max(20);
    let mut rows = vec![];
    for line in text.trim_end().split('\n') {
        let indent: String = line.chars().take_while(|c| *c == ' ').collect();
        let mut row = indent.clone();
        for word in line.split_whitespace() {
            let len = row.chars().count();
            if len > indent.len() && len + 1 + word.chars().count() > cols {
                rows.push(std::mem::replace(&mut row, indent.clone()));
            }
            if row.chars().count() > indent.len() {
                row.push(' ');
            }
            row.push_str(word);
        }
        rows.push(row);
    }
    rows
}

/// Stream Claude's reply into `on_text`. Stops quietly once `cancel` is set.
#[cfg(test)]
pub fn run(backend: &Backend, req: &EditRequest, on_text: &mut dyn FnMut(&str), cancel: &AtomicBool) -> Result<(), String> {
    run_with(backend, req, on_text, &mut |_| {}, cancel)
}

/// Like `run`, also saying what Claude looks at in the project (`on_look`).
pub fn run_with(
    backend: &Backend,
    req: &EditRequest,
    on_text: &mut dyn FnMut(&str),
    on_look: &mut dyn FnMut(String),
    cancel: &AtomicBool,
) -> Result<(), String> {
    match backend {
        Backend::Api { key } => run_api(key, req, on_text, on_look, cancel),
        Backend::ClaudeCode => run_claude_code(req, on_text, on_look, cancel),
        #[cfg(test)]
        Backend::Fixed(reply) => {
            on_text(reply);
            Ok(())
        }
        #[cfg(test)]
        Backend::Rounds(replies) => {
            on_text(&replies[req.followups.len().min(replies.len() - 1)]);
            Ok(())
        }
    }
}

/// Rounds of looking around before Claude has to answer.
const MAX_TOOL_ROUNDS: usize = 12;

/// The Messages API, with read-only project tools run here. Every content
/// block (thinking included) goes back unchanged on the next round.
fn run_api(key: &str, req: &EditRequest, on_text: &mut dyn FnMut(&str), on_look: &mut dyn FnMut(String), cancel: &AtomicBool) -> Result<(), String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(Duration::from_secs(300)))
        .build()
        .into();
    let root = req.root.clone();
    let mut messages = vec![json!({ "role": "user", "content": user_prompt(req) })];
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
        if root.is_some() {
            body["tools"] = crate::project::definitions();
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
            let message = serde_json::from_str::<Value>(&text)
                .ok()
                .and_then(|v| v["error"]["message"].as_str().map(str::to_string))
                .unwrap_or(text);
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
            let Some(data) = line.strip_prefix("data: ") else { continue };
            let Ok(event) = serde_json::from_str::<Value>(data) else { continue };
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
                    match delta["type"].as_str() {
                        Some("text_delta") => {
                            let text = delta["text"].as_str().unwrap_or_default();
                            on_text(text);
                            let so_far = block["text"].as_str().unwrap_or_default().to_string();
                            block["text"] = json!(so_far + text);
                        }
                        Some("thinking_delta") => {
                            let so_far = block["thinking"].as_str().unwrap_or_default().to_string();
                            block["thinking"] = json!(so_far + delta["thinking"].as_str().unwrap_or_default());
                        }
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
        for (ix, block) in blocks.iter_mut().enumerate() {
            if block["type"] != "tool_use" {
                continue;
            }
            let input: Option<Value> = serde_json::from_str(&partial_json[ix]).ok().filter(Value::is_object);
            let name = block["name"].as_str().unwrap_or_default().to_string();
            let outcome = match (&input, &root) {
                (Some(input), Some(root)) => {
                    on_look(crate::project::describe(&name, input));
                    crate::project::call(root, &name, input)
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
fn run_claude_code(req: &EditRequest, on_text: &mut dyn FnMut(&str), on_look: &mut dyn FnMut(String), cancel: &AtomicBool) -> Result<(), String> {
    let mut command = Command::new(Backend::claude_binary());
    command
        .args(["-p", "--output-format", "stream-json", "--include-partial-messages", "--verbose"])
        .args(["--setting-sources", "", "--strict-mcp-config", "--no-session-persistence"])
        .args(["--model", MODEL, "--effort", &effort(), "--system-prompt", SYSTEM]);
    match &req.root {
        // It reads the project to understand it; it changes nothing (that's the steps).
        Some(root) => command.args(["--tools", "Read,Glob,Grep", "--allowedTools", "Read,Glob,Grep"]).current_dir(root),
        None => command.args(["--tools", ""]).current_dir(std::env::temp_dir()),
    };
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("couldn't run `claude`: {e}"))?;
    child.stdin.take().ok_or("no stdin")?.write_all(user_prompt(req).as_bytes()).map_err(|e| e.to_string())?;
    let stdout = child.stdout.take().ok_or("no stdout")?;
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
                    on_text(event["delta"]["text"].as_str().unwrap_or_default());
                }
            }
            Some("assistant") => {
                for block in value["message"]["content"].as_array().into_iter().flatten() {
                    if block["type"] == "tool_use" {
                        let name = block["name"].as_str().unwrap_or_default();
                        let mut input = block["input"].clone();
                        // Show paths relative to the project.
                        // (macOS paths may come back resolved: /var is /private/var.)
                        let real = req.root.as_ref().and_then(|r| r.canonicalize().ok());
                        if let (Some(path), Some(root)) = (input["file_path"].as_str(), &req.root)
                            && let Some(rel) = Path::new(path).strip_prefix(root).ok().or_else(|| Path::new(path).strip_prefix(real.as_ref()?).ok())
                        {
                            input["file_path"] = json!(rel.display().to_string());
                        }
                        on_look(crate::project::describe(name, &input));
                    }
                }
            }
            Some("result") if value["is_error"] == true => {
                failure = Some(value["result"].as_str().unwrap_or("Claude Code failed").to_string());
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
    use super::{Change, EditRequest, Kind, Step, answer_text, change, locate, parse_steps, reply_kind, user_prompt, wrap};

    #[test]
    fn notes_pin_to_lines_as_they_stream() {
        use super::{Note, parse_notes};
        let reply = "<answer>\n<note>\n<at>\n    return a - b\n</at>\nThis subtracts.\n</note>\n<note>\nIt should add.\n</no";
        assert_eq!(
            parse_notes(reply),
            vec![
                Note { at: vec!["    return a - b".into()], text: "This subtracts.".into() },
                Note { at: vec![], text: "It should add.".into() },
            ]
        );
        assert_eq!(parse_notes("<answer>\n<note>\n<at>\n    ret"), vec![]);
        assert_eq!(
            parse_notes("<answer>\nPlain answer.\n\nSecond part.\n</answer>"),
            vec![Note { at: vec![], text: "Plain answer.".into() }, Note { at: vec![], text: "Second part.".into() }]
        );
    }

    #[test]
    fn tells_answers_from_edits() {
        assert_eq!(reply_kind(""), None);
        assert_eq!(reply_kind("\n<answ"), None);
        assert_eq!(reply_kind("<answer>\nIt adds."), Some(Kind::Question));
        assert_eq!(reply_kind("<step>\n<old>"), Some(Kind::Edit));
        assert_eq!(answer_text("<answer>\nIt adds\ntwo numbers.\n</ans"), "It adds\ntwo numbers.");
    }

    #[test]
    fn wraps_answers_to_the_editor() {
        assert_eq!(wrap("one two three four", 20), ["one two three four"]);
        let rows = wrap("aaaa bbbb cccc dddd eeee ffff\n\n    code(x)", 20);
        assert_eq!(rows, ["aaaa bbbb cccc dddd", "eeee ffff", "", "    code(x)"]);
    }

    fn lines(s: &[&str]) -> Vec<String> {
        s.iter().map(|l| l.to_string()).collect()
    }

    #[test]
    fn prompt_marks_the_lines() {
        let req = EditRequest {
            path: None,
            language: "rust".into(),
            text: "fn a() {}\nfn b() {}\n".into(),
            range: 10..20,
            instruction: "rename to c".into(),
            followups: vec![],
            context: None,
            root: None,
        };
        let prompt = user_prompt(&req);
        assert!(prompt.contains("<file>\nfn a() {}\n<cursor>fn b() {}\n</cursor>\n</file>"), "{prompt}");
        assert!(prompt.ends_with("Request: rename to c"));
        let again = user_prompt(&EditRequest { followups: vec![("<step>…</step>".into(), "keep the old name".into())], ..req });
        assert!(again.contains("<earlier>\n<step>…</step>\n</earlier>") && again.contains("The user now says: keep the old name"));
    }

    #[test]
    fn steps_appear_as_they_stream() {
        let full = "<step>\n<old>\n    pass\n</old>\n<new>\n    total = a + b\n    return total\n</new>\n</step>\n<step>\n<old>\n</old>\n<new>\nx\n</new>\n</step>";
        assert_eq!(parse_steps(&full[..20]), vec![]);
        let partial = &full[..full.find("    return").unwrap() + 6];
        assert_eq!(parse_steps(partial), vec![Step { file: None, old: lines(&["    pass"]), new: lines(&["    total = a + b", "    re"]), complete: false }]);
        let cut_tag = &full[..full.find("</new>").unwrap() + 3];
        assert_eq!(parse_steps(cut_tag)[0].new, lines(&["    total = a + b", "    return total", ""]));
        let steps = parse_steps(full);
        assert_eq!(steps.len(), 2);
        assert!(steps[0].complete && steps[1].complete);
        assert_eq!(steps[1], Step { file: None, old: vec![], new: lines(&["x"]), complete: true });
    }

    #[test]
    fn locates_the_nearest_copy() {
        let text = "a\nx\nb\nx\nc\n";
        assert_eq!(locate(text, &lines(&["x"]), 3..4), Some(3..4));
        assert_eq!(locate(text, &lines(&["x  "]), 0..1), Some(1..2));
        assert_eq!(locate(text, &lines(&["b", "x"]), 0..1), Some(2..4));
        assert_eq!(locate(text, &lines(&["nope"]), 0..1), None);
        assert_eq!(locate("def f():\n\treturn 1\n", &lines(&["    return 1"]), 0..1), Some(1..2), "indentation may differ");
        assert_eq!(locate(text, &[], 4..5), Some(4..5));
    }

    #[test]
    fn change_hides_shared_lines() {
        // An insertion that repeats its anchor line shows only the new line.
        let c = change(5..6, &lines(&["fn a() {"]), &lines(&["fn a() {", "    x();"]));
        assert_eq!(c, Change { removed: 6..6, added: lines(&["    x();"]) });
        // A replacement strikes the old line and adds the new one.
        let c = change(2..5, &lines(&["a", "b", "c"]), &lines(&["a", "B", "c"]));
        assert_eq!(c, Change { removed: 3..4, added: lines(&["B"]) });
        // An empty `old` replaces the marked lines.
        let c = change(7..8, &[], &lines(&["new"]));
        assert_eq!(c, Change { removed: 7..8, added: lines(&["new"]) });
    }

    /// One real request through `claude -p`: `cargo test claude_code_writes_steps -- --ignored`.
    #[test]
    #[ignore]
    fn claude_code_writes_steps() {
        use std::sync::atomic::AtomicBool;
        let text = "import os\n\ndef add(a, b):\n    pass\n\ndef main():\n    print(add(1, 2))\n";
        let req = EditRequest {
            path: Some("calc.py".into()),
            language: "py".into(),
            text: text.into(),
            range: 26..35,
            instruction: "return the sum, and add type hints to add".into(),
            followups: vec![],
            context: None,
            root: None,
        };
        let (mut out, mut first_step) = (String::new(), None);
        let started = std::time::Instant::now();
        super::run(&super::Backend::ClaudeCode, &req, &mut |t| {
            out.push_str(t);
            if first_step.is_none() && !parse_steps(&out).is_empty() {
                first_step = Some(started.elapsed());
            }
        }, &AtomicBool::new(false))
        .unwrap();
        let steps = parse_steps(&out);
        println!("first step after {first_step:?}, done after {:?}\n{out}", started.elapsed());
        assert!(!steps.is_empty() && steps.iter().all(|s| s.complete));
        for s in &steps {
            assert!(locate(text, &s.old, 3..4).is_some(), "unplaceable step {s:?}");
        }
        assert!(steps.iter().any(|s| s.new.iter().any(|l| l.contains("return a + b"))));
    }

    /// Live: a bigger ask comes back as small steps. `cargo test claude_code_keeps_steps_small -- --ignored`.
    #[test]
    #[ignore]
    fn claude_code_keeps_steps_small() {
        use std::sync::atomic::AtomicBool;
        let text = "import json\n\n\ndef main():\n    pass\n";
        let req = EditRequest {
            path: Some("tool.py".into()),
            language: "py".into(),
            text: text.into(),
            range: 12..13,
            instruction: "write a function load_users(path) that reads a JSON list of users, skips ones without an email, and returns them sorted by name".into(),
            followups: vec![],
            context: None,
            root: None,
        };
        let mut out = String::new();
        let started = std::time::Instant::now();
        super::run(&super::Backend::ClaudeCode, &req, &mut |t| out.push_str(t), &AtomicBool::new(false)).unwrap();
        let steps = parse_steps(&out);
        let sizes: Vec<usize> = steps.iter().map(|s| change(0..s.old.len(), &s.old, &s.new).added.len()).collect();
        println!("{} steps in {:?}, added lines per step: {sizes:?}\n{out}", steps.len(), started.elapsed());
        assert!(steps.len() >= 4 && sizes.iter().all(|&n| n <= 3), "{sizes:?}");
    }

    /// Live: a question gets an answer, not steps. `cargo test claude_code_answers_questions -- --ignored`.
    #[test]
    #[ignore]
    fn claude_code_answers_questions() {
        use std::sync::atomic::AtomicBool;
        let text = "def add(a, b):\n    return a - b\n";
        let req = EditRequest {
            path: Some("calc.py".into()),
            language: "py".into(),
            text: text.into(),
            range: 0..32,
            instruction: "is this function correct?".into(),
            followups: vec![],
            context: None,
            root: None,
        };
        let mut out = String::new();
        let started = std::time::Instant::now();
        super::run(&super::Backend::ClaudeCode, &req, &mut |t| out.push_str(t), &AtomicBool::new(false)).unwrap();
        println!("{:?}\n{out}", started.elapsed());
        assert_eq!(reply_kind(&out), Some(Kind::Question), "{out}");
        assert!(parse_steps(&out).is_empty() && !answer_text(&out).is_empty());
        let notes = super::parse_notes(&out);
        assert!(!notes.is_empty());
        for n in &notes {
            assert!(locate(text, &n.at, 0..2).is_some(), "unplaceable note {n:?}");
        }
    }

    /// Live: a redirect changes the suggestion. `cargo test claude_code_follows_redirects -- --ignored`.
    #[test]
    #[ignore]
    fn claude_code_follows_redirects() {
        use std::sync::atomic::AtomicBool;
        let text = "def total(xs):\n    pass\n";
        let mut req = EditRequest {
            path: Some("calc.py".into()),
            language: "py".into(),
            text: text.into(),
            range: 15..24,
            instruction: "implement it".into(),
            followups: vec![],
            context: None,
            root: None,
        };
        let mut first = String::new();
        super::run(&super::Backend::ClaudeCode, &req, &mut |t| first.push_str(t), &AtomicBool::new(false)).unwrap();
        req.followups.push((first.clone(), "don't use sum(); write an explicit for loop".into()));
        let mut second = String::new();
        super::run(&super::Backend::ClaudeCode, &req, &mut |t| second.push_str(t), &AtomicBool::new(false)).unwrap();
        println!("first:\n{first}\nsecond:\n{second}");
        let new: Vec<String> = parse_steps(&second).into_iter().flat_map(|s| s.new).collect();
        assert!(new.iter().any(|l| l.contains("for ")) && !new.iter().any(|l| l.contains("sum(")), "{second}");
    }

    /// Live: a question mid-plan gets an answer, not new steps. `cargo test claude_code_answers_mid_plan -- --ignored`.
    #[test]
    #[ignore]
    fn claude_code_answers_mid_plan() {
        use std::sync::atomic::AtomicBool;
        let text = "def add(a, b):\n    pass\n";
        let mut req = EditRequest {
            path: Some("calc.py".into()),
            language: "py".into(),
            text: text.into(),
            range: 0..24,
            instruction: "implement it with type hints".into(),
            followups: vec![],
            context: None,
            root: None,
        };
        let mut first = String::new();
        super::run(&super::Backend::ClaudeCode, &req, &mut |t| first.push_str(t), &AtomicBool::new(false)).unwrap();
        req.followups.push((first.clone(), "why int and not float?".into()));
        let mut second = String::new();
        super::run(&super::Backend::ClaudeCode, &req, &mut |t| second.push_str(t), &AtomicBool::new(false)).unwrap();
        println!("first:\n{first}\nsecond:\n{second}");
        assert_eq!(reply_kind(&second), Some(Kind::Question), "{second}");
    }

    #[test]
    fn creations_and_steps_in_other_files_parse_in_order() {
        use super::{Create, Piece, parse_pieces};
        let reply = "<create>\nsrc/util.rs\n</create>\n<step>\n<file>\nsrc/util.rs\n</file>\n<old>\n</old>\n<new>\npub fn one() {}\n</new>\n</step>\n<step>\n<old>\nfn main() {}\n</old>\n<new>\nmod util;\nfn main() {}\n</new>\n</step>\n";
        let pieces = parse_pieces(reply);
        assert_eq!(pieces.len(), 3);
        assert_eq!(pieces[0], Piece::Create(Create { path: "src/util.rs".into() }));
        let Piece::Step(s) = &pieces[1] else { panic!() };
        assert_eq!((s.file.as_deref(), s.old.len(), s.new[0].as_str()), (Some("src/util.rs"), 0, "pub fn one() {}"));
        let Piece::Step(s) = &pieces[2] else { panic!() };
        assert_eq!(s.file, None);
        assert_eq!(reply_kind(reply), Some(Kind::Edit));
        assert_eq!(parse_pieces("<create>\nsrc/ut"), vec![], "a path appears once its line is done");
    }

    /// Live: a change needing a new file proposes it as a step. `cargo test claude_code_creates_files -- --ignored`.
    #[test]
    #[ignore]
    fn claude_code_creates_files() {
        use std::sync::atomic::AtomicBool;
        use super::{Piece, parse_pieces};
        let text = "def add(a, b):\n    return a + b\n\nprint(add(1, 2))\n";
        let req = EditRequest {
            path: Some("/proj/main.py".into()),
            language: "py".into(),
            text: text.into(),
            range: 0..32,
            instruction: "move add into a new module mathutil.py and import it here".into(),
            followups: vec![],
            context: None,
            root: Some("/proj".into()),
        };
        let mut out = String::new();
        super::run(&super::Backend::ClaudeCode, &req, &mut |t| out.push_str(t), &AtomicBool::new(false)).unwrap();
        println!("{out}");
        let pieces = parse_pieces(&out);
        assert!(pieces.iter().any(|p| matches!(p, Piece::Create(c) if c.path.ends_with("mathutil.py"))), "{out}");
        assert!(pieces.iter().any(|p| matches!(p, Piece::Step(s) if s.file.as_deref().is_some_and(|f| f.ends_with("mathutil.py")))));
    }

    #[test]
    fn removals_parse_as_pieces() {
        use super::{Create, Piece, parse_pieces};
        let reply = "<remove>\nsrc/old.rs\n</remove>\n";
        assert_eq!(parse_pieces(reply), vec![Piece::Remove(Create { path: "src/old.rs".into() })]);
        assert_eq!(reply_kind(reply), Some(Kind::Edit));
    }

    /// Live: Claude reads another project file to answer. `cargo test claude_code_reads_the_project -- --ignored`
    #[test]
    #[ignore]
    fn claude_code_reads_the_project() {
        use std::sync::atomic::AtomicBool;
        let root = std::env::temp_dir().join(format!("ghostwrite-read-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("helpers.py"), "def rescale_by_golden(values):\n    return [v * 1.618 for v in values]\n").unwrap();
        let text = "data = [1, 2, 3]\nprint(data)\n";
        std::fs::write(root.join("main.py"), text).unwrap();
        let req = EditRequest {
            path: Some(root.join("main.py")),
            language: "py".into(),
            text: text.into(),
            range: 17..29,
            instruction: "print the data rescaled with the helper in helpers.py instead".into(),
            followups: vec![],
            context: None,
            root: Some(root.clone()),
        };
        let (mut out, mut looks) = (String::new(), vec![]);
        super::run_with(&super::Backend::ClaudeCode, &req, &mut |t| out.push_str(t), &mut |w| looks.push(w), &AtomicBool::new(false)).unwrap();
        println!("looked: {looks:?}\n{out}");
        std::fs::remove_dir_all(&root).ok();
        assert!(out.contains("rescale_by_golden"), "it found the helper by reading the project");
    }

    #[test]
    fn renames_parse_as_pieces() {
        use super::{Piece, parse_pieces};
        let reply = "<rename>\nsrc/util.rs\nsrc/helpers.rs\n</rename>\n";
        assert_eq!(parse_pieces(reply), vec![Piece::Rename { from: "src/util.rs".into(), to: "src/helpers.rs".into() }]);
        assert_eq!(parse_pieces("<rename>\nsrc/util.rs\n"), vec![], "both paths first");
        assert_eq!(reply_kind(reply), Some(Kind::Edit));
    }
}
