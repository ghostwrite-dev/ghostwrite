//! Reaching the model: Claude through the Messages API or Claude Code (`claude
//! -p`, under the user's login), or a local model through [`local`]. They differ only in how a turn is
//! sent: every one gets the same prompt, and looks around the same way, with
//! `<tool>` blocks run here ([`converse`], [`tools`]). The reply streams in as
//! text; what the model looks at is reported as it happens.
//!
//! A story is one conversation. The API and a local model are sent every turn;
//! Claude Code keeps a session, reported back with [`Listener::session`], and
//! each follow-up resumes it with only the newest turn (so it still has the
//! files it read). If resuming fails, the whole conversation is sent anew.

use std::io::{BufRead as _, BufReader, Write as _};
#[cfg(unix)]
use std::os::unix::process::CommandExt as _;
use std::path::PathBuf;
use std::process::{Child, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde_json::{Value, json};

use crate::attach::Attachment;
use crate::context::{Request, Role};
use crate::protocol;
use crate::{local, models, settings, tools};

const API_URL: &str = "https://api.anthropic.com/v1/messages";
/// Rounds of looking around before the model has to answer.
const MAX_TOOL_ROUNDS: usize = 12;
/// The most of files a conversation is shown: in base64, about what a request can hold.
const MAX_SHOWN: usize = 24 << 20;
/// What one command may show, and what a reply may read in all before it has
/// to answer: a local model's context is small, and has to fit the answer too.
const ROOM: usize = 120_000;
const LOCAL_ROOM: usize = 4_000;
/// A reply is one small step: one that runs on stops here. It has plenty of room,
/// and a small model doesn't get to ramble.
const LOCAL_MAX_TOKENS: usize = 400;
/// Turns kept whole for a local model. Every step is small, and what was accepted is
/// in the files it's shown: earlier replies shrink to a line each.
const LOCAL_TURNS_KEPT: usize = 2;
/// How far what's sent is shrunk, in turn, to fit a local model's context.
const LOCAL_SHRINKS: [usize; 4] = [1, 2, 4, 8];
const END_TOOL: &str = "</tool>";

#[derive(Debug, Clone)]
pub enum Backend {
    /// The Claude API, with `ANTHROPIC_API_KEY`.
    Api { key: String, model: String },
    /// `claude -p`, under the user's Claude Code login.
    ClaudeCode { model: String },
    /// A local model run here: `user/repo:quantization` on Hugging Face, or a file.
    Local { model: String, label: String },
    /// Replies with these texts in turn, the last one from then on (for tests).
    Scripted(Vec<String>),
}

/// What a running request reports.
pub trait Listener {
    fn text(&mut self, chunk: &str);
    /// What the model is looking at in the project ("reading src/lib.rs").
    fn looking(&mut self, what: String);
    /// The Claude Code session the reply is in, to resume for the next turn.
    fn session(&mut self, _id: String) {}
    /// What a turn used: tokens in and out.
    fn used(&mut self, _input: u64, _output: u64) {}
    /// The model's process is up: one started ahead of time (`true`), or just now.
    fn started(&mut self, _ahead: bool) {}
    /// The model would read `path`, outside the project: whether the user lets it
    /// (and what's under it, if it's a folder). Blocks until they've said.
    fn may_read(&mut self, _path: &std::path::Path) -> bool {
        false
    }
}

/// Tokens read for a request: its own, and what came from or went into the cache.
fn tokens_in(usage: &Value) -> u64 {
    ["input_tokens", "cache_creation_input_tokens", "cache_read_input_tokens"].iter().filter_map(|key| usage[*key].as_u64()).sum()
}

impl Backend {
    /// What it's called, for the prompt ("Ask …").
    pub fn name(&self) -> &str {
        match self {
            Backend::Api { model, .. } | Backend::ClaudeCode { model } => {
                models::CLAUDE.iter().find(|(id, ..)| id == model).map_or("Claude", |(_, name, _)| name)
            }
            Backend::Local { label, .. } => label,
            Backend::Scripted(_) => "the script",
        }
    }

    /// What tells it from another model.
    pub fn id(&self) -> String {
        match self {
            Backend::Api { model, .. } | Backend::ClaudeCode { model } | Backend::Local { model, .. } => model.clone(),
            Backend::Scripted(_) => "the script".into(),
        }
    }

    /// How many replies may be asked for ahead of the one under review: a
    /// local model answers one request at a time.
    pub fn asks_ahead(&self) -> usize {
        match self {
            Backend::Local { .. } => 1,
            _ => crate::story::PREFETCH,
        }
    }

    /// What's asked at first: the one chosen before, else the configured local
    /// model, else Claude.
    pub fn available() -> Option<Self> {
        models::remembered()
            .or_else(|| settings().local_model.clone().map(|model| Backend::Local { label: models::local_label(&model), model }))
            .or_else(|| Self::claude(&settings().model))
    }

    /// Claude's `model` through the API with a key in the environment, else Claude Code if it's installed.
    pub(crate) fn claude(model: &str) -> Option<Self> {
        if let Some(key) = std::env::var("ANTHROPIC_API_KEY").ok().filter(|k| !k.is_empty()).or_else(crate::credentials::saved) {
            return Some(Backend::Api { key, model: model.to_string() });
        }
        let on_path = std::env::var_os("PATH").is_some_and(|paths| ghostwrite_project::platform::on_path("claude", &paths).is_some());
        (on_path || local_claude().is_some()).then(|| Backend::ClaudeCode { model: model.to_string() })
    }

    /// Get ready to be asked soon, in `root`, resuming `session` if it's a
    /// follow-up: the user has started typing a request. A local model takes
    /// seconds to load; Claude Code a few tenths of a second to start.
    pub fn warm(&self, root: Option<&std::path::Path>, session: Option<&str>) {
        match self {
            Backend::Local { model, .. } => local::warm(model),
            Backend::ClaudeCode { model } => {
                let startup = Startup::new(model, root, session, false);
                std::thread::spawn(move || start_ahead(startup));
            }
            Backend::Api { .. } | Backend::Scripted(_) => {}
        }
    }

    /// It's no longer what's asked: give back what it holds.
    pub fn release(&self) {
        if matches!(self, Backend::Local { .. }) {
            local::unload();
        }
    }

    /// Stream the model's reply to `req` into `listener`. Blocks: run it on a
    /// background thread. Returns quietly once `cancel` is set.
    pub fn run(&self, req: &Request, listener: &mut dyn Listener, cancel: &AtomicBool) -> Result<(), String> {
        match self {
            Backend::Api { key, model } => converse(&mut Chat::new(key, model, API_URL, req), req, listener, cancel),
            Backend::ClaudeCode { model } => match run_claude_code(model, req, req.session.as_deref(), listener, cancel) {
                // The session is gone: start over with the whole conversation (not
                // after other errors, a rate limit say, which would only fail again, costlier).
                Err(e) if req.session.is_some() && e.contains("No conversation found") => run_claude_code(model, req, None, listener, cancel),
                result => result,
            },
            Backend::Local { model, .. } => {
                let ask = |req: &Request, listener: &mut dyn Listener| converse(&mut LocalChat::new(model, req), req, listener, cancel);
                fitting(req, listener, ask)
            }
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
    // Opened from a desktop there's no shell's PATH: where it's usually put.
    let home = std::env::home_dir()?;
    let places: Vec<PathBuf> = if cfg!(windows) {
        vec![home.join(r"AppData\Roaming\npm\claude.cmd"), home.join(r".local\bin\claude.exe")]
    } else {
        vec![home.join(".local/bin/claude"), home.join(".npm-global/bin/claude"), "/opt/homebrew/bin/claude".into(), "/usr/local/bin/claude".into()]
    };
    places.into_iter().find(|p| p.is_file())
}

fn effort() -> String {
    // Interactive, so brief thinking by default; override for harder asks.
    let effort = std::env::var("GHOSTWRITE_AI_EFFORT").ok().unwrap_or_default();
    if matches!(effort.as_str(), "low" | "medium" | "high" | "xhigh" | "max") { effort } else { "low".into() }
}

/// One way of having a model reply. `next` is the turn after a tool block: that
/// block, as it's to stand in the conversation, and what it returned.
trait Replier {
    fn reply(&mut self, next: Option<Turn<'_>>, listener: &mut dyn Listener, cancel: &AtomicBool) -> Result<(), String>;
    /// What one command may show.
    fn room(&self) -> usize;
    /// The next reply has to be an item, if that can be enforced.
    fn must_answer(&mut self) {}
    /// Whether it can be shown images and PDFs.
    fn sees_files(&self) -> bool {
        false
    }
}

/// A turn of looking: the block the model wrote, what came of it, and the files it read
/// that it's to see.
#[derive(Clone, Copy)]
struct Turn<'a> {
    block: &'a str,
    result: &'a str,
    files: &'a [Attachment],
}

impl Turn<'_> {
    /// What came of it, as a message says it: the files come before the words about them.
    fn content(&self) -> Value {
        if self.files.is_empty() {
            return json!(self.result);
        }
        let mut blocks: Vec<Value> = self.files.iter().map(Attachment::block).collect();
        blocks.push(json!({ "type": "text", "text": self.result }));
        Value::Array(blocks)
    }
}

/// The harness, the same for every model: its replies go to the review until
/// one is a `<tool>` block, whose commands are run here, all at once, for the
/// next turn to show.
fn converse(replier: &mut dyn Replier, req: &Request, listener: &mut dyn Listener, cancel: &AtomicBool) -> Result<(), String> {
    let room = replier.room();
    let mut seen = std::collections::HashSet::new();
    let mut looked = 0;
    let mut next: Option<(String, String, Vec<Attachment>)> = None;
    let sees = replier.sees_files();
    let mut shown = 0;
    // What the user let it read outside the project.
    let mut allowed: Vec<PathBuf> = vec![];
    for round in 0..MAX_TOOL_ROUNDS {
        let mut reply = ToolFilter::new(listener, req.root.is_some());
        replier.reply(next.as_ref().map(|(block, result, files)| Turn { block, result, files }), &mut reply, cancel)?;
        let block = reply.finish();
        if cancel.load(Ordering::SeqCst) {
            return Ok(());
        }
        let Some(block) = block else { return Ok(()) };
        let Some(root) = &req.root else { return Err("there's no project to look in".into()) };
        let commands = tools::commands(&block);
        listener.looking(commands.iter().map(|c| tools::describe(c)).collect::<Vec<_>>().join(", "));
        let fresh: Vec<&str> = commands.iter().copied().filter(|c| seen.insert(c.to_string())).collect();
        // Outside the project, the user's asked first, one at a time.
        for path in fresh.iter().filter_map(|command| tools::outside(root, command)) {
            if !allowed.iter().any(|ok| path.starts_with(ok)) && listener.may_read(&path) {
                allowed.push(path);
            }
            if cancel.load(Ordering::SeqCst) {
                return Ok(());
            }
        }
        let allowed = &allowed;
        let looked_at: Vec<tools::Looked> = std::thread::scope(|scope| {
            let running: Vec<_> = fresh.iter().map(|command| scope.spawn(move || tools::look(root, command, room, sees, allowed))).collect();
            running.into_iter().map(|r| r.join().unwrap_or(tools::Looked { text: String::new(), file: None })).collect()
        });
        let mut files = vec![];
        let mut outputs = vec![];
        for tools::Looked { text, file } in looked_at {
            match file {
                // What's been shown stays in the conversation, which has to fit in a request.
                Some(file) if shown + file.data.len() > MAX_SHOWN => {
                    outputs.push(format!("That's more files than can be shown at once: {} MB so far. Go on from what you've seen.", shown >> 20))
                }
                Some(file) => {
                    shown += file.data.len();
                    files.push(file);
                    outputs.push(text);
                }
                None => outputs.push(text),
            }
        }
        looked += outputs.iter().map(String::len).sum::<usize>();
        // A model going in circles, or out of room, is told to stop looking; if it can be, made to.
        let answer_now = fresh.is_empty() || looked >= 2 * room || round + 2 >= MAX_TOOL_ROUNDS;
        if answer_now {
            replier.must_answer();
        }
        let results: String =
            fresh.iter().zip(&outputs).map(|(command, output)| format!("<result of=\"{command}\">\n{}\n</result>\n", output.trim_end())).collect();
        let what = if fresh.is_empty() { "You've already seen what those return.\n" } else { "" };
        let then = if answer_now {
            "Now reply with one item, from what you've seen."
        } else {
            "Now reply with one item, or another <tool> block if you still need to look."
        };
        next = Some((format!("<tool>\n{}\n{END_TOOL}", commands.join("\n")), format!("{results}{what}{then}"), files));
    }
    Err("the model kept looking around without answering".into())
}

/// A reply on its way to the review, unless it's a `<tool>` block: words before
/// that go through, the block doesn't, and an item's lines are never taken for
/// one (it may be code about tags). Only a line that might still become one
/// waits, so an item isn't held up.
struct ToolFilter<'a> {
    listener: &'a mut dyn Listener,
    pending: String,
    block: Option<String>,
    /// Nothing more can be a block: there are no tools, or an item has started.
    through: bool,
    mid_line: bool,
}

impl<'a> ToolFilter<'a> {
    fn new(listener: &'a mut dyn Listener, tools: bool) -> Self {
        Self { listener, pending: String::new(), block: None, through: !tools, mid_line: false }
    }

    /// The block, if the reply was one. What's after its end isn't for anyone.
    fn finish(mut self) -> Option<String> {
        if let Some(block) = self.block.take() {
            let end = block.find(END_TOOL).unwrap_or(block.len());
            return Some(block[..end].to_string());
        }
        let rest = std::mem::take(&mut self.pending);
        if !rest.is_empty() {
            self.listener.text(&rest);
        }
        None
    }

    fn forward(&mut self, upto: usize) {
        let text: String = self.pending.drain(..upto).collect();
        self.mid_line = !text.ends_with('\n');
        self.listener.text(&text);
    }
}

impl Listener for ToolFilter<'_> {
    fn used(&mut self, input: u64, output: u64) {
        self.listener.used(input, output);
    }

    fn text(&mut self, chunk: &str) {
        if self.through {
            return self.listener.text(chunk);
        }
        if let Some(block) = &mut self.block {
            return block.push_str(chunk);
        }
        self.pending.push_str(chunk);
        while !self.pending.is_empty() {
            let end = self.pending.find('\n').map(|i| i + 1);
            if self.mid_line {
                self.forward(end.unwrap_or(self.pending.len()));
                continue;
            }
            let line = self.pending[..end.unwrap_or(self.pending.len())].trim_start();
            if let Some(rest) = line.strip_prefix("<tool>") {
                let start = self.pending.len() - self.pending.trim_start().len() + "<tool>".len();
                debug_assert!(rest.len() <= self.pending.len());
                self.block = Some(self.pending[start..].to_string());
                self.pending.clear();
                return;
            }
            // A tag, or the start of one: wait to see which.
            if end.is_none() && (line.is_empty() || line.starts_with('<')) {
                return;
            }
            if protocol::STARTS.iter().any(|tag| line.trim_end() == *tag) {
                self.through = true;
                let all = std::mem::take(&mut self.pending);
                return self.listener.text(&all);
            }
            self.forward(end.unwrap_or(self.pending.len()));
        }
    }

    fn looking(&mut self, what: String) {
        self.listener.looking(what);
    }

    fn session(&mut self, id: String) {
        self.listener.session(id);
    }
}

