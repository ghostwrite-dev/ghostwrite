//! Where a story's items show. Most go in the code: a step where its lines
//! are, a note at the lines it's pinned to. An answer that isn't pinned to
//! lines shows where it was asked: under the item a follow-up was about, or
//! in a tour's panel when the story began as a question about its output.
//!
//! [`Layout`] works this out from the story, what it was asked about, and
//! the project's files; the editor only turns the result into tabs.

use std::ops::Range;
use std::path::PathBuf;

use crate::placement::{self, Asked, Files, Place, Placer};
use crate::protocol::{self, Note};
use crate::story::{Item, ItemId, Story};

/// What a story was asked about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    /// The lines pointed at.
    Lines,
    /// An error in a command's output, to fix at the lines it points to.
    Fix(String),
    /// A command's output, asked about from its panel.
    Output(String),
}

impl Origin {
    /// The command output that goes along with the request.
    pub fn output(&self) -> Option<&str> {
        match self {
            Origin::Lines => None,
            Origin::Fix(output) | Origin::Output(output) => Some(output),
        }
    }

    /// Asked from a panel: the prompt, and answers not pinned to code, show there.
    pub fn in_panel(&self) -> bool {
        matches!(self, Origin::Output(_))
    }
}

/// Where an item shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Spot {
    /// In a file (`path: None`: the one asked about), near these lines.
    Code(Place),
    /// In the panel the story was asked from.
    Panel,
}

pub struct Layout<'a, F> {
    pub story: &'a Story,
    pub origin: &'a Origin,
    pub asked: Asked<'a>,
    pub files: F,
    pub placer: &'a Placer,
}

impl<F: Files> Layout<'_, F> {
    /// Where item `id` shows.
    pub fn spot(&self, id: ItemId) -> Option<Spot> {
        let item = self.story.item(id)?;
        let unpinned = matches!(&item, Item::Note(Note { file: None, at, .. }) if at.is_empty());
        if unpinned {
            match self.story.asked_about(id) {
                // A follow-up's answer goes where it was asked.
                Some(about) => match self.spot(about)? {
                    Spot::Panel => return Some(Spot::Panel),
                    Spot::Code(_) => {
                        if let Some(lines) = self.lines(about) {
                            return Some(Spot::Code(lines));
                        }
                    }
                },
                None if self.origin.in_panel() => return Some(Spot::Panel),
                None => {}
            }
        }
        Some(Spot::Code(self.placer.place(&item, &self.asked, &self.files)))
    }

    /// The lines item `id` shows at, in its file (`near` is those lines).
    pub fn lines(&self, id: ItemId) -> Option<Place> {
        let Spot::Code(place) = self.spot(id)? else { return None };
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
    /// the marked lines before the first reply. `None` in a panel.
    pub fn waiting_spot(&self) -> Option<Spot> {
        if let Some(about) = self.story.writing_about() {
            return match self.spot(about)? {
                Spot::Panel => Some(Spot::Panel),
                Spot::Code(_) => self.lines(about).map(Spot::Code),
            };
        }
        if self.origin.in_panel() {
            return Some(Spot::Panel);
        }
        Some(Spot::Code(Place { path: None, near: self.asked.lines.clone() }))
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
    use crate::story::{Part, Verdict};

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

    fn layout<'a>(story: &'a Story, origin: &'a Origin, project: &'a Project, placer: &'a Placer) -> Layout<'a, &'a Project> {
        let main = Path::new("/p/main.rs");
        Layout { story, origin, asked: Asked { path: Some(main), text: MAIN, lines: 0..1 }, files: project, placer }
    }

    #[test]
    fn a_follow_up_is_answered_under_what_it_was_about() {
        let project = Project(vec![("/p/main.rs".into(), MAIN.into()), ("/p/lib.rs".into(), "\n\npub fn run() {}\n".into())]);
        let placer = Placer::new("/p".into());
        let mut story = Story::new("add a flag");
        arrived(&mut story, "<step>\n<file>\nlib.rs\n</file>\n<old>\npub fn run() {}\n</old>\n<new>\npub fn run(v: bool) {}\n</new>\n</step>\n");
        let step = story.current().unwrap();
        story.follow_up("why?".into());
        let waiting = layout(&story, &Origin::Lines, &project, &placer).waiting_spot();
        assert_eq!(waiting, Some(Spot::Code(Place { path: Some("/p/lib.rs".into()), near: 2..3 })), "waiting under the step");
        arrived(&mut story, "<answer>\nSo callers choose.\n</answer>\n");
        let answer = story.current().unwrap();
        assert_eq!(answer.part, Part::Answer);
        let layout = layout(&story, &Origin::Lines, &project, &placer);
        assert_eq!(layout.spot(answer), Some(Spot::Code(Place { path: Some("/p/lib.rs".into()), near: 2..3 })));
        assert_eq!(layout.lines(step), Some(Place { path: Some("/p/lib.rs".into()), near: 2..3 }));
    }

    #[test]
    fn answers_about_output_show_in_the_panel() {
        let project = Project(vec![("/p/main.rs".into(), MAIN.into())]);
        let placer = Placer::new("/p".into());
        let origin = Origin::Output("$ ls\na\nb".into());
        let mut story = Story::new("what is this?");
        assert_eq!(layout(&story, &origin, &project, &placer).waiting_spot(), Some(Spot::Panel));
        arrived(&mut story, "<answer>\nTwo files.\n</answer>\n");
        let note = story.current().unwrap();
        assert_eq!(layout(&story, &origin, &project, &placer).spot(note), Some(Spot::Panel));
        // A question about that answer is answered there too.
        story.follow_up("why two?".into());
        arrived(&mut story, "<answer>\nBecause.\n</answer>\n");
        let again = story.current().unwrap();
        assert_eq!(layout(&story, &origin, &project, &placer).spot(again), Some(Spot::Panel));
        // But a note pinned to code goes to the code.
        story.decide(Verdict::Accepted);
        story.follow_up("where's main?".into());
        arrived(&mut story, "<answer>\n<note>\n<at>\nfn main() { run(); }\n</at>\nHere.\n</note>\n</answer>\n");
        let pinned = story.current().unwrap();
        assert_eq!(layout(&story, &origin, &project, &placer).spot(pinned), Some(Spot::Code(Place { path: None, near: 0..1 })));
    }
}
