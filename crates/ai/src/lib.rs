//! Claude in the editor.
//!
//! You point at some lines and say what you want. Claude answers either with
//! notes pinned to the code (a question) or with small whole-line steps and
//! file operations (a change); nothing changes until you accept a step. The
//! [`Story`] is that exchange as you review it, follow-ups included.
//!
//! - [`protocol`]: the reply format, parsed as it streams, and placing steps.
//! - [`context`]: what Claude is told: the file, the project, recent
//!   interactions, and its own notes from earlier sessions.
//! - [`backend`]: the Messages API or Claude Code, streaming.
//! - [`tools`]: read-only project tools for the API backend.
//! - [`story`]: the review state machine.
//! - [`placement`]: which file and lines an item applies to, and the edit a step makes.
//! - [`layout`]: where each item shows in the code.

pub mod backend;
pub mod context;
pub mod layout;
pub mod placement;
pub mod protocol;
pub mod story;
pub mod tools;

pub use backend::Backend;
pub use context::{Interaction, Request, Role, Surroundings};
pub use layout::Layout;
pub use story::{FollowUp, Item, ItemId, Part, Settled, Story, Verdict};

/// Settings the app reads from its config and hands over at launch.
pub struct Settings {
    pub model: String,
    /// Bytes of the file around the marked lines sent as context.
    pub context_bytes: usize,
    /// Bytes of uncommitted changes sent.
    pub max_diff: usize,
}

impl Default for Settings {
    fn default() -> Self {
        Self { model: "claude-opus-5-5".into(), context_bytes: 200_000, max_diff: 12_000 }
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