/// The whole conversation so far, as sent each turn. Turns are only ever added,
/// so what's cached of it (the model's, the API's) stays good, and only what's
/// new is read.
fn conversation(req: &Request) -> Vec<Value> {
    req.messages().into_iter().map(|(role, text)| json!({ "role": if role == Role::User { "user" } else { "assistant" }, "content": text })).collect()
}

/// The conversation with only its last turns whole. It starts the same every
/// time, so what's cached of the start stays good.
fn recent(req: &Request) -> Vec<Value> {
    let mut messages = conversation(req);
    let turns = req.followups.len();
    if turns <= LOCAL_TURNS_KEPT {
        return messages;
    }
    let dropped = &req.followups[..turns - LOCAL_TURNS_KEPT];
    let earlier: Vec<String> = dropped.iter().flat_map(|f| &f.decided).map(|d| format!("- {}", d.trim())).collect();
    messages.drain(1..1 + 2 * dropped.len());
    if !earlier.is_empty() {
        let next = messages[2]["content"].as_str().unwrap_or_default();
        messages[2]["content"] = json!(format!("Before that, the user decided:\n{}\n\n{next}", earlier.join("\n")));
    }
    messages
}

fn add_turn(messages: &mut Vec<Value>, next: Option<Turn<'_>>) {
    if let Some(turn) = next {
        messages.push(json!({ "role": "assistant", "content": turn.block }));
        messages.push(json!({ "role": "user", "content": turn.content() }));
    }
}

