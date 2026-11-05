//! What plugins hear of files and are shown of them, and edits they ask for,
//! checked before they're made. The app asks them, off the UI thread.

use std::ops::Range;
use std::path::Path;

use ghostwrite_plugin::{Context as Shown, Event};

use crate::document::DocId;
use crate::project::Project;
use crate::tabs::Tab;

/// Something that happened to a file, for the plugins that want to hear of it.
pub struct Heard {
    pub event: Event,
    pub doc: DocId,
    /// Its text's version then.
    pub version: u64,
    /// What it was then.
    pub shown: Shown,
}

/// What a plugin is shown of `tab`.
pub fn shown(root: &Path, tab: &Tab) -> Shown {
    let selection = tab.view.selection;
    Shown {
        root: root.display().to_string(),
        path: tab.doc.path.as_ref().map(|path| path.display().to_string()),
        language: tab.doc.language.map(|language| language.name.to_string()),
        text: tab.doc.buffer.whole_text().to_string(),
        cursor: tab.cursor(),
        selection: selection.from()..selection.to(),
    }
}

impl Project {
    /// `event` happened to tab `ix`, if any plugin wants to know.
    pub(crate) fn tell_plugins(&mut self, ix: usize, event: Event) {
        if ghostwrite_plugin::hearing(event).is_empty() {
            return;
        }
        let Some(tab) = self.tabs.get(ix).filter(|tab| !tab.doc.read_only()) else { return };
        let heard = Heard { event, doc: tab.doc.id, version: tab.doc.buffer.version(), shown: shown(&self.root, tab) };
        self.heard.push(heard);
    }
}

/// `edits` done to `tab`, as one undo step, if they're all sound; they're all about
/// the text as it is, whatever order they come in.
pub fn apply_edits(tab: &mut Tab, mut edits: Vec<(Range<usize>, String)>) -> Result<(), String> {
    if tab.doc.read_only() {
        return Err("this file is read-only".into());
    }
    let text = &tab.doc.buffer;
    edits.sort_by_key(|(range, _)| range.start);
    let mut reached = 0;
    for (range, _) in &edits {
        if range.start > range.end || range.end > text.len() || !text.is_char_boundary(range.start) || !text.is_char_boundary(range.end) {
            return Err(format!("its edit of {}..{} isn't in the text", range.start, range.end));
        }
        if range.start < reached {
            return Err("its edits overlap".into());
        }
        reached = range.end;
    }
    edits.retain(|(range, new)| text.slice(range.clone()) != **new);
    if edits.is_empty() {
        return Ok(());
    }
    // Where a place in the text ends up: inside an edit, after what it put there.
    let moved = |at: usize| {
        let mut shift = 0isize;
        for (range, new) in &edits {
            if at < range.start || (at == range.start && range.is_empty()) {
                break;
            }
            if at < range.end {
                return (range.start as isize + shift) as usize + new.len();
            }
            shift += new.len() as isize - range.len() as isize;
        }
        (at as isize + shift) as usize
    };
    let selection = tab.view.selection;
    let after = ghostwrite_core::Range::new(moved(selection.anchor), moved(selection.head));
    tab.doc.buffer.begin(selection);
    for (range, new) in edits.iter().rev() {
        tab.doc.buffer.edit(range.clone(), new);
    }
    tab.doc.buffer.commit(after);
    tab.view.selection = after;
    tab.sync();
    Ok(())
}
