//! `space a`: ask the model about the lines at the cursor (or the selection),
//! then review its reply one item at a time where it applies, across the
//! project's files, and the terminal. What the story is and what it decides
//! (the review keys, where its items are, carrying out what's accepted, what
//! the model is asked) is [`ghostwrite_view::story`], with no gpui. Here is
//! what the app does for it:
//!
//! - here: starting a story from the keys and commands, doing what a review
//!   key comes to ([`Do`]), bringing what's next into view ([`Show`]): the
//!   tab, the tree, the terminal, where the light goes; and ending it;
//! - [`request`]: running the requests (the reply, and the next ones asked
//!   ahead) on the backend in the background, and handing on what they
//!   report.
//!
//! [`story_view`](super::story_view) draws it.

mod request;

use std::ops::{Deref, DerefMut};

use ghostwrite_ai::Backend;
use ghostwrite_ai::protocol::Note;
use ghostwrite_ai::{Item, Story};
use ghostwrite_core::{Mode, Review};
use ghostwrite_view::story::{Do, Show, StoryState};
use gpui::{Context, Window};

use super::attention::Panel;
use super::{DocId, Editor, PromptKind};
use request::Requests;

/// The story that's up: its state, and the requests it has on their way.
pub(super) struct Live {
    pub(super) state: StoryState,
    requests: Requests,
    /// The last item accepted was a command: if the story ends there, the keys stay on
    /// the terminal, to read what it prints.
    ran: bool,
    /// Asked about the terminal: the empty tab it was asked from, which goes with it if
    /// it's still empty, and the tab that was in front before.
    scratch: Option<(DocId, Option<DocId>)>,
}

impl Live {
    fn new(state: StoryState) -> Self {
        Self { state, requests: Requests::default(), ran: false, scratch: None }
    }
}

impl Deref for Live {
    type Target = StoryState;

    fn deref(&self) -> &StoryState {
        &self.state
    }
}

impl DerefMut for Live {
    fn deref_mut(&mut self) -> &mut StoryState {
        &mut self.state
    }
}

impl Editor {
    /// Open the prompt about the cursor's lines (or the selection's), with
    /// text typed in.
    pub(super) fn open_story(&mut self, typed: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.open_story_about(None, typed, window, cx);
    }

    /// Open the prompt about the cursor's lines, or about `quoted`, what was selected on
    /// the terminal: then the code isn't looked at, whatever's in front (pages, say). It's
    /// asked from an empty tab, which goes with the story.
    pub(super) fn open_story_about(&mut self, quoted: Option<String>, typed: &str, window: &mut Window, cx: &mut Context<Self>) {
        if quoted.is_none() && self.tab().is_some_and(|t| t.doc.read_only()) {
            self.project.say("the model doesn't comment on this: ask in the file it's made from");
            return;
        }
        if self.backend.is_none() {
            self.project.say("to ask, give it a Claude API key (`:api-key ‹key›`), install Claude Code (`claude`), or choose a local model (`:model`)");
            return;
        }
        if let Some(backend) = &self.backend {
            backend.warm(Some(&self.project.root), None);
        }
        // One story at a time: whatever was up ends first.
        if self.story.is_some() {
            self.finish_story(cx);
        }
        let was = self.tab().map(|tab| tab.doc.id);
        // Nothing open, or about the terminal: ask from an empty tab, which stays while the story's up.
        if self.tab().is_none() || quoted.is_some() && !self.tab().is_some_and(|tab| tab.doc.is_pristine()) {
            self.new_buffer(cx);
        }
        // The story is reviewed in AI mode, and afterwards it's back to where you were.
        self.leave_insert();
        self.modal.enter(Mode::Ai);
        let from_select = self.modal.entered_from() == Some(Mode::Select);
        let state = match quoted.clone() {
            Some(quoted) => StoryState::about_terminal(&self.project, quoted),
            None => StoryState::about_selection(&mut self.project, from_select),
        };
        let Some(state) = state else { return };
        let doc = state.doc;
        self.begin_story(state);
        if let Some(live) = self.story.as_mut().filter(|_| quoted.is_some()) {
            live.scratch = Some((doc, was.filter(|was| *was != doc)));
        }
        self.open_prompt(PromptKind::Ask, typed, window, cx);
    }