/// Ask with what's sent shrunk as far as it takes to fit the model's context, so
/// that never comes to the user: however big the project, a small step's prompt fits.
fn fitting(req: &Request, listener: &mut dyn Listener, mut ask: impl FnMut(&Request, &mut dyn Listener) -> Result<(), String>) -> Result<(), String> {
    let mut result = Ok(());
    for small in LOCAL_SHRINKS {
        result = ask(&Request { small, ..req.clone() }, listener);
        if !result.as_ref().is_err_and(|why| why.contains(protocol::NO_ROOM)) {
            return result;
        }
    }
    result
}

/// A local model, whose grammar holds it to the format.
struct LocalChat<'a> {
    model: &'a str,
    messages: Vec<Value>,
    /// The files it may name: its grammar has them, so it can't make one up.
    files: Vec<String>,
    tools: bool,
    /// It's only being asked something.
    question: bool,
}

impl<'a> LocalChat<'a> {
    fn new(model: &'a str, req: &Request) -> Self {
        let mut files = req.root.as_ref().map_or(vec![], |root| ghostwrite_project::files::files(root, root));
        files.extend(
            req.root.as_ref().and_then(|root| req.path.as_ref()?.strip_prefix(root).ok()).map(|p| p.display().to_string()).filter(|p| !files.contains(p)),
        );
        let mut messages = vec![json!({ "role": "system", "content": protocol::system() })];
        messages.extend(recent(req));
        let said = req.followups.iter().rev().filter(|f| !f.correction).map(|f| f.said.trim()).find(|s| !s.is_empty());
        Self { model, messages, files, tools: req.root.is_some(), question: protocol::is_question(said.unwrap_or(&req.instruction)) }
    }

    fn request(&self) -> Value {
        json!({ "messages": self.messages, "grammar": protocol::grammar(&self.files, self.tools, self.question), "temperature": 0.2, "max_tokens": LOCAL_MAX_TOKENS })
    }
}

impl Replier for LocalChat<'_> {
    fn reply(&mut self, next: Option<Turn<'_>>, listener: &mut dyn Listener, cancel: &AtomicBool) -> Result<(), String> {
        add_turn(&mut self.messages, next);
        local::ask(self.model, self.request(), listener, cancel)
    }

    fn room(&self) -> usize {
        LOCAL_ROOM
    }

    fn must_answer(&mut self) {
        self.tools = false;
    }
}

/// Claude through the Messages API.
struct Chat {
    url: String,
    key: String,
    model: String,
    agent: ureq::Agent,
    messages: Vec<Value>,
}

impl Chat {
    fn new(key: &str, model: &str, url: &str, req: &Request) -> Self {
        let agent = ureq::Agent::config_builder().http_status_as_error(false).timeout_global(Some(Duration::from_secs(300))).build().into();
        Self { url: url.to_string(), key: key.to_string(), model: model.to_string(), agent, messages: conversation(req) }
    }
}

impl Replier for Chat {
    fn sees_files(&self) -> bool {
        true
    }

