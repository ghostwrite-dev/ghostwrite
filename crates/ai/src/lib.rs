//! Claude in the editor.
//!
//! You point at some lines and say what you want. Claude answers either with
//! notes pinned to the code (a question) or with small whole-line steps and
//! file operations (a change); nothing changes until you accept a step. The
//! [`Story`] is that exchange as you review it, follow-ups included.
//!
//! - [`protocol`]: the reply format, parsed as it streams.
//! - [`context`]: what Claude is told: the file, the project, recent
//!   interactions, and its own notes from earlier sessions.
//! - [`memory`]: those notes and interactions, kept per project.
//! - [`backend`]: the Messages API, Claude Code or a local model, streaming.
//! - [`local`]: asking a local model.
//! - [`models`]: every model that can be asked, and the one chosen.
//! - [`tools`]: the `<tool>` commands, the same for every model.
//! - [`story`]: the review state machine.
//! - [`lines`]: files shown with numbered lines, and finding the lines an item quotes.
//! - [`placement`]: which file and lines an item applies to, and the edit a step makes.
//! - [`layout`]: where each item shows in the code.

pub mod attach;
pub mod backend;
pub mod context;
pub mod credentials;
pub mod layout;
pub mod lines;
pub mod local;
pub mod memory;
pub mod models;
pub mod placement;
pub mod protocol;
pub mod story;
pub mod tools;

pub use backend::Backend;
pub use context::{Interaction, Request, Role, Surroundings};
pub use layout::Layout;
pub use story::{FollowUp, Item, ItemId, Settled, Story, Verdict};

/// Settings the app reads from its config and hands over at launch.
pub struct Settings {
    pub model: String,
    /// A local model to offer, asked at first: Hugging Face's
    /// `user/repo:quantization` or a `.gguf` file, run with llama.cpp in a process of its own.
    pub local_model: Option<String>,
    /// Rules plugins add to the model's prompt.
    pub prompt_rules: String,
    /// Bytes of the file around the marked lines sent as context (a local model is sent less).
    pub context_bytes: usize,
    /// Bytes of uncommitted changes sent.
    pub max_diff: usize,
}

impl Default for Settings {
    fn default() -> Self {
        Self { model: "claude-opus-5-5".into(), local_model: None, prompt_rules: String::new(), context_bytes: 200_000, max_diff: 12_000 }
    }
}

static SETTINGS: std::sync::OnceLock<Settings> = std::sync::OnceLock::new();

/// Set the settings, once, before the first request.
pub fn configure(settings: Settings) {
    SETTINGS.set(settings).ok();
}

pub(crate) fn settings() -> &'static Settings {
    SETTINGS.get_or_init(Settings::default)
}
