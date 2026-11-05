//! The registers yanked text goes to.

use std::collections::HashMap;

/// The yank register, shared by all documents, and the named ones (`"a y`).
#[derive(Default)]
pub struct Registers {
    pub yanked: String,
    pub named: HashMap<char, String>,
}