    fn reply(&mut self, next: Option<Turn<'_>>, listener: &mut dyn Listener, cancel: &AtomicBool) -> Result<(), String> {
        add_turn(&mut self.messages, next);
        let body = json!({
            "model": self.model,
            "max_tokens": 32000,
            "stream": true,
            // Re-run a safety-classifier decline on Anthropic's recommended model for that category.
            "fallbacks": "default",
            "output_config": { "effort": effort() },
            "stop_sequences": [END_TOOL],
            // Cached: the system prompt (never changes), and the conversation
            // so far (each round and follow-up resends it).
            "system": [{ "type": "text", "text": protocol::system(), "cache_control": { "type": "ephemeral" } }],
            "cache_control": { "type": "ephemeral" },
            "messages": self.messages,
        });
        let response = self
            .agent
            .post(&self.url)
            .header("x-api-key", &self.key)
            .header("anthropic-version", "2026-10-21")
            .header("anthropic-beta", "server-side-fallback-2026-07-01")
            .send_json(&body)
            .map_err(|e| format!("couldn't reach Claude: {e}"))?;
        let status = response.status();
        let mut reader = BufReader::new(response.into_body().into_reader());
        if !status.is_success() {
            let mut text = String::new();
            std::io::Read::read_to_string(&mut reader, &mut text).ok();
            let message = serde_json::from_str::<Value>(&text).ok().and_then(|v| v["error"]["message"].as_str().map(str::to_string)).unwrap_or(text);
            return Err(format!("Claude API {status}: {message}"));
        }
        let mut read = 0;
        for line in reader.lines() {
            if cancel.load(Ordering::SeqCst) {
                return Ok(());
            }
            let line = line.map_err(|e| e.to_string())?;
            let Some(event) = line.strip_prefix("data:").and_then(|data| serde_json::from_str::<Value>(data.trim()).ok()) else { continue };
            if event["type"] == "message_start" {
                read = tokens_in(&event["message"]["usage"]);
            }
            if event["type"] == "message_delta"
                && let Some(written) = event["usage"]["output_tokens"].as_u64()
            {
                listener.used(read, written);
            }
            match event["type"].as_str() {
                Some("content_block_delta") if event["delta"]["type"] == "text_delta" => listener.text(event["delta"]["text"].as_str().unwrap_or_default()),
                Some("message_delta") if event["delta"]["stop_reason"] == "refusal" => return Err("Claude declined this request".into()),
                Some("error") => return Err(event["error"]["message"].as_str().unwrap_or("Claude API error").to_string()),
                _ => {}
            }
        }
        Ok(())
    }

    fn room(&self) -> usize {
        ROOM
    }
}

/// How a Claude Code is started: two started alike are interchangeable.
#[derive(Clone, Debug, PartialEq)]
struct Startup {
    model: String,
    effort: String,
    root: PathBuf,
    resume: Option<String>,
    fork: bool,
}

impl Startup {
    fn new(model: &str, root: Option<&std::path::Path>, resume: Option<&str>, fork: bool) -> Self {
        let root = root.map_or_else(std::env::temp_dir, std::path::Path::to_path_buf);
        Self { model: model.to_string(), effort: effort(), root, resume: resume.map(str::to_string), fork: fork && resume.is_some() }
    }

    fn spawn(&self) -> Result<Child, String> {
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
fn start_ahead(startup: Startup) {
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
fn take_started(startup: &Startup) -> Option<Child> {
    let mut started = STARTED.lock().unwrap_or_else(|e| e.into_inner());
    let ix = started.iter_mut().position(|(waiting, child, at)| waiting == startup && at.elapsed() < STARTED_FOR && child_running(child))?;
    Some(started.remove(ix).1)
}

fn child_running(child: &mut Child) -> bool {
    matches!(child.try_wait(), Ok(None))
}

/// Claude Code in the project folder, its own tools off. It stays up through
/// the rounds of a reply, so looking around doesn't start it again each time.
/// One started ahead ([`Backend::warm`]) is taken if it's like this one.
fn run_claude_code(model: &str, req: &Request, resume: Option<&str>, listener: &mut dyn Listener, cancel: &AtomicBool) -> Result<(), String> {
    let startup = Startup::new(model, req.root.as_deref(), resume, req.fork);
    // Resuming, the session has the conversation so far: only the newest turn is new.
    let message = if resume.is_some() { req.latest() } else { req.prompt() };
    let mut child = match take_started(&startup) {
        Some(child) => {
            listener.started(true);
            child
        }
        None => {
            listener.started(false);
            startup.spawn()?
        }
    };
    let stdin = child.stdin.take().ok_or("no stdin")?;
    let stdout = child.stdout.take().ok_or("no stdout")?;
    let stderr = child.stderr.take();
    // Stopped, it's killed at once, not when it next prints (it may be thinking for minutes).
    let (child, done) = (Mutex::new(child), AtomicBool::new(false));
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
                    return stop(&child);
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        });
        let mut session = ClaudeCode { stdin, lines: BufReader::new(stdout).lines(), child: &child, first: Some(message), session_told: false };
        let result = converse(&mut session, req, listener, cancel);
        // The reply is complete; don't wait for the process to wind down.
        stop(&child);
        done.store(true, Ordering::SeqCst);
        let errors = errors.join().unwrap_or_default();
        result.map_err(|e| if e.starts_with("claude exited") { format!("{e}: {}", errors.trim()) } else { e })
    })
}

struct ClaudeCode<'a> {
    stdin: std::process::ChildStdin,
    lines: std::io::Lines<BufReader<std::process::ChildStdout>>,
    child: &'a Mutex<Child>,
    first: Option<String>,
    session_told: bool,
}

impl Replier for ClaudeCode<'_> {
    fn sees_files(&self) -> bool {
        true
    }

    fn reply(&mut self, next: Option<Turn<'_>>, listener: &mut dyn Listener, cancel: &AtomicBool) -> Result<(), String> {
        // It remembers its own tool block; it only needs what that returned.
        let message = self.first.take().map(|first| json!(first)).or_else(|| next.map(|turn| turn.content())).unwrap_or(json!(""));
        let turn = json!({ "type": "user", "message": { "role": "user", "content": message } });
        writeln!(self.stdin, "{turn}").and_then(|()| self.stdin.flush()).map_err(|e| e.to_string())?;
        let mut line_ended = true;
        for line in self.lines.by_ref() {
            if cancel.load(Ordering::SeqCst) {
                return Ok(());
            }
            let Ok(value) = serde_json::from_str::<Value>(&line.map_err(|e| e.to_string())?) else { continue };
            if let Some(id) = value["session_id"].as_str().filter(|_| !self.session_told) {
                listener.session(id.to_string());
                self.session_told = true;
            }
            match value["type"].as_str() {
                Some("stream_event") => {
                    let event = &value["event"];
                    // A new block of text starts a line, so a tag at its start stands on a line of
                    // its own and the words before it stay apart.
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
                Some("result") if value["is_error"] == true => return Err(value["result"].as_str().unwrap_or("Claude Code failed").to_string()),
                Some("result") => {
                    listener.used(tokens_in(&value["usage"]), value["usage"]["output_tokens"].as_u64().unwrap_or(0));
                    return Ok(());
                }
                _ => {}
            }
        }
        if cancel.load(Ordering::SeqCst) {
            return Ok(());
        }
        let status = self.child.lock().unwrap().wait().map_err(|e| e.to_string())?;
        Err(format!("claude exited with {status}"))
    }

    fn room(&self) -> usize {
        ROOM
    }
}