    /// A story of `items` the editor gives (replacing any story under
    /// review), from the file in view: each is shown in turn, and asking about
    /// one hands over to the model.
    pub(super) fn open_given(&mut self, instruction: String, items: Vec<Item>, cx: &mut Context<Self>) {
        if self.story_involves_model() {
            self.project.say("the story is still under review: esc esc ends it first");
            return;
        }
        let Some(story) = Story::given(instruction, items) else { return };
        // From a file: the first item's, if none is open.
        if self.tab().is_none()
            && let Some(Item::Note(Note { file: Some(file), .. })) = story.current_item()
            && let Some(ix) = self.project.open(&self.project.root.join(&file))
        {
            self.activate(ix, cx);
        }
        let Some(state) = StoryState::given(&self.project, story) else { return };
        if self.story.is_some() {
            self.close_story(cx);
        }
        self.leave_insert();
        self.modal.enter(Mode::Ai);
        self.begin_story(state);
        self.settle_story(cx);
    }

    /// Show `text` as a note under the cursor's line, reviewed like an
    /// answer (`space k` docs); Enter asks the model about it.
    pub(super) fn show_note(&mut self, text: &str, cx: &mut Context<Self>) {
        // Docs that arrive after a story began (`space k` then `space a`) don't replace it.
        if self.story.is_some() {
            return;
        }
        self.leave_insert();
        let Some(state) = StoryState::docs(&self.project, text) else { return };
        self.modal.enter(Mode::Ai);
        self.begin_story(state);
        self.settle_story(cx);
    }

    /// The story's up, and the tab it was asked from stays: its items are found from it.
    fn begin_story(&mut self, state: StoryState) {
        self.project.tabs.hold(Some(state.doc));
        self.story = Some(Live::new(state));
        // The model reads the shell in front, until this story runs something.
        self.terminal.ran_in = None;
    }

    /// Enter in the prompt: the first ask, or (while reviewing) something
    /// said about the current item.
    pub(super) fn ask(&mut self, said: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(live) = self.story.as_mut() else { return };
        let following = live.asked();
        if !live.state.ask(said) {
            return;
        }
        if following {
            live.requests.stop_reply();
        }
        self.close_prompt(window, cx);
        self.send_request(cx);
    }

    /// Esc in the prompt: before asking, close; while reviewing, back to the review.
    pub(super) fn cancel_ask(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let asked = self.story.as_ref().is_some_and(|live| live.asked());
        self.close_prompt(window, cx);
        if !asked {
            self.close_story(cx);
        }
    }

    /// A review key for the story, which only goes forward: what it comes to is the story's to say.
    pub(super) fn story_review(&mut self, key: Review, window: &mut Window, cx: &mut Context<Self>) {
        let todo = match self.story.as_mut() {
            Some(live) => live.state.review(key, &mut self.project),
            // No story (AI mode without one): Esc still leaves it.
            None if key == Review::Stop => Do::Finish,
            None => Do::Nothing,
        };
        match todo {
            Do::Nothing => {}
            Do::AnswerRead(yes) => {
                if let Some(answer) = self.story.as_mut().and_then(|live| live.requests.read_answer.take()) {
                    answer.send(yes).ok();
                }
                cx.notify();
            }
            Do::BackToTheStep => self.back_to_the_step(cx),
            Do::Accept => self.accept(cx),
            Do::Ask => self.open_prompt(PromptKind::Ask, "", window, cx),
            Do::Finish => self.finish_story(cx),
        }
    }

    /// The user has looked elsewhere, while a story's up: what it shows next waits for them.
    pub(super) fn wander(&mut self) {
        if let Some(live) = self.story.as_mut() {
            live.wander();
        }
    }

