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
//! - [`layout`]: where each item shows: in the code, or in the panel it was asked from.

pub mod backend;
pub mod context;
pub mod layout;
pub mod placement;
pub mod protocol;
pub mod story;
pub mod tools;

pub use backend::Backend;
pub use context::{Interaction, Request, Role, Surroundings};
pub use layout::{Layout, Origin, Spot};
pub use story::{FollowUp, Item, ItemId, Part, Settled, Story, Verdict};

pub const MODEL: &str = "claude-opus-5-5";