/// Stop Claude Code and whatever it started: its whole process group. Not once
/// it's been waited for: its id could be another process's by then.
fn stop(child: &Mutex<Child>) {
    let mut child = child.lock().unwrap();
    if matches!(child.try_wait(), Ok(None)) {
        // SAFETY: kill(2) only sends a signal, to the group the child leads
        // (`process_group(0)`); it hasn't been reaped, so the id is still its.
        #[cfg(unix)]
        unsafe {
            libc::kill(-(child.id() as libc::pid_t), libc::SIGKILL)
        };
        // Windows has no such groups, but this stops what it started as well.
        #[cfg(windows)]
        {
            ghostwrite_project::platform::command("taskkill").args(["/T", "/F", "/PID", &child.id().to_string()]).output().ok();
            child.kill().ok();
        }
    }
    child.wait().ok();
}

#[cfg(test)]
mod tests {
    //! The harness, against a canned model and a stand-in API; and live checks
    //! against Claude Code and a local model: `cargo test -p ghostwrite-ai -- --ignored`.

    use std::sync::atomic::AtomicBool;

    use super::*;
    use crate::protocol::{self, SYSTEM};

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

    /// A server that answers a request for each of `rounds` with its events,
    /// as server-sent events, and hands back the requests it got.
    /// A project with a picture and a PDF in it, and a request about it.
    fn with_files() -> (std::path::PathBuf, Request) {
        let root = util_project("api-files");
        let mut picture = std::io::Cursor::new(vec![]);
        image::RgbImage::new(2, 2).write_to(&mut picture, image::ImageFormat::Png).unwrap();
        std::fs::write(root.join("figure.png"), picture.into_inner()).unwrap();
        std::fs::write(root.join("paper.pdf"), b"%PDF-1.4\nthe paper").unwrap();
        let req = asked_in(&root);
        (root, req)
    }

