//! `space a`: ask Claude about the lines at the cursor (or the selection),
//! then review its reply one item at a time where it applies, across the
//! project's files, and the terminal: a command Claude proposes runs there
//! when it's accepted, and the story goes on without waiting for it. Tab
//! accepts (or reads on); there's no skipping: Enter
//! says what you want instead, and the next item follows it. Esc stops. A
//! file to create is shown in the file tree. An item that can't be found or
//! applied goes back to the model to rewrite before it's shown, and so does
//! a reply that breaks the format.
//!
//! [`StoryState`] is the feature: a [`Story`] (the bookkeeping) laid out in
//! the [`Project`], and the conversation with Claude it's from. It decides,
//! and the app carries it out: the review keys, what the model says
//! ([`Event`]s) and the user looking elsewhere come in, and what the app is
//! to do goes out ([`Do`], [`Show`], [`AskAhead`]). Its parts:
//!
//! - here: starting a story, the review keys, and ending it (logged for
//!   the next ones, Claude's rewritten notes kept), `:undo-story` and `:clear`;
//! - [`review`]: where its items are, carrying out what's accepted, and
//!   what to show next;
//! - [`request`]: what Claude is asked and told, and what it says back;
//! - [`stops`]: the items of the stories the editor gives.
//!
//! The app (`editor/story`) runs the requests on the backend in the
//! background, opens the prompt, runs commands in the terminal, and moves
//! the focus; `story_view` there draws it.

pub mod request;
pub mod review;
pub mod stops;
#[cfg(test)]
mod tests;

use std::cell::RefCell;
use std::ops::Range as Lines;
use std::path::PathBuf;

use ghostwrite_ai::placement::Placer;
use ghostwrite_ai::timing::{Timer, Wait};
use ghostwrite_ai::{ItemId, Story};
use ghostwrite_core::{Range, Review};

use crate::{Autoscroll, DocId, Project};
pub use request::{AskAhead, Event};
use request::{Conversation, Used};
pub use review::{Located, Show, Waiting};

pub struct StoryState {
    /// `None` until the first request is sent.
    pub story: Option<Story>,
    /// The document asked about, and the lines pointed at (zero-based, end exclusive).
    pub doc: DocId,
    pub lines: Lines<usize>,
    /// Docs from `space k`: shown without moving the cursor, as helix does.
    docs_note: bool,
    /// The item last brought into view.
    shown: Option<ItemId>,
    /// The user has looked elsewhere since the story last put them somewhere: a step
    /// that comes now waits for them, and Tab brings them to it before it accepts.
    pub wandered: bool,
    /// The story moved the user since this was last taken: that's not looking elsewhere.
    pub moved_them: bool,
    /// The tabs the story opened to show an item, closed as it moves on from them.
    opened: Vec<DocId>,
    /// Asked from select mode: the selection to give back afterwards.
    selection: Option<Range>,
    /// Asked from the terminal: what was selected on it, which the question is about.
    pub quoted: Option<String>,
    placer: Placer,
    conversation: Conversation,
    /// What it has used of the model so far.
    used: Used,
    /// The documents its accepted steps changed, one entry a step, in order.
    applied: RefCell<Vec<DocId>>,
    /// The wait for the model being timed, until there's something to look at.
    pub timer: Option<Timer>,
    /// What the user last said (asked, or said during the review): shown while waiting.
    pub said: Option<String>,
    /// The model would read this, outside the project: Tab lets it, Enter doesn't.
    /// (The app keeps where the answer goes.)
    pub asking_to_read: Option<PathBuf>,
}

/// What the app is to do about a review key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Do {
    /// Nothing more (what there was to say is said).
    Nothing,
    /// Tell the model whether it may read what it asked to: yes or no.
    AnswerRead(bool),
    /// Bring the user back to the step they looked away from: the light goes
    /// there from where they were, and the story [settles](StoryState::settle).
    BackToTheStep,
    /// Accept the current item: its command runs in the terminal first, if
    /// it's one, then [`StoryState::accept`].
    Accept,
    /// Open the prompt, to say what you want instead.
    Ask,
    /// End the story ([`StoryState::finish`]).
    Finish,
}

/// What it takes to undo a story: each document it changed, the steps it
/// took there, and the version it left it at.
pub type Undo = Vec<(DocId, usize, u64)>;