    /// Tab, having looked elsewhere: back to the step (or where it'll come), and the light
    /// goes there from where the cursor was.
    fn back_to_the_step(&mut self, cx: &mut Context<Self>) {
        if let Some(rect) = self.measured.cursor {
            self.came_back_from(rect);
        }
        self.settle_story(cx);
    }

    fn accept(&mut self, cx: &mut Context<Self>) {
        if self.story.as_ref().is_some_and(|live| live.lost_its_tab(&self.project)) {
            return self.finish_story(cx);
        }
        // Waiting on the story's command, with nothing up: Tab goes on without it.
        let nothing_up = self.story.as_ref().is_some_and(|live| live.story.as_ref().is_none_or(|story| story.current().is_none()));
        if nothing_up && self.go_on_without_the_command(cx) {
            return;
        }
        let Some(command) = self.story.as_ref().map(|live| live.command_to_run()) else { return };
        let ran = match &command {
            Some(run) => match self.run_in_terminal(run, cx) {
                Ok(ran) => Some(ran),
                Err(e) => return self.project.say(e),
            },
            None => None,
        };
        let Some(live) = self.story.as_mut() else { return };
        let accepted = match &ran {
            Some(ran) => live.state.accept_command(&mut self.project, ran),
            None => live.state.accept(&mut self.project),
        };
        if !accepted {
            return;
        }
        live.ran = ran.is_some();
        // A command is set going: what's next is asked for once it's done.
        self.settle_story(cx);
    }

    /// Tab in the terminal, at the story's command: it runs, and the story goes on
    /// from where the next step is.
    pub(super) fn run_from_the_terminal(&mut self, cx: &mut Context<Self>) {
        self.modal.leave(ghostwrite_core::Mode::Terminal);
        self.accept(cx);
    }

    /// Accept each item the editor gave in turn, until the story's over.
    pub(super) fn accept_all_given(&mut self, cx: &mut Context<Self>) {
        while self.story.as_ref().is_some_and(|live| live.given_to_accept()) {
            let before = self.story.as_ref().and_then(|live| live.story.as_ref()?.current());
            self.accept(cx);
            // One that can't be applied stays up, and says why.
            if self.story.as_ref().and_then(|live| live.story.as_ref()?.current()) == before {
                break;
            }
        }
    }

    /// Bring the story's current item into view (or finish).
    pub(super) fn settle_story(&mut self, cx: &mut Context<Self>) {
        if self.story.as_ref().is_some_and(|live| live.lost_its_tab(&self.project)) {
            return self.finish_story(cx);
        }
        // By the document: a tab closing can put another in the same place.
        let was_at = self.tab().map(|tab| tab.doc.id);
        let model = self.backend.as_ref().map(|b| b.id()).unwrap_or_default();
        let Some(live) = self.story.as_mut() else { return };
        let show = live.state.settle(&mut self.project, &model);
        // A command to run, just come up: the terminal it runs in shows (unless the user's
        // looking elsewhere). Only then: more of the reply coming in leaves the terminal be.
        if matches!(show, Show::Tab(_)) && self.story.as_ref().is_some_and(|live| live.command_to_run().is_some()) {
            self.show_terminal_for_command(cx);
        }
        let in_terminal = self.command_proposed();
        match show {
            Show::Nothing | Show::Ready => {}
            Show::Revise => self.send_request(cx),
            Show::Tab(ix) => {
                if ix != self.project.tabs.active_index() {
                    self.activate(ix, cx);
                }
                // The story put them there: that's not looking elsewhere.
                if let Some(live) = self.story.as_mut() {
                    live.wandered = false;
                }
                let another_file = self.tab().map(|tab| tab.doc.id) != was_at;
                // A command: the keys read the terminal it's in, in AI mode, to look through it.
                if in_terminal {
                    self.start_reading(true, cx);
                    self.look_at(Panel::Terminal, false, cx);
                } else {
                    self.stop_reading();
                    self.look_at(Panel::Code, another_file, cx);
                }
            }
            Show::Tree => {
                self.tree_pane.shown = true;
                self.look_at(Panel::Tree, false, cx);
            }
            Show::Finished => self.finish_story(cx),
        }
        self.prefetch(cx);
        cx.notify();
    }

