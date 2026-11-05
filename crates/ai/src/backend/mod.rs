//! Reaching the model: Claude through the Messages API or Claude Code (`claude
//! -p`, under the user's login), or a local model through [`local`](crate::local). They differ only in how a turn is
//! sent: every one gets the same prompt, and looks around the same way, with
//! `<tool>` blocks run here ([`converse()`], [`tools`](crate::tools)). The reply streams in as
//! text; what the model looks at is reported as it happens.
//!
//! A story is one conversation. The API and a local model are sent every turn;
//! Claude Code keeps a session, reported back with [`Listener::session`], and
//! each follow-up resumes it with only the newest turn (so it still has the
//! files it read). If resuming fails, the whole conversation is sent anew.

mod api;
mod claude_code;
mod converse;
mod local;
#[cfg(test)]
mod tests;
mod warm;

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use serde_json::Value;

use crate::context::Request;
use crate::{models, settings};
use api::{API_URL, Chat};
use claude_code::run_claude_code;
use converse::converse;
use local::{LocalChat, fitting};
use warm::{Startup, start_ahead};

/// What one command may show, and what a reply may read in all before it has
/// to answer: a local model's context is small, and has to fit the answer too.
const ROOM: usize = 120_000;
const LOCAL_ROOM: usize = 4_000;

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
    /// model, else Claude through the API or Claude Code.
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
            Backend::Local { model, .. } => crate::local::warm(model),
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
            crate::local::unload();
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