impl StoryState {
    pub fn new(doc: DocId, lines: Lines<usize>, root: PathBuf) -> Self {
        Self {
            story: None,
            doc,
            lines,
            docs_note: false,
            shown: None,
            wandered: false,
            moved_them: false,
            opened: vec![],
            used: Default::default(),
            applied: Default::default(),
            selection: None,
            quoted: None,
            placer: Placer::new(root),
            conversation: Conversation::default(),
            timer: None,
            said: None,
            asking_to_read: None,
        }
    }

    /// About the cursor's lines (or the selection's) in the tab in front,
    /// which are selected, whole. Asked from select mode (`from_select`), the
    /// selection is given back afterwards.
    pub fn about_selection(project: &mut Project, from_select: bool) -> Option<Self> {
        let root = project.root.clone();
        let tab = project.tab_mut()?;
        let sel = tab.view.selection;
        let kept = from_select.then_some(sel);
        let (from, to) = if sel.is_wide(&tab.doc.buffer) { (sel.from(), tab.doc.buffer.prev(sel.to())) } else { (tab.cursor(), tab.cursor()) };
        let buffer = &tab.doc.buffer;
        let lines = buffer.line_of(from)..buffer.line_of(to) + 1;
        let span = buffer.lines_range(lines.clone());
        let end = span.end - usize::from(span.end > span.start && buffer.char_at(span.end - 1) == Some('\n'));
        tab.view.selection = Range::new(span.start, end.max(span.start));
        let mut state = StoryState::new(tab.doc.id, lines, root);
        state.selection = kept;
        Some(state)
    }

    /// `story`, from the cursor's line in the tab in front.
    fn at_cursor(project: &Project, story: Story) -> Option<Self> {
        let tab = project.tab()?;
        let line = tab.doc.buffer.line_of(tab.cursor());
        let mut state = StoryState::new(tab.doc.id, line..line + 1, project.root.clone());
        state.story = Some(story);
        Some(state)
    }

    /// A story the editor gives, from the file in view: each item is shown in
    /// turn, and asking about one hands over to the model.
    pub fn given(project: &Project, story: Story) -> Option<Self> {
        Self::at_cursor(project, story)
    }

    /// `text` as a note under the cursor's line, reviewed like an answer
    /// (`space k` docs); Enter asks Claude about it.
    pub fn docs(project: &Project, text: &str) -> Option<Self> {
        let mut state = Self::at_cursor(project, Story::answered("Explain what's under the cursor; its documentation was shown.", text))?;
        state.docs_note = true;
        Some(state)
    }

    /// The first request has been sent.
    pub fn asked(&self) -> bool {
        self.story.is_some()
    }

    /// Whether Claude is in it (being asked, or under review).
    pub fn involves_model(&self) -> bool {
        self.story.as_ref().is_none_or(Story::involves_model)
    }

    /// Whether Claude is working and nothing is shown yet, so the dots animate.
    pub fn thinking(&self) -> bool {
        self.story.as_ref().is_some_and(|s| s.error.is_none() && s.current().is_none())
    }

    /// Whether the tab the story was asked from has been closed, which leaves nowhere to find its items.
    pub fn lost_its_tab(&self, project: &Project) -> bool {
        project.tabs.by_id(self.doc).is_none()
    }

    /// Enter in the prompt: the first ask, or (while reviewing) something
    /// said about the current item. Whether there's a request to send.
    pub fn ask(&mut self, said: String) -> bool {
        if said.trim().is_empty() {
            return false;
        }
        self.said = Some(said.trim().to_string());
        match self.story.as_mut() {
            Some(story) => {
                story.follow_up(said);
                self.shown = None;
                // A new wait.
                self.timer = None;
            }
            None => {
                let said = match self.quoted.take() {
                    Some(quoted) => format!("About this, in the terminal:\n```\n{quoted}\n```\n{said}"),
                    None => said,
                };
                self.story = Some(Story::new(said));
            }
        }
        true
    }