    /// `:undo-story`: take back everything the last story's accepted steps did,
    /// in every file, if nothing's been edited in those files since.
    pub(super) fn undo_story(&mut self, cx: &mut Context<Self>) -> Result<(), String> {
        if self.story.is_some() {
            return Err("the story is still under review: esc esc ends it first".into());
        }
        ghostwrite_view::story::undo(&mut self.project, &mut self.last_story)?;
        cx.notify();
        Ok(())
    }

    /// `:clear`: start over. The story up ends (what it applied stays), and what
    /// the model kept of the stories before (its notes, the recent requests) is
    /// forgotten: the next one is told nothing of them.
    pub(super) fn clear_context(&mut self, cx: &mut Context<Self>) {
        if self.story.is_some() {
            self.finish_story(cx);
        }
        ghostwrite_view::story::forget(&mut self.project);
        cx.notify();
    }

    /// Ask `chosen` from now on (`:model`), and give back what the last one held.
    pub(in crate::editor) fn choose_model(&mut self, chosen: Backend) -> Result<(), String> {
        if self.story_involves_model() {
            return Err("the story is still under review: esc esc ends it first".into());
        }
        if let Some(previous) = self.backend.as_ref().filter(|previous| previous.id() != chosen.id()) {
            previous.release();
        }
        self.project.status = Some(format!("asking {} from now on", chosen.name()).into());
        ghostwrite_ai::models::remember(&chosen);
        chosen.warm(Some(&self.project.root), None);
        self.backend = Some(std::sync::Arc::new(chosen));
        Ok(())
    }

    /// Whether a story with the model in it is up (being asked, or under review).
    fn story_involves_model(&self) -> bool {
        self.story.as_ref().is_some_and(|live| live.involves_model())
    }

    /// Close the review, saying what came of it, and remember it.
    pub(super) fn finish_story(&mut self, cx: &mut Context<Self>) {
        if let Some(live) = self.story.as_ref() {
            live.finish(&mut self.project);
        }
        self.close_story(cx);
    }

    fn close_story(&mut self, cx: &mut Context<Self>) {
        self.attention.on = Panel::Code;
        self.story_left_the_terminal();
        self.forget_where_the_story_was();
        // What it set going isn't waited for any more.
        self.terminal.running = None;
        let mut ran = false;
        if let Some(Live { state, requests, ran: ran_last, scratch }) = self.story.take() {
            requests.stop();
            ran = ran_last;
            if let Some(undo) = state.close(&mut self.project) {
                self.last_story = undo;
            }
            self.close_scratch(scratch, cx);
        } else {
            self.project.tabs.hold(None);
            self.project.tree.propose(None);
        }
        self.modal.leave(Mode::Ai);
        if self.asking() {
            self.prompt = None;
            // The ask field had the keys: the code gets them back.
            self.refocus = true;
        }
        // It ended with a command: the keys stay on the terminal, in normal mode, to read what it prints.
        if ran && self.terminal.shown && !self.terminal.shells.is_empty() {
            self.back_to_the_command();
            self.modal.enter(Mode::Normal);
            self.start_reading(false, cx);
        }
        cx.notify();
    }

    /// A story about the terminal is over: the empty tab it was asked from goes, if it's
    /// still empty, and the tab that was in front comes back.
    fn close_scratch(&mut self, scratch: Option<(DocId, Option<DocId>)>, cx: &mut Context<Self>) {
        let Some((doc, was)) = scratch else { return };
        let Some(ix) = self.project.tabs.index_of_id(doc).filter(|ix| self.project.tabs.get(*ix).is_some_and(|tab| tab.doc.is_pristine())) else {
            return;
        };
        let front = self.project.tabs.active_index() == ix;
        self.project.close(ix);
        if let Some(ix) = was.and_then(|was| self.project.tabs.index_of_id(was)).filter(|_| front) {
            self.activate(ix, cx);
        }
        self.tab_changed(cx);
    }
}
