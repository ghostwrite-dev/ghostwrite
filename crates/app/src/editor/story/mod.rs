//! `space a`: ask Claude about the lines at the cursor (or the selection),
//! then review its reply one item at a time where it applies, across the
//! project's files, and the terminal. What the story is and what it decides
//! (the review keys, where its items are, carrying out what's accepted, what
//! Claude is asked) is [`ghostwrite_view::story`], with no gpui. Here is
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
use super::{Editor, PromptKind};
use request::Requests;

/// The story that's up: its state, and the requests it has on their way.
pub(super) struct Live {
    pub(super) state: StoryState,
    requests: Requests,
}

impl Live {
    fn new(state: StoryState) -> Self {
        Self { state, requests: Requests::default() }
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
        if self.tab().is_some_and(|t| t.doc.read_only()) {
            self.project.say("the model doesn't comment on this: ask in the file it's made from");
            return;
        }
        if self.backend.is_none() {
            self.project.say("to ask, give it a Claude API key (`:api-key ‹key›`), install Claude Code (`claude`), or choose a local model (`:model`)");
            return;
        }
        // Nothing open: ask from an empty tab, which stays while the story's up.
        if self.tab().is_none() {
            self.new_buffer(cx);
        }
        if let Some(backend) = &self.backend {
            backend.warm(Some(&self.project.root), None);
        }
        // One story at a time: whatever was up ends first.
        if self.story.is_some() {
            self.finish_story(cx);
        }
        // The story is reviewed in AI mode, and afterwards it's back to where you were.
        self.leave_insert();
        self.modal.enter(Mode::Ai);
        let from_select = self.modal.entered_from() == Some(Mode::Select);
        let Some(state) = StoryState::about_selection(&mut self.project, from_select) else { return };
        self.begin_story(state);
        self.open_prompt(PromptKind::Ask, typed, window, cx);
    }

    /// A story of `items` the editor gives (replacing any story under
    /// review), from the file in view: each is shown in turn, and asking about
    /// one hands over to the model.
    pub(super) fn open_given(&mut self, instruction: String, items: Vec<Item>, cx: &mut Context<Self>) {
        if self.reviewing_claude() {
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
    /// answer (`space k` docs); Enter asks Claude about it.
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
        let Some(command) = self.story.as_ref().map(|live| live.command_to_run()) else { return };
        if let Some(run) = &command
            && let Err(e) = self.run_in_terminal(run, cx)
        {
            return self.project.say(e);
        }
        let Some(live) = self.story.as_mut() else { return };
        if !live.state.accept(&mut self.project) {
            return;
        }
        // A command is set going, and the story goes on without waiting for it.
        self.settle_story(cx);
        // It was the last thing to do: the keys stay on the terminal, in normal mode, to
        // read what it prints.
        if command.is_some() && self.story.is_none() {
            self.modal.enter(Mode::Normal);
            self.start_reading(false, cx);
        }
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
        // A command to run: the terminal it runs in shows (unless the user's looking elsewhere).
        if show != Show::Ready && self.story.as_ref().is_some_and(|live| live.command_to_run().is_some()) {
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
        if self.reviewing_claude() {
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

    /// Whether a story with Claude in it is up (being asked, or under review).
    fn reviewing_claude(&self) -> bool {
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
        if let Some(Live { state, requests }) = self.story.take() {
            requests.stop();
            if let Some(undo) = state.close(&mut self.project) {
                self.last_story = undo;
            }
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
        cx.notify();
    }
}