    /// A review key for the story, which only goes forward.
    pub fn review(&mut self, key: Review, project: &mut Project) -> Do {
        // Where it takes the user isn't somewhere they went themselves.
        self.moved_them = true;
        // The model would read outside the project: Tab lets it, Enter doesn't.
        if let Some(path) = self.asking_to_read.take() {
            match key {
                Review::Next | Review::Ask => {
                    let yes = key == Review::Next;
                    project.say(format!("{} {}", if yes { "let it read" } else { "didn't let it read" }, ghostwrite_ai::tools::shown(&path)));
                    return Do::AnswerRead(yes);
                }
                // Esc: still asking (Esc again ends the story, and with it the question).
                _ => self.asking_to_read = Some(path),
            }
        }
        match key {
            // Having looked elsewhere: back to the step (or where it'll come). The next Tab accepts it.
            Review::Next if self.wandered => {
                self.wandered = false;
                self.shown = None;
                Do::BackToTheStep
            }
            Review::Next => Do::Accept,
            Review::Ask => Do::Ask,
            Review::Stop => Do::Finish,
            Review::Stopping => {
                project.say("esc again to end this story");
                Do::Nothing
            }
        }
    }

    /// The user has looked elsewhere: what it shows next waits for them.
    pub fn wander(&mut self) {
        self.wandered = true;
    }

    /// Accept the current item, carrying it out (its command, if it's one,
    /// the app has run). Whether it was: if not, it's said why.
    pub fn accept(&mut self, project: &mut Project) -> bool {
        if let Err(e) = self.accept_item(project) {
            project.say(e);
            return false;
        }
        // Timed until the next step shows (if the story ends instead, it's dropped).
        if self.story.as_ref().is_some_and(Story::involves_model) {
            self.timer = Some(Timer::start(Wait::NextStep));
        }
        true
    }

    /// Whether the story is one the editor gave, with an item up to accept.
    pub fn given_to_accept(&self) -> bool {
        self.story.as_ref().is_some_and(|story| !story.involves_model() && story.current().is_some())
    }

    /// The story's closed: the tab it was asked from is let go, a selection
    /// asked from comes back, and the tree proposes nothing. What it takes to
    /// undo it, if it changed anything.
    pub fn close(self, project: &mut Project) -> Option<Undo> {
        project.tabs.hold(None);
        let undo = self.undo(project);
        // Asked from select mode: the selection comes back with it.
        if let Some(sel) = self.selection
            && let Some(tab) = project.tabs.by_id_mut(self.doc)
        {
            tab.select(sel, Autoscroll::Fit);
        }
        project.tree.propose(None);
        undo
    }

    /// The story ends having changed these documents: what it takes to undo it.
    fn undo(&self, project: &Project) -> Option<Undo> {
        let mut docs: Undo = vec![];
        for id in self.applied.borrow().iter() {
            let Some(version) = project.tabs.by_id(*id).map(|tab| tab.doc.buffer.version()) else { continue };
            match docs.iter_mut().find(|(doc, ..)| doc == id) {
                Some((_, steps, _)) => *steps += 1,
                None => docs.push((*id, 1, version)),
            }
        }
        (!docs.is_empty()).then_some(docs)
    }
}

/// `:undo-story`: take back everything the last story's accepted steps did,
/// in every file, if nothing's been edited in those files since.
pub fn undo(project: &mut Project, last: &mut Undo) -> Result<(), String> {
    if last.is_empty() {
        return Err("no story to undo".into());
    }
    let changed = last.iter().find_map(|(id, _, version)| {
        let tab = project.tabs.by_id(*id)?;
        (tab.doc.buffer.version() != *version).then(|| tab.doc.name().to_string())
    });
    if let Some(name) = changed {
        return Err(format!("{name} has been edited since the story: u undoes it step by step"));
    }
    let (mut steps, mut files) = (0, 0);
    for (id, count, _) in std::mem::take(last) {
        let Some(tab) = project.tabs.by_id_mut(id) else { continue };
        for _ in 0..count {
            if let Some(selection) = tab.doc.buffer.undo() {
                tab.view.selection = selection;
                steps += 1;
            }
        }
        tab.view.autoscroll = Some(Autoscroll::Fit);
        tab.sync();
        files += 1;
    }
    project.say(format!("undid {steps} step{} in {files} file{}: not yet saved", if steps == 1 { "" } else { "s" }, if files == 1 { "" } else { "s" }));
    Ok(())
}

/// `:clear`: what the model kept of the stories before (its notes, the
/// recent requests) is forgotten: the next one is told nothing of them.
pub fn forget(project: &mut Project) {
    let logged = ghostwrite_ai::memory::forget(&project.root);
    project.say(format!("starting over: Claude's notes and {logged} earlier request{} are forgotten", if logged == 1 { "" } else { "s" }));
}
