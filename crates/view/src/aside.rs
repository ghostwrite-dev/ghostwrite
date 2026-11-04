//! What's beside a file, if a plugin has something for its language: a
//! [`ghostwrite_plugin::Panel`], which says what to ask the file's language server
//! about the cursor and what to show of the answers. It's kept on the document and
//! kept up to date as the cursor moves and the file is edited; the app asks and draws.

use ghostwrite_plugin::Panel;

use crate::latest::Latest;

/// A document's panel, and the editor's side of keeping it up to date.
pub struct Aside {
    pub panel: Box<dyn Panel>,
    /// The buffer version and cursor asked about last.
    pub asked: Option<(u64, usize)>,
    /// Only the latest answer counts: a server may answer only once it's got as far
    /// as the cursor, so a cursor that's moved on doesn't wait for the old one.
    pub latest: Latest,
}

impl Aside {
    /// `panel`, not asked anything yet.
    pub fn new(panel: Box<dyn Panel>) -> Self {
        Self { panel, asked: None, latest: Latest::default() }
    }
}