    #[test]
    fn claude_sees_images_and_pdfs_it_reads_before_the_words_about_them() {
        let (_root, req) = with_files();
        let (url, served) = stand_in(vec![
            &[
                r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
                r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"<tool>\nread figure.png\nread paper.pdf\nread util.py\n"}}"#,
                r#"{"type":"message_delta","delta":{"stop_reason":"stop_sequence"}}"#,
            ],
            &[
                r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"<done>\n</done>\n"}}"#,
                r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"}}"#,
            ],
        ]);
        let mut out = Collect::default();
        converse(&mut Chat::new("key", "a model", &url, &req), &req, &mut out, &AtomicBool::new(false)).unwrap();
        let requests = served.join().unwrap();
        let second = body_of(&requests[1]);
        let blocks = second["messages"].as_array().unwrap().last().unwrap()["content"].as_array().unwrap().clone();
        assert_eq!(blocks.iter().map(|block| block["type"].as_str().unwrap()).collect::<Vec<_>>(), ["image", "document", "text"]);
        assert_eq!(blocks[0]["source"]["media_type"], "image/png");
        assert_eq!(blocks[1]["source"]["media_type"], "application/pdf");
        assert_eq!(blocks[1]["source"]["data"], "JVBERi0xLjQKdGhlIHBhcGVy");
        let words = blocks[2]["text"].as_str().unwrap();
        assert!(
            words.contains("figure.png is an image") && words.contains("paper.pdf is a PDF") && words.contains("def load"),
            "and a text file reads as ever: {words}"
        );
    }

    #[test]
    fn a_model_that_cant_see_is_told_and_nothing_is_sent() {
        let (root, _) = with_files();
        for file in ["figure.png", "paper.pdf"] {
            let looked = tools::look(&root, &format!("read {file}"), 1000, false, &[]);
            assert!(looked.file.is_none() && looked.text.contains("this model can only read text"), "{}", looked.text);
        }
        let reading = tools::look(&root, "read util.py", 1000, false, &[]);
        assert!(reading.file.is_none() && reading.text.contains("def load"));
        assert!(tools::look(&root, "head figure.png", 1000, true, &[]).file.is_some(), "however it's read");
        assert!(tools::look(&root, "read figure.png:3", 1000, true, &[]).file.is_some());
        assert!(tools::look(&root, "read ../../outside.png", 1000, true, &[]).file.is_none(), "only what's in the project");
    }

    #[test]
    fn claude_is_never_sent_more_than_a_request_can_hold() {
        struct Sees(Vec<usize>);
        impl Replier for Sees {
            fn reply(&mut self, next: Option<Turn<'_>>, listener: &mut dyn Listener, _cancel: &AtomicBool) -> Result<(), String> {
                match next {
                    None => listener.text("<tool>\nread a.pdf\nread b.pdf\n</tool>"),
                    Some(turn) => {
                        self.0.push(turn.files.len());
                        assert!(turn.result.contains("more files than can be shown"), "{}", turn.result);
                        listener.text("<done>\n</done>\n");
                    }
                }
                Ok(())
            }
            fn room(&self) -> usize {
                1000
            }
            fn sees_files(&self) -> bool {
                true
            }
        }
        let (root, req) = with_files();
        let big = vec![b' '; MAX_SHOWN / 4 * 3];
        for name in ["a.pdf", "b.pdf"] {
            std::fs::write(root.join(name), [&b"%PDF-1.4\n"[..], &big].concat()).unwrap();
        }
        let mut replier = Sees(vec![]);
        converse(&mut replier, &req, &mut Collect::default(), &AtomicBool::new(false)).unwrap();
        assert_eq!(replier.0, [1], "one fits, and it's said of the other");
    }

    fn stand_in(rounds: Vec<&'static [&'static str]>) -> (String, std::thread::JoinHandle<Vec<String>>) {
        use std::io::{Read as _, Write as _};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let served = std::thread::spawn(move || {
            let mut requests = vec![];
            for events in rounds {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = vec![0; 1 << 20];
                let mut got = 0;
                // Headers, then as many body bytes as they say.
                loop {
                    got += stream.read(&mut request[got..]).unwrap();
                    let text = String::from_utf8_lossy(&request[..got]);
                    if let Some(end) = text.find("\r\n\r\n") {
                        let length =
                            text.lines().find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|n| n.trim().parse::<usize>().unwrap()));
                        if got >= end + 4 + length.unwrap_or(0) {
                            break;
                        }
                    }
                }
                let body: String = events.iter().map(|e| format!("data: {e}\n\n")).collect();
                write!(stream, "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\ncontent-length: {}\r\n\r\n{body}", body.len())
                    .unwrap();
                requests.push(String::from_utf8_lossy(&request[..got]).into_owned());
            }
            requests
        });
        (url, served)
    }

    fn body_of(request: &str) -> Value {
        serde_json::from_str(request.split_once("\r\n\r\n").unwrap().1).unwrap()
    }

    fn asked() -> Request {
        Request {
            path: None,
            language: "rust".into(),
            range: 0..0,
            text: "fn main() {}\n".into(),
            instruction: "what does this do?".into(),
            followups: vec![],
            root: None,
            others: vec![],
            surroundings: Default::default(),
            session: None,
            fork: false,
            small: 0,
        }
    }

    fn claude_code() -> Backend {
        Backend::ClaudeCode { model: settings().model.clone() }
    }

    fn asked_in(root: &std::path::Path) -> Request {
        Request { root: Some(root.to_path_buf()), ..asked() }
    }

    /// A model that replies with these, in turn, noting what it's given.
    #[derive(Default)]
    struct Canned {
        replies: Vec<&'static str>,
        next: Vec<(String, String)>,
        must_answer_at: Vec<usize>,
    }

    impl Replier for Canned {
        fn reply(&mut self, next: Option<Turn<'_>>, listener: &mut dyn Listener, _cancel: &AtomicBool) -> Result<(), String> {
            if let Some(Turn { block, result, .. }) = next {
                self.next.push((block.to_string(), result.to_string()));
            }
            listener.text(self.replies.remove(0));
            Ok(())
        }
        fn room(&self) -> usize {
            LOCAL_ROOM
        }
        fn must_answer(&mut self) {
            self.must_answer_at.push(self.next.len());
        }
    }

    const READS: &str = "<tool>\nread util.py\n</tool>\n";
    const DONE: &str = "<done>\n</done>\n";

    fn util_project(name: &str) -> std::path::PathBuf {
        let root = ghostwrite_project::scratch_dir(name);
        std::fs::write(root.join("util.py"), "def load(): pass\n").unwrap();
        root
    }

    #[test]
    fn a_tool_block_is_run_here_and_never_shown() {
        let root = util_project("tools-run");
        let mut model = Canned { replies: vec![READS, DONE], ..Default::default() };
        let mut out = Collect::default();
        converse(&mut model, &asked_in(&root), &mut out, &AtomicBool::new(false)).unwrap();
        assert_eq!(out.0, DONE, "the block isn't shown");
        assert_eq!(out.1, ["reading util.py"]);
        let (block, result) = &model.next[0];
        assert_eq!(block, "<tool>\nread util.py\n</tool>");
        assert!(result.contains("<result of=\"read util.py\">\n1│def load(): pass\n</result>"), "{result}");
    }

    #[test]
    fn reading_outside_the_project_is_asked_about_first() {
        /// Says yes or no, and notes what it was asked.
        struct Asks(bool, Vec<std::path::PathBuf>);
        impl Listener for Asks {
            fn text(&mut self, _: &str) {}
            fn looking(&mut self, _: String) {}
            fn may_read(&mut self, path: &std::path::Path) -> bool {
                self.1.push(path.to_path_buf());
                self.0
            }
        }
        let root = util_project("tools-asks");
        let elsewhere = ghostwrite_project::scratch_dir("tools-asks-elsewhere");
        std::fs::write(elsewhere.join("notes.txt"), "remember this\n").unwrap();
        let file = ghostwrite_project::platform::real(&elsewhere.join("notes.txt")).unwrap();
        let block: &'static str = Box::leak(format!("<tool>\nread {}\nread util.py\n</tool>\n", file.display()).into_boxed_str());
        for yes in [true, false] {
            let mut model = Canned { replies: vec![block, DONE], ..Default::default() };
            let mut asks = Asks(yes, vec![]);
            converse(&mut model, &asked_in(&root), &mut asks, &AtomicBool::new(false)).unwrap();
            assert_eq!(asks.1, std::slice::from_ref(&file), "asked once, about that file (not the project's)");
            let result = &model.next[0].1;
            assert_eq!(result.contains("remember this"), yes, "{result}");
            assert!(result.contains("def load"), "the project's file is read either way");
        }
    }

    #[test]
    fn a_model_going_in_circles_is_made_to_answer() {
        let root = util_project("tools-circles");
        let mut model = Canned { replies: vec![READS, READS, DONE], ..Default::default() };
        converse(&mut model, &asked_in(&root), &mut Collect::default(), &AtomicBool::new(false)).unwrap();
        assert_eq!(model.must_answer_at, [1], "the second time, not the first");
        assert!(model.next[1].1.contains("already seen"), "{}", model.next[1].1);
    }

    #[test]
    fn several_commands_all_come_back() {
        let root = util_project("tools-several");
        let mut model = Canned { replies: vec!["<tool>\nread util.py\nlist\n</tool>\n", DONE], ..Default::default() };
        converse(&mut model, &asked_in(&root), &mut Collect::default(), &AtomicBool::new(false)).unwrap();
        let result = &model.next[0].1;
        assert!(result.contains("<result of=\"read util.py\">") && result.contains("<result of=\"list\">"), "{result}");
    }

    #[test]
    fn a_local_models_request_holds_it_to_the_format() {
        let root = util_project("tools-grammar");
        let req = asked_in(&root);
        let mut chat = LocalChat::new("a model", &req);
        let request = chat.request();
        assert_eq!(request["messages"][0]["content"], SYSTEM);
        assert_eq!(request["messages"][1]["role"], "user");
        let grammar = request["grammar"].as_str().unwrap();
        assert!(grammar.contains("\"util.py\"") && grammar.contains("tool ::="), "{grammar}");
        chat.must_answer();
        assert!(!chat.request()["grammar"].as_str().unwrap().contains("tool ::="), "it can only answer");
        // What's cached is only ever added to.
        let before = chat.messages.clone();
        add_turn(&mut chat.messages, Some(Turn { block: "<tool>\nlist\n</tool>", result: "results", files: &[] }));
        assert_eq!(chat.messages[..before.len()], before[..]);
    }

    #[test]
    fn a_local_model_gets_earlier_steps_as_a_line_each() {
        let turn = |n: usize| crate::FollowUp {
            reply: format!("reply {n}"),
            decided: vec![format!("accepted: step {n}")],
            said: String::new(),
            correction: false,
            terminal: None,
        };
        let req = Request { followups: (0..5).map(turn).collect(), ..asked() };
        let all = conversation(&req);
        let kept = recent(&req);
        assert_eq!(all.len(), 11);
        assert_eq!(kept.len(), 5, "the request, and two turns");
        assert_eq!(kept[0], all[0], "it starts the same every time");
        assert_eq!((&kept[1]["content"], &kept[3]["content"]), (&json!("reply 3"), &json!("reply 4")));
        let summary = kept[2]["content"].as_str().unwrap();
        assert!(summary.contains("- accepted: step 0") && summary.contains("- accepted: step 2"), "{summary}");
        assert!(!summary.contains("reply 0"));
    }

    #[test]
    fn what_doesnt_fit_is_shrunk_until_it_does() {
        let mut tried = vec![];
        let result = fitting(&asked(), &mut Collect::default(), |req, listener| {
            tried.push(req.small);
            if req.small < 4 {
                return Err(format!("{}: 14014 tokens", protocol::NO_ROOM));
            }
            listener.text("fits");
            Ok(())
        });
        assert_eq!((result, tried), (Ok(()), vec![1, 2, 4]));
        let mut tried = 0;
        let error = fitting(&asked(), &mut Collect::default(), |_, _| {
            tried += 1;
            Err(protocol::NO_ROOM.to_string())
        });
        assert_eq!(tried, 4, "as far as it goes");
        assert!(error.unwrap_err().contains(protocol::NO_ROOM));
        let mut tried = 0;
        let other = fitting(&asked(), &mut Collect::default(), |_, _| {
            tried += 1;
            Err("the model crashed".to_string())
        });
        assert_eq!((other.unwrap_err().as_str(), tried), ("the model crashed", 1), "only what's about room");
    }

    /// A change of several steps, each accepted as the editor does: it's applied, and
    /// the next is asked for.
    #[test]
    #[ignore]
    fn a_local_model_goes_on_step_by_step() {
        let root = ghostwrite_project::scratch_dir("live-steps");
        let path = root.join("calc.py");
        std::fs::write(&path, "def add(a, b):\n    pass\n\n\ndef sub(a, b):\n    pass\n\n\ndef mul(a, b):\n    pass\n\n\ndef div(a, b):\n    pass\n").unwrap();
        let mut followups: Vec<crate::FollowUp> = vec![];
        let mut done = 0;
        for round in 0..8 {
            let text = std::fs::read_to_string(&path).unwrap();
            let req = Request {
                path: Some(path.clone()),
                language: "python".into(),
                range: 0..text.len(),
                text: text.clone(),
                instruction: "implement all four functions".into(),
                followups: followups.clone(),
                root: Some(root.clone()),
                ..asked()
            };
            let mut out = Collect::default();
            local_model().run(&req, &mut out, &AtomicBool::new(false)).unwrap();
            println!("--- round {round} {:?}\n{}", out.1, out.0);
            if followups.iter().any(|f: &crate::FollowUp| f.reply.trim() == out.0.trim()) {
                followups.push(crate::FollowUp { reply: out.0, decided: vec![], said: protocol::SAID_ALREADY.into(), correction: true, terminal: None });
                continue;
            }
            let decided = match protocol::parse(&out.0, true) {
                protocol::Reply::Step(step) => {
                    let near = step.line.map_or(0..1, |l| l..l + step.old.len());
                    let Some(edit) = crate::placement::edit(&text, &step, near.clone()) else {
                        let said = protocol::misplaced("calc.py", &text, &step.old, near);
                        followups.push(crate::FollowUp { reply: out.0, decided: vec![], said, correction: true, terminal: None });
                        continue;
                    };
                    let mut text = text;
                    text.replace_range(edit.bytes, &edit.text);
                    std::fs::write(&path, text).unwrap();
                    done += 1;
                    vec![format!("accepted: {}", crate::Item::Step(step).describe())]
                }
                protocol::Reply::Done { .. } => break,
                other => {
                    println!("not a step: {other:?}");
                    vec![]
                }
            };
            followups.push(crate::FollowUp { reply: out.0, decided, said: String::new(), correction: false, terminal: None });
        }
        let text = std::fs::read_to_string(&path).unwrap();
        println!("=== after {done} steps\n{text}");
        assert!(["a + b", "a - b", "a * b", "a / b"].iter().all(|op| text.contains(op)), "{text}");
    }

    /// As in a data project: thousands of files, a long file, a full terminal.
    #[test]
    #[ignore]
    fn a_local_model_fits_a_big_project() {
        let root = ghostwrite_project::scratch_dir("live-big");
        for i in 0..5000 {
            std::fs::write(root.join(format!("mesh_{i:05}.off")), "x").unwrap();
        }
        let text: String = (0..20_000).map(|i| format!("def f{i}(mesh_dir, data_dir, fig_dir, mesh_ext=\".off\"):\n    return {i}\n")).collect();
        std::fs::write(root.join("big.py"), &text).unwrap();
        let at = text.len() / 2;
        let surroundings = crate::Surroundings { terminal: Some((0..50_000).map(|i| format!("output {i}\n")).collect()), ..Default::default() };
        let req = Request {
            path: Some(root.join("big.py")),
            language: "python".into(),
            range: at..at,
            text,
            instruction: "what does this function return?".into(),
            root: Some(root.clone()),
            surroundings,
            ..asked()
        };
        let mut out = Collect::default();
        local_model().run(&req, &mut out, &AtomicBool::new(false)).unwrap();
        println!("{:?}\n{}", out.1, out.0);
        assert_eq!(protocol::check(&out.0, true), Ok(()), "{}", out.0);
    }

    #[test]
    fn claude_through_the_api_uses_the_same_block() {
        let root = util_project("api-tools");
        let (url, served) = stand_in(vec![
            &[
                r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
                r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"<tool>\nread util.py\n"}}"#,
                r#"{"type":"message_delta","delta":{"stop_reason":"stop_sequence"}}"#,
            ],
            &[
                r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"<done>\n</done>\n"}}"#,
                r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"}}"#,
            ],
        ]);
        let mut out = Collect::default();
        let req = asked_in(&root);
        converse(&mut Chat::new("key", "a model", &url, &req), &req, &mut out, &AtomicBool::new(false)).unwrap();
        assert_eq!(out.0, DONE);
        assert_eq!(out.1, ["reading util.py"]);
        let requests = served.join().unwrap();
        let first = body_of(&requests[0]);
        assert_eq!(first["stop_sequences"][0], "</tool>", "it stops at the block's end");
        assert!(first.get("tools").is_none());
        assert_eq!(first["system"][0]["text"], SYSTEM);
        let second = body_of(&requests[1]);
        assert!(second["messages"].as_array().unwrap().last().unwrap()["content"].as_str().unwrap().contains("def load"));
    }

    /// Forwards what's streamed, to see just how it's split.
    fn filtered(chunks: &[&str]) -> (String, Option<String>) {
        let mut out = Collect::default();
        let mut filter = ToolFilter::new(&mut out, true);
        for chunk in chunks {
            filter.text(chunk);
        }
        let block = filter.finish();
        (out.0, block)
    }

    #[test]
    fn only_a_tool_block_is_held_back() {
        assert_eq!(filtered(&["<to", "ol>\nread a.py\n", "</tool>\n"]), (String::new(), Some("\nread a.py\n".into())));
        assert_eq!(filtered(&["<tool>read a.py</tool>"]), (String::new(), Some("read a.py".into())), "all on a line");
        assert_eq!(filtered(&["<tool>\nread a.py\n</tool>\nand then it made up the result"]).1, Some("\nread a.py\n".into()));
        assert_eq!(filtered(&["Let me look.\n", "<tool>\nls\n</tool>\n"]), ("Let me look.\n".into(), Some("\nls\n".into())), "words before it go on");
        assert_eq!(filtered(&["<file>\n", "a.py\n</file>\n"]), ("<file>\na.py\n</file>\n".into(), None));
        assert_eq!(filtered(&["<file>\na.py\n</file>\n<new>\n<tool>\n</new>\n"]).1, None, "code in an item isn't a block");
        assert_eq!(filtered(&["<sa"]), ("<sa".into(), None), "what's waiting isn't lost");
    }

    #[test]
    fn an_item_isnt_held_up_once_its_started() {
        let mut out = Collect::default();
        let mut filter = ToolFilter::new(&mut out, true);
        filter.text("<file>\n");
        filter.text("half a li");
        drop(filter);
        assert_eq!(out.0, "<file>\nhalf a li");
    }

    /// What's asked of a live local model: `GHOSTWRITE_LOCAL_MODEL`, a Hugging Face
    /// name or a `.gguf`, else the 14B one. `GHOSTWRITE_MODEL_WORKER` has to be a
    /// built ghostwrite.
    fn local_model() -> Backend {
        let model = std::env::var("GHOSTWRITE_LOCAL_MODEL").unwrap_or_else(|_| "bartowski/Qwen2.5-Coder-14B-Instruct-GGUF:Q3_K_M".into());
        Backend::Local { label: models::local_label(&model), model }
    }

    #[test]
    #[ignore]
    fn a_local_model_makes_an_edit() {
        let root = ghostwrite_project::scratch_dir("live-local-edit");
        let text = "def add(a, b):\n    pass\n";
        std::fs::write(root.join("calc.py"), text).unwrap();
        let req = Request {
            path: Some(root.join("calc.py")),
            language: "python".into(),
            range: 0..text.len(),
            text: text.into(),
            instruction: "make it return the sum".into(),
            root: Some(root.clone()),
            ..asked()
        };
        let mut out = Collect::default();
        local_model().run(&req, &mut out, &AtomicBool::new(false)).unwrap();
        println!("{:?}\n{}", out.1, out.0);
        assert_eq!(protocol::check(&out.0, true), Ok(()), "{}", out.0);
        let protocol::Reply::Step(step) = protocol::parse(&out.0, true) else { panic!("an edit: {}", out.0) };
        assert!(step.new.join("\n").contains("return") && crate::placement::edit(text, &step, 0..2).is_some(), "{}", out.0);
    }

    /// The file that answers it isn't shown: it has to be looked at, with the tools.
    fn asked_about_another_file(root: &std::path::Path, text: &str) -> Request {
        Request {
            path: Some(root.join("main.py")),
            language: "python".into(),
            range: 0..text.len(),
            text: text.into(),
            instruction: "what does load return when the file is missing? read util.py to find out".into(),
            root: Some(root.to_path_buf()),
            ..asked()
        }
    }

    #[test]
    #[ignore]
    fn a_local_model_reads_with_the_tools() {
        let (root, main) = two_file_project();
        let mut out = Collect::default();
        local_model().run(&asked_about_another_file(&root, &main), &mut out, &AtomicBool::new(false)).unwrap();
        println!("looked at: {:?}\n{}", out.1, out.0);
        assert_eq!(protocol::check(&out.0, true), Ok(()), "{}", out.0);
        assert!(out.1.iter().any(|w| w.contains("util.py")), "{:?}", out.1);
    }

    #[test]
    #[ignore]
    fn claude_code_reads_with_the_tools() {
        let (root, main) = two_file_project();
        let mut out = Collect::default();
        claude_code().run(&asked_about_another_file(&root, &main), &mut out, &AtomicBool::new(false)).unwrap();
        println!("looked at: {:?}\n{}", out.1, out.0);
        assert!(out.1.iter().any(|w| w.contains("util.py")), "{:?}", out.1);
        assert_eq!(protocol::check(&out.0, true), Ok(()), "{}", out.0);
        assert!(out.0.contains("default"), "{}", out.0);
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
            small: 0,
        };
        let mut out = Collect::default();
        claude_code().run(&req, &mut out, &AtomicBool::new(false)).unwrap();
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
        let followup = crate::FollowUp { reply: first, decided: vec![], said: "why int and not float?".into(), correction: false, terminal: None };
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
            small: 0,
        };
        let mut out = Collect::default();
        claude_code().run(&req, &mut out, &AtomicBool::new(false)).unwrap();
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
            followups.push(crate::FollowUp { reply: out, decided: vec![], said: String::new(), correction: false, terminal: None });
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
                small: 0,
            };
            let mut out = Collect::default();
            claude_code().run(&req, &mut out, &AtomicBool::new(false)).unwrap();
            println!("{}", out.0);
            let step = match item(&out.0) {
                protocol::Reply::Step(step) => step,
                protocol::Reply::Done { .. } => break,
                protocol::Reply::Note(_) | protocol::Reply::Run(_) => {
                    followups.push(crate::FollowUp { reply: out.0, decided: vec![], said: String::new(), correction: false, terminal: None });
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
            followups.push(crate::FollowUp { reply: out.0, decided, said: String::new(), correction: false, terminal: None });
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
            vec![crate::FollowUp { reply: first, decided: vec![], said, correction: false, terminal: None }],
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
            small: 0,
        };
        let mut first = Collect::default();
        claude_code().run(&req, &mut first, &AtomicBool::new(false)).unwrap();
        let session = first.2.clone().expect("a session");
        // Ahead: as if the first item were accepted, in a fork.
        req.followups.push(crate::FollowUp {
            reply: first.0.clone(),
            decided: vec!["accepted: it".into()],
            said: String::new(),
            correction: false,
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
        claude_code().run(&req, &mut ahead, &AtomicBool::new(false)).unwrap();
        println!("ahead:\n{}", ahead.0);
        assert_ne!(ahead.2.as_deref(), Some(session.as_str()), "the fork is a session of its own");
        assert_eq!(protocol::check(&ahead.0, true), Ok(()), "{}", ahead.0);
        // Then the user redirected it after all: the original session goes on as it was.
        req.followups[0] = crate::FollowUp { decided: vec![], said: "what did I just do?".into(), ..req.followups[0].clone() };
        req.text = text.into();
        req.fork = false;
        let mut real = Collect::default();
        claude_code().run(&req, &mut real, &AtomicBool::new(false)).unwrap();
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
            small: 0,
        };
        let mut first = Collect::default();
        claude_code().run(&req, &mut first, &AtomicBool::new(false)).unwrap();
        let session = first.2.clone().expect("a session to resume");
        req.followups.push(crate::FollowUp {
            reply: first.0,
            decided: vec![],
            said: "you may say it now: what was the codeword in my first message?".into(),
            correction: false,
            terminal: None,
        });
        req.session = Some(session);
        assert!(!req.latest().contains("PELICAN"), "only the newest turn is sent");
        let mut second = Collect::default();
        claude_code().run(&req, &mut second, &AtomicBool::new(false)).unwrap();
        println!("{}", second.0);
        assert!(second.0.contains("PELICAN"), "the resumed session remembers: {}", second.0);
    }
}
