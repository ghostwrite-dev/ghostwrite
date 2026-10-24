//! Where a story's items show: a step where its lines are, a note at the
//! lines it's pinned to. An answer that isn't pinned to lines shows where
//! it was asked: under the item a follow-up was about, or under the marked
//! lines.
//!
//! [`Layout`] works this out from the story, what it was asked about, and
//! the project's files; the editor only turns the result into tabs.

use std::ops::Range;
use std::path::PathBuf;

use crate::placement::{self, Asked, Files, Place, Placer};
use crate::protocol::{self, Note};
use crate::story::{Item, ItemId, Story};

pub struct Layout<'a, F> {
    pub story: &'a Story,
    pub asked: Asked<'a>,
    pub files: F,
    pub placer: &'a Placer,
}

impl<F: Files> Layout<'_, F> {
    /// Where item `id` shows.
    pub fn spot(&self, id: ItemId) -> Option<Place> {
        let item = self.story.item(id)?;
        let unpinned = matches!(&item, Item::Note(Note { file: None, at, .. }) if at.is_empty());
        // A follow-up's answer goes where it was asked.
        if unpinned
            && let Some(lines) = self.story.asked_about(id).and_then(|about| self.lines(about))
        {
            return Some(lines);
        }
        Some(self.placer.place(&item, &self.asked, &self.files))
    }

    /// The lines item `id` shows at, in its file (`near` is those lines).
    pub fn lines(&self, id: ItemId) -> Option<Place> {
        let place = self.spot(id)?;
        let text = match &place.path {
            None => self.asked.text.to_string(),
            Some(path) => self.files.text(path)?,
        };
        let near: Range<usize> = match self.story.item(id)? {
            Item::Step(step) => {
                let view = placement::view(&text, &step, place.near.clone())?;
                view.first..view.anchor + 1
            }
            Item::Note(note) => protocol::locate(&text, &note.at, place.near.clone())?,
            _ => return None,
        };
        Some(Place { near, ..place })
    }

    /// Where Claude's answer is coming, while a reply is on its way and
    /// there's nothing to review: under what it was asked about, or under
    /// the marked lines before the first reply.
    pub fn waiting_spot(&self) -> Option<Place> {
        if let Some(about) = self.story.writing_about() {
            return self.lines(about);
        }
        Some(Place { path: None, near: self.asked.lines.clone() })
    }

    /// The file a spot is in: its own, or the one asked about.
    pub fn file(&self, place: &Place) -> Option<PathBuf> {
        place.path.clone().or_else(|| self.asked.path.map(PathBuf::from))
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::*;
    use crate::story::Part;

    struct Project(Vec<(PathBuf, String)>);

    impl Files for &Project {
        fn text(&self, path: &Path) -> Option<String> {
            self.0.iter().find(|(p, _)| p == path).map(|(_, t)| t.clone())
        }
        fn all(&self) -> Vec<PathBuf> {
            self.0.iter().map(|(p, _)| p.clone()).collect()
        }
    }

    fn arrived(story: &mut Story, reply: &str) {
        story.push_text(reply);
        story.finished_writing();
        story.settle();
    }

    const MAIN: &str = "fn main() { run(); }\n";

    fn layout<'a>(story: &'a Story, project: &'a Project, placer: &'a Placer) -> Layout<'a, &'a Project> {
        let main = Path::new("/p/main.rs");
        Layout { story, asked: Asked { path: Some(main), text: MAIN, lines: 0..1 }, files: project, placer }
    }

    #[test]
    fn a_follow_up_is_answered_under_what_it_was_about() {
        let project = Project(vec![("/p/main.rs".into(), MAIN.into()), ("/p/lib.rs".into(), "\n\npub fn run() {}\n".into())]);
        let placer = Placer::new("/p".into());
        let mut story = Story::new("add a flag");
        arrived(&mut story, "<step>\n<file>\nlib.rs\n</file>\n<old>\npub fn run() {}\n</old>\n<new>\npub fn run(v: bool) {}\n</new>\n</step>\n");
        let step = story.current().unwrap();
        story.follow_up("why?".into());
        let waiting = layout(&story, &project, &placer).waiting_spot();
        assert_eq!(waiting, Some(Place { path: Some("/p/lib.rs".into()), near: 2..3 }), "waiting under the step");
        arrived(&mut story, "<answer>\nSo callers choose.\n</answer>\n");
        let answer = story.current().unwrap();
        assert_eq!(answer.part, Part::Answer);
        let layout = layout(&story, &project, &placer);
        assert_eq!(layout.spot(answer), Some(Place { path: Some("/p/lib.rs".into()), near: 2..3 }));
        assert_eq!(layout.lines(step), Some(Place { path: Some("/p/lib.rs".into()), near: 2..3 }));
    }
}
