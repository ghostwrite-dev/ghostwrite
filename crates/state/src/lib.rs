//! The editor's state, as helix's `helix-view`: the [`Project`] with its open
//! [`Document`]s, each in a [`Tab`] with how it's being looked at, and what the
//! keys work on besides (the registers, the jump list, the search, what `.`
//! and macros play), and the story under review, with what it decides. Nothing here draws or knows about gpui: the app holds
//! this, drives it from keys and the mouse, and draws it.

pub mod aside;
pub mod commands;
pub mod config;
pub mod cursors;
pub mod document;
pub mod find;
pub mod jumps;
pub mod latest;
pub mod plugin_host;
pub mod project;
pub mod registers;
pub mod rendered;
pub mod repeat;
pub mod semantic;
pub mod story;
pub mod tabs;

pub use document::{DocId, Document};
pub use project::Project;
pub use tabs::{Autoscroll, Tab, Tabs};
