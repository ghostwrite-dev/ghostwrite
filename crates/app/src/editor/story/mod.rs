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
//! the [`Project`], and the conversation with Claude it's from. Its parts:
//!
//! - here: starting a story, the review keys, and ending it (logged for
//!   the next ones, Claude's rewritten notes kept);
//! - [`review`]: where its items are, carrying out what's accepted, and
//!   what to show next;
//! - [`request`]: what Claude is asked and told, and running the requests
//!   (the reply, and the next ones asked ahead) in the background.
//!
//! [`story_view`](super::story_view) draws it.

mod request;
mod review;

use ghostwrite_ai::Backend;
use std::ops::Range as Lines;
use std::path::PathBuf;

use ghostwrite_ai::placement::Placer;
use ghostwrite_ai::protocol::Note;
use ghostwrite_ai::timing::{Timer, Wait};
use ghostwrite_ai::{Item, ItemId, Story};
use ghostwrite_core::{Mode, Range, Review};
use gpui::{Context, Window};

use super::attention::Panel;
use super::{Autoscroll, DocId, Editor, PromptKind};
use request::Conversation;
use review::Show;
pub(super) use review::{Located, Waiting};

pub(super) struct StoryState {
    /// `None` until the first request is sent.
    pub(super) story: Option<Story>,
    /// The document asked about, and the lines pointed at (zero-based, end exclusive).
    pub(super) doc: DocId,
    pub(super) lines: Lines<usize>,
    /// Docs from `space k`: shown without moving the cursor, as helix does.
    docs_note: bool,
    /// The item last brought into view.
    shown: Option<ItemId>,
    /// The user has looked elsewhere since the story last put them somewhere: a step
    /// that comes now waits for them, and Tab brings them to it before it accepts.
    pub(super) wandered: bool,
    /// The story moved the user since this was last taken: that's not looking elsewhere.
    pub(super) moved_them: bool,
    /// The tabs the story opened to show an item, closed as it moves on from them.
    opened: Vec<DocId>,
    /// Asked from select mode: the selection to give back afterwards.
    selection: Option<Range>,
    /// Asked from the terminal: what was selected on it, which the question is about.
    pub(super) quoted: Option<String>,
    placer: Placer,
    conversation: Conversation,
    /// What it has used of the model so far.
    used: request::Used,
    /// The documents its accepted steps changed, one entry a step, in order.
    applied: std::cell::RefCell<Vec<DocId>>,
    /// The wait for the model being timed, until there's something to look at.
    pub(super) timer: Option<Timer>,
    /// What the user last said (asked, or said during the review): shown while waiting.
    pub(super) said: Option<String>,
    /// What the user has let the model read outside the project.
    readable: request::Readable,
    /// The model would read this, outside the project: Tab lets it, Enter doesn't.
    pub(super) asking_to_read: Option<(PathBuf, std::sync::mpsc::Sender<bool>)>,
}

impl StoryState {
    fn new(doc: DocId, lines: Lines<usize>, root: PathBuf) -> Self {
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
            readable: Default::default(),
            asking_to_read: None,
        }
    }

    /// The first request has been sent.
    pub(super) fn asked(&self) -> bool {
        self.story.is_some()
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
        let root = self.project.root.clone();
        let Some(tab) = self.project.tab_mut() else { return };
        let sel = tab.view.selection;
        let kept = from_select.then_some(sel);
        let (from, to) = if sel.is_wide(tab.doc.text()) { (sel.from(), tab.doc.buffer.prev(sel.to())) } else { (tab.cursor(), tab.cursor()) };
        let buffer = &tab.doc.buffer;
        let lines = buffer.line_of(from)..buffer.line_of(to) + 1;
        let span = buffer.lines_range(lines.clone());
        let end = span.end - usize::from(buffer.text()[span.clone()].ends_with('\n'));
        tab.view.selection = Range::new(span.start, end.max(span.start));
        let mut state = StoryState::new(tab.doc.id, lines, root);
        state.selection = kept;
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
        let Some(tab) = self.tab() else { return };
        let line = tab.doc.buffer.line_of(tab.cursor());
        let mut state = StoryState::new(tab.doc.id, line..line + 1, self.project.root.clone());
        state.story = Some(story);
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
        let Some(tab) = self.tab() else { return };
        let line = tab.doc.buffer.line_of(tab.cursor());
        let mut state = StoryState::new(tab.doc.id, line..line + 1, self.project.root.clone());
        self.modal.enter(Mode::Ai);
        state.docs_note = true;
        state.story = Some(Story::answered("Explain what's under the cursor; its documentation was shown.", text));
        self.begin_story(state);
        self.settle_story(cx);
    }

    /// The story's up, and the tab it was asked from stays: its items are found from it.
    fn begin_story(&mut self, state: StoryState) {
        self.project.tabs.hold(Some(state.doc));
        self.story = Some(state);
    }

    /// Whether the tab the story was asked from has been closed, which leaves nowhere to find its items.
    fn story_lost_its_tab(&self) -> bool {
        self.story.as_ref().is_some_and(|state| self.project.tabs.by_id(state.doc).is_none())
    }

    /// Enter in the prompt: the first ask, or (while reviewing) something
    /// said about the current item.
    pub(super) fn ask(&mut self, said: String, window: &mut Window, cx: &mut Context<Self>) {
        if said.trim().is_empty() {
            return;
        }
        let Some(state) = self.story.as_mut() else { return };
        state.said = Some(said.trim().to_string());
        match state.story.as_mut() {
            Some(story) => {
                state.conversation.stop_reply();
                story.follow_up(said);
                state.shown = None;
                // A new wait.
                state.timer = None;
            }
            None => {
                let said = match state.quoted.take() {
                    Some(quoted) => format!("About this, in the terminal:\n```\n{quoted}\n```\n{said}"),
                    None => said,
                };
                state.story = Some(Story::new(said));
            }
        }
        self.close_prompt(window, cx);
        self.send_request(cx);
    }

    /// Esc in the prompt: before asking, close; while reviewing, back to the review.
    pub(super) fn cancel_ask(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let asked = self.story.as_ref().is_some_and(StoryState::asked);
        self.close_prompt(window, cx);
        if !asked {
            self.close_story(cx);
        }
    }

    /// A review key for the story, which only goes forward.
    pub(super) fn story_review(&mut self, key: Review, window: &mut Window, cx: &mut Context<Self>) {
        // Where it takes the user isn't somewhere they went themselves.
        if let Some(state) = self.story.as_mut() {
            state.moved_them = true;
        }
        // The model would read outside the project: Tab lets it, Enter doesn't.
        if let Some((path, answer)) = self.story.as_mut().and_then(|state| state.asking_to_read.take()) {
            match key {
                Review::Next | Review::Ask => {
                    let yes = key == Review::Next;
                    answer.send(yes).ok();
                    self.project.say(format!("{} {}", if yes { "let it read" } else { "didn't let it read" }, ghostwrite_ai::tools::shown(&path)));
                    return cx.notify();
                }
                // Esc: still asking (Esc again ends the story, and with it the question).
                _ => {
                    if let Some(state) = self.story.as_mut() {
                        state.asking_to_read = Some((path, answer));
                    }
                }
            }
        }
        match key {
            Review::Next if self.story.as_ref().is_some_and(|state| state.wandered) => self.back_to_the_step(cx),
            Review::Next => self.accept(cx),
            Review::Ask => self.open_prompt(PromptKind::Ask, "", window, cx),
            Review::Stop => self.finish_story(cx),
            Review::Stopping => self.project.say("esc again to end this story"),
        }
    }

    /// The user has looked elsewhere, while a story's up: what it shows next waits for them.
    pub(super) fn wander(&mut self) {
        if let Some(state) = self.story.as_mut() {
            state.wandered = true;
        }
    }

    /// Tab, having looked elsewhere: back to the step (or where it'll come), and the light
    /// goes there from where the cursor was. The next Tab accepts it.
    fn back_to_the_step(&mut self, cx: &mut Context<Self>) {
        let cursor = self.measured.cursor;
        let Some(state) = self.story.as_mut() else { return };
        state.wandered = false;
        state.shown = None;
        if let Some(rect) = cursor {
            self.came_back_from(rect);
        }
        self.settle_story(cx);
    }

    fn accept(&mut self, cx: &mut Context<Self>) {
        if self.story_lost_its_tab() {
            return self.finish_story(cx);
        }
        let Some(command) = self.story.as_ref().map(StoryState::command_to_run) else { return };
        if let Some(run) = &command
            && let Err(e) = self.run_in_terminal(run, cx)
        {
            return self.project.say(e);
        }
        let Some(state) = self.story.as_mut() else { return };
        if let Err(e) = state.accept(&mut self.project) {
            self.project.say(e);
            return;
        }
        // Timed until the next step shows (if the story ends instead, it's dropped).
        if state.story.as_ref().is_some_and(Story::involves_model) {
            state.timer = Some(Timer::start(Wait::NextStep));
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
        while self.story.as_ref().is_some_and(|state| state.story.as_ref().is_some_and(|story| !story.involves_model() && story.current().is_some())) {
            let before = self.story.as_ref().and_then(|state| state.story.as_ref()?.current());
            self.accept(cx);
            // One that can't be applied stays up, and says why.
            if self.story.as_ref().and_then(|state| state.story.as_ref()?.current()) == before {
                break;
            }
        }
    }

    /// Bring the story's current item into view (or finish).
    pub(super) fn settle_story(&mut self, cx: &mut Context<Self>) {
        if self.story_lost_its_tab() {
            return self.finish_story(cx);
        }
        // By the document: a tab closing can put another in the same place.
        let was_at = self.tab().map(|tab| tab.doc.id);
        let Some(state) = self.story.as_mut() else { return };
        let show = state.settle(&mut self.project);
        // A command to run: the terminal it runs in shows (unless the user's looking elsewhere).
        if show != Show::Ready && self.story.as_ref().and_then(|s| s.story.as_ref()?.current_item()).is_some_and(|item| matches!(item, Item::Run(_))) {
            self.show_terminal_for_command(cx);
        }
        let in_terminal = self.command_proposed();
        // Something to look at: the wait for it is over.
        if matches!(show, Show::Tab(_) | Show::Tree | Show::Finished)
            && let Some(timer) = self.story.as_mut().and_then(|state| state.timer.take())
            && !matches!(show, Show::Finished)
        {
            timer.shown(&self.backend.as_ref().map(|b| b.id()).unwrap_or_default());
        }
        match show {
            Show::Nothing | Show::Ready => {}
            Show::Revise => self.send_request(cx),
            Show::Tab(ix) => {
                if ix != self.project.tabs.active_index() {
                    self.activate(ix, cx);
                }
                // The story put them there: that's not looking elsewhere.
                if let Some(state) = self.story.as_mut() {
                    state.wandered = false;
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

    /// A story ended having changed these documents: what it takes to undo it.
    fn remember_story(&mut self, applied: &[DocId]) {
        let mut docs: Vec<(DocId, usize, u64)> = vec![];
        for id in applied {
            let Some(version) = self.project.tabs.by_id(*id).map(|tab| tab.doc.buffer.version()) else { continue };
            match docs.iter_mut().find(|(doc, ..)| doc == id) {
                Some((_, steps, _)) => *steps += 1,
                None => docs.push((*id, 1, version)),
            }
        }
        if !docs.is_empty() {
            self.last_story = docs;
        }
    }

    /// `:undo-story`: take back everything the last story's accepted steps did,
    /// in every file, if nothing's been edited in those files since.
    pub(super) fn undo_story(&mut self, cx: &mut Context<Self>) -> Result<(), String> {
        if self.story.is_some() {
            return Err("the story is still under review: esc esc ends it first".into());
        }
        if self.last_story.is_empty() {
            return Err("no story to undo".into());
        }
        let changed = self.last_story.iter().find_map(|(id, _, version)| {
            let tab = self.project.tabs.by_id(*id)?;
            (tab.doc.buffer.version() != *version).then(|| tab.doc.name().to_string())
        });
        if let Some(name) = changed {
            return Err(format!("{name} has been edited since the story: u undoes it step by step"));
        }
        let (mut steps, mut files) = (0, 0);
        for (id, count, _) in std::mem::take(&mut self.last_story) {
            let Some(tab) = self.project.tabs.by_id_mut(id) else { continue };
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
        self.project.say(format!(
            "undid {steps} step{} in {files} file{}: not yet saved",
            if steps == 1 { "" } else { "s" },
            if files == 1 { "" } else { "s" }
        ));
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
        let logged = ghostwrite_ai::memory::forget(&self.project.root);
        self.project.say(format!("starting over: Claude's notes and {logged} earlier request{} are forgotten", if logged == 1 { "" } else { "s" }));
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
        self.story.as_ref().is_some_and(|state| state.story.as_ref().is_none_or(|story| story.involves_model()))
    }

    /// Close the review, saying what came of it, and remember it.
    pub(super) fn finish_story(&mut self, cx: &mut Context<Self>) {
        if let Some(state) = self.story.as_ref() {
            state.finish(&mut self.project);
        }
        self.close_story(cx);
    }

    fn close_story(&mut self, cx: &mut Context<Self>) {
        self.attention.on = Panel::Code;
        self.story_left_the_terminal();
        self.forget_where_the_story_was();
        self.project.tabs.hold(None);
        if let Some(state) = self.story.take() {
            state.conversation.stop();
            self.remember_story(&state.applied.borrow());
            // Asked from select mode: the selection comes back with it.
            if let Some(sel) = state.selection
                && let Some(tab) = self.project.tabs.by_id_mut(state.doc)
            {
                tab.select(sel, Autoscroll::Fit);
            }
        }
        self.modal.leave(Mode::Ai);
        if self.asking() {
            self.prompt = None;
            // The ask field had the keys: the code gets them back.
            self.refocus = true;
        }
        self.project.tree.propose(None);
        cx.notify();
    }
}
