//! `space i`: ask Claude about the lines at the cursor (or the selection),
//! then review its reply where it applies, across the project's files. Tab
//! accepts (or reads on), Backspace skips, Enter asks or redirects, Esc
//! stops. A file to create, remove or rename is shown in the file tree.
//!
//! [`StoryState`] is the feature: a [`Story`] (the bookkeeping) placed in
//! the [`Project`] with [`placement`]. It applies what's accepted and says
//! what to show next; [`story_view`](super::story_view) draws it. The
//! editor runs the request and switches tabs. When a story ends it's logged
//! for the next ones, and Claude's rewritten notes (if any) are kept.

use std::ops::Range as Lines;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use futures::StreamExt as _;
use ghostwrite_ai::backend::Listener;
use ghostwrite_ai::placement::{self, Asked, Placer};
use ghostwrite_ai::protocol::{self, Note, Step};
use ghostwrite_ai::{Interaction, Item, ItemId, Request, Settled, Story, Verdict};
use ghostwrite_core::{Mode, Range, Review};
use ghostwrite_project::tree::Proposal;
use ghostwrite_project::{fs, store};
use gpui::{AppContext as _, Context, Task, Window};

use super::{Autoscroll, DocId, Editor, Project, PromptKind};

pub(super) struct StoryState {
    /// `None` until the first request is sent.
    pub(super) story: Option<Story>,
    /// The document asked about, and the lines pointed at (zero-based, end exclusive).
    pub(super) doc: DocId,
    pub(super) lines: Lines<usize>,
    /// Command output it's about (an error from `:sh`).
    pub(super) output: Option<String>,
    /// Docs from `space k`: shown without moving the cursor, as helix does.
    docs_note: bool,
    /// Asked about a tour's output from its panel: answered there too.
    pub(super) in_panel: bool,
    /// The item last brought into view.
    shown: Option<ItemId>,
    /// Asked from select mode: the selection to give back afterwards.
    selection: Option<Range>,
    placer: Placer,
    cancel: Arc<AtomicBool>,
    _task: Option<Task<()>>,
}

/// Where an item is: its tab (if its file is open), its file (`None`: the
/// one asked about), and the lines to look for it near.
pub(super) struct Located {
    pub tab: Option<usize>,
    pub path: Option<PathBuf>,
    pub near: Lines<usize>,
}

/// How a file in the tree is waiting on the story.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Waiting {
    /// The current item is here (or Claude's answer is coming here).
    Here,
    /// Later items are here.
    Later,
}

/// What to show after the story moved on.
pub(super) enum Show {
    /// Nothing new (still waiting, or already showing it).
    Nothing,
    /// This tab, where the current item is (its selection already set).
    Tab(usize),
    /// The file tree, where a proposal about a file is.
    Tree,
    /// It's over.
    Finished,
}

/// What a running request reports, on its way to the editor.
enum Message {
    Text(String),
    Looking(String),
    Done,
    Failed(String),
}

struct Forward(futures::channel::mpsc::UnboundedSender<Message>);

impl Listener for Forward {
    fn text(&mut self, chunk: &str) {
        self.0.unbounded_send(Message::Text(chunk.to_string())).ok();
    }

    fn looking(&mut self, what: String) {
        self.0.unbounded_send(Message::Looking(what)).ok();
    }
}

impl StoryState {
    fn new(doc: DocId, lines: Lines<usize>, output: Option<String>, root: PathBuf) -> Self {
        Self {
            story: None,
            doc,
            lines,
            output,
            docs_note: false,
            in_panel: false,
            shown: None,
            selection: None,
            placer: Placer::new(root),
            cancel: Arc::default(),
            _task: None,
        }
    }

    /// The first request has been sent.
    pub(super) fn asked(&self) -> bool {
        self.story.is_some()
    }

    /// The request for the story so far.
    fn request(&self, project: &Project) -> Option<Request> {
        let story = self.story.as_ref()?;
        let doc = &project.tabs.by_id(self.doc)?.doc;
        Some(Request {
            path: doc.path.clone(),
            language: doc.language_name(),
            text: doc.text().to_string(),
            range: doc.buffer.lines_range(self.lines.clone()),
            instruction: story.instruction.clone(),
            followups: story.followups().to_vec(),
            output: story.output.clone(),
            root: Some(project.root.clone()),
        })
    }

    fn receive(&mut self, message: Message) {
        let Some(story) = self.story.as_mut() else { return };
        match message {
            Message::Text(chunk) => story.push_text(&chunk),
            Message::Looking(what) => story.looking = Some(what),
            Message::Done => story.finished_writing(),
            Message::Failed(e) => story.failed(e),
        }
        if !story.is_writing() {
            store::save_transcript(&story.transcript());
        }
    }

    /// Where an item is, without opening anything.
    pub(super) fn locate(&self, project: &Project, item: &Item) -> Option<Located> {
        let asked_tab = project.tabs.by_id(self.doc)?;
        let asked = Asked { path: asked_tab.doc.path.as_deref(), text: asked_tab.doc.text(), lines: self.lines.clone() };
        let place = self.placer.place(item, &asked, &project.files());
        let tab = match &place.path {
            None => project.tabs.index_of_id(self.doc),
            Some(path) => project.tabs.index_of_path(path),
        };
        Some(Located { tab, path: place.path, near: place.near })
    }

    /// An answer that isn't pinned to lines, shown in the tour panel it was
    /// asked from (and so is one to a question asked about it there).
    pub(super) fn in_panel_item(&self, id: ItemId, item: &Item) -> bool {
        let Some(story) = self.story.as_ref().filter(|_| self.in_panel) else { return false };
        matches!(item, Item::Note(Note { file: None, at, .. }) if at.is_empty())
            && story.asked_about(id).is_none_or(|about| story.item(about).is_some_and(|it| self.in_panel_item(about, &it)))
    }

    /// Where item `id` is. Like [`Self::locate`], but an answer that isn't
    /// pinned to lines goes where its question was asked.
    pub(super) fn locate_at(&self, project: &Project, id: ItemId, item: &Item) -> Option<Located> {
        let story = self.story.as_ref()?;
        if let Item::Note(Note { file: None, at, .. }) = item
            && at.is_empty()
            && let Some(asked) = story.asked_about(id).and_then(|about| self.lines_of(project, about))
        {
            return Some(asked);
        }
        self.locate(project, item)
    }

    /// The tab and lines item `id` shows at (`near` is its lines).
    pub(super) fn lines_of(&self, project: &Project, id: ItemId) -> Option<Located> {
        let item = self.story.as_ref()?.item(id)?;
        if self.in_panel_item(id, &item) {
            return None;
        }
        let located = self.locate_at(project, id, &item)?;
        let text = project.tabs.get(located.tab?)?.doc.text();
        let near = match &item {
            Item::Step(step) => {
                let view = placement::view(text, step, located.near.clone())?;
                view.first..view.anchor + 1
            }
            Item::Note(note) => protocol::locate(text, &note.at, located.near.clone())?,
            _ => return None,
        };
        Some(Located { near, ..located })
    }

    /// The files the story is waiting in: where the current item is (or
    /// where Claude's answer is coming), and where later items are.
    pub(super) fn files(&self, project: &Project) -> Vec<(PathBuf, Waiting)> {
        let asked = || project.tabs.by_id(self.doc)?.doc.path.clone();
        let file_of = |located: Located| located.path.or_else(asked);
        let Some(story) = self.story.as_ref() else { return asked().map(|p| (p, Waiting::Here)).into_iter().collect() };
        let current = story.current();
        let mut files: Vec<(PathBuf, Waiting)> = story
            .items()
            .into_iter()
            .filter(|(id, item)| story.verdict(*id) == Verdict::Pending && !item.is_file_op() && !self.in_panel_item(*id, item))
            .filter_map(|(id, item)| {
                let path = file_of(self.locate_at(project, id, &item)?)?;
                Some((path, if Some(id) == current { Waiting::Here } else { Waiting::Later }))
            })
            .collect();
        // Nothing to review yet: Claude is answering where it was asked.
        if current.is_none() && story.is_writing() && !self.in_panel {
            let at = story.writing_about().and_then(|about| self.lines_of(project, about)).map_or_else(asked, file_of);
            files.extend(at.map(|p| (p, Waiting::Here)));
        }
        files
    }

    /// Where item `id` is, opening its file (in a tab, not switched to) if it isn't open.
    fn open_located_at(&self, project: &mut Project, id: ItemId, item: &Item) -> Option<Located> {
        let located = self.locate_at(project, id, item)?;
        match &located.path {
            Some(path) if located.tab.is_none() && path.is_file() => {
                project.open(path)?;
                self.locate_at(project, id, item)
            }
            _ => Some(located),
        }
    }

    /// Where an item is, opening its file (in a tab, not switched to) if it isn't open.
    fn open_located(&self, project: &mut Project, item: &Item) -> Option<Located> {
        let located = self.locate(project, item)?;
        match &located.path {
            Some(path) if located.tab.is_none() && path.is_file() => {
                project.open(path)?;
                self.locate(project, item)
            }
            _ => Some(located),
        }
    }

    /// What the tree should show for item `id`: a file operation, the one
    /// an answer was asked about, or a step for a file not made yet.
    pub(super) fn proposal(&self, project: &Project, id: ItemId, item: &Item) -> Option<Proposal> {
        let story = self.story.as_ref()?;
        let about = story.asked_about(id).and_then(|a| story.item(a)).filter(Item::is_file_op);
        let root = &project.root;
        match about.as_ref().unwrap_or(item) {
            Item::Create(rel) => Some(Proposal::Create(root.join(rel))),
            Item::Remove(rel) => Some(Proposal::Remove(root.join(rel.trim_end_matches('/')))),
            Item::Rename { from, to } => Some(Proposal::Rename(root.join(from.trim_end_matches('/')), to.clone())),
            Item::Step(_) => self.locate(project, item)?.path.filter(|p| !p.exists()).map(Proposal::Create),
            Item::Note(_) => None,
        }
    }

    /// Accept (carrying it out) or skip the current item.
    fn decide(&mut self, project: &mut Project, verdict: Verdict) -> Result<(), String> {
        let Some(item) = self.story.as_ref().and_then(Story::current_item) else { return Ok(()) };
        if verdict == Verdict::Accepted {
            self.apply(project, &item)?;
        }
        if let Some(story) = self.story.as_mut() {
            story.decide(verdict);
        }
        Ok(())
    }

    fn apply(&self, project: &mut Project, item: &Item) -> Result<(), String> {
        let path = |rel: &str| project.root.join(rel.trim_end_matches('/'));
        match item {
            Item::Note(_) => {}
            Item::Create(rel) => {
                let target = path(rel);
                if !target.exists() {
                    fs::create(&project.root, rel)?;
                }
                project.tree.refresh();
                if target.is_file() {
                    project.open(&target);
                }
            }
            Item::Rename { from, to } => {
                let (from, to) = (path(from), path(to));
                if !to.starts_with(&project.root) {
                    return Err(format!("can't move {} outside the project", from.display()));
                }
                fs::move_to(&from, &to)?;
                project.path_moved(&from, &to);
            }
            Item::Remove(rel) => {
                let target = path(rel);
                if target.exists() {
                    fs::trash(&target)?;
                    project.path_deleted(&target);
                }
            }
            Item::Step(step) => self.apply_step(project, step, item)?,
        }
        Ok(())
    }

    fn apply_step(&self, project: &mut Project, step: &Step, item: &Item) -> Result<(), String> {
        if !step.complete {
            return Err("that step is still being written".into());
        }
        // A step for a file that isn't there yet (Claude didn't propose creating it): make it.
        if let Some(path) = self.locate(project, item).and_then(|l| l.path).filter(|p| !p.exists() && step.old.is_empty()) {
            let rel = path.strip_prefix(&project.root).map_err(|_| format!("{} is outside the project", path.display()))?;
            fs::create(&project.root, &rel.to_string_lossy())?;
            project.tree.refresh();
        }
        let missing = "that step's file doesn't exist; backspace skips it";
        let located = self.open_located(project, item).ok_or(missing)?;
        let tab = located.tab.and_then(|ix| project.tabs.get_mut(ix)).ok_or(missing)?;
        let edit = placement::edit(tab.doc.text(), step, located.near).ok_or("couldn't find where that step goes; backspace skips it")?;
        tab.doc.buffer.begin(tab.view.selection);
        tab.doc.buffer.edit(edit.bytes, &edit.text);
        let end = tab.doc.buffer.line_end(edit.cursor_line);
        tab.select(Range::point(end), Autoscroll::Fit);
        tab.doc.buffer.commit(tab.view.selection);
        tab.sync();
        Ok(())
    }

    /// After anything changes: move on, and put a newly current item where
    /// it'll be seen: its lines selected and centered in its file's tab, or
    /// its proposal in the tree.
    fn settle(&mut self, project: &mut Project) -> Show {
        let Some(story) = self.story.as_mut() else { return Show::Nothing };
        let id = match story.settle() {
            Settled::Finished => return Show::Finished,
            Settled::Waiting => return Show::Nothing,
            Settled::At(id) => id,
        };
        if self.shown == Some(id) {
            return Show::Nothing;
        }
        let Some(item) = story.item(id) else { return Show::Nothing };
        self.shown = Some(id);
        let proposal = self.proposal(project, id, &item);
        let in_tree = proposal.is_some();
        project.tree.propose(proposal);
        if in_tree {
            return Show::Tree;
        }
        if self.in_panel_item(id, &item) {
            return Show::Nothing;
        }
        let Some(Located { tab: Some(ix), near, .. }) = self.open_located_at(project, id, &item) else { return Show::Nothing };
        let Some(tab) = project.tabs.get_mut(ix) else { return Show::Nothing };
        let (first, anchor, cursor) = match &item {
            Item::Step(step) => match placement::view(tab.doc.text(), step, near) {
                Some(view) => (view.first, view.anchor, tab.doc.buffer.line_end(view.anchor)),
                None => return Show::Tab(ix),
            },
            Item::Note(note) => match protocol::locate(tab.doc.text(), &note.at, near) {
                Some(at) => (at.start, at.end - 1, tab.doc.buffer.line_start(at.start)),
                None => return Show::Tab(ix),
            },
            _ => return Show::Nothing,
        };
        if self.docs_note {
            tab.view.autoscroll = Some(Autoscroll::Fit);
        } else {
            tab.select(Range::point(cursor), Autoscroll::Center(first..anchor + 1));
        }
        Show::Tab(ix)
    }

    /// The story is over: say what came of it, keep Claude's notes, and log it.
    fn finish(&self, project: &mut Project) {
        let Some(story) = self.story.as_ref() else { return };
        let (kept, total) = story.summary();
        if total > 0 {
            project.say(format!("applied {kept} of {total} steps"));
        }
        if let Some(notes) = story.memory_update() {
            store::save_memory(&project.root, &notes);
        }
        if !self.docs_note || !story.followups().is_empty() {
            let path = project.tabs.by_id(self.doc).and_then(|t| t.doc.path.clone());
            let file = path.map_or("untitled".into(), |p| project.display(&p));
            let said = story.followups().iter().map(|f| f.said.clone()).collect();
            Interaction::new(file, story.instruction.clone(), said, story.outcome()).log(&project.root);
        }
    }
}

impl Editor {
    /// Open the prompt about the cursor's lines (or the selection's),
    /// optionally about some command output and with text typed in.
    pub(super) fn open_story(&mut self, output: Option<String>, typed: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.backend.is_none() {
            self.project.say("asking Claude needs ANTHROPIC_API_KEY or Claude Code (`claude`) installed");
            return;
        }
        // The story is reviewed in AI mode, and afterwards it's back to where
        // you were. A tour waits (if it isn't already handing a stop over).
        if let Some(state) = self.tour.as_mut().filter(|s| !s.tour.paused()) {
            state.tour.pause();
        }
        self.leave_insert();
        self.modal.enter(Mode::Ai);
        let from_select = self.modal.entered_from() == Some(Mode::Select);
        let root = self.project.root.clone();
        let Some(tab) = self.project.tab_mut() else {
            self.project.say("open a file to ask about");
            return;
        };
        let sel = tab.view.selection;
        let kept = from_select.then_some(sel);
        let (from, to) = if sel.is_wide(tab.doc.text()) { (sel.from(), tab.doc.buffer.prev(sel.to())) } else { (tab.cursor(), tab.cursor()) };
        let buffer = &tab.doc.buffer;
        let lines = buffer.line_of(from)..buffer.line_of(to) + 1;
        let span = buffer.lines_range(lines.clone());
        let end = span.end - usize::from(buffer.text()[span.clone()].ends_with('\n'));
        tab.view.selection = Range::new(span.start, end.max(span.start));
        let mut state = StoryState::new(tab.doc.id, lines, output, root);
        state.selection = kept;
        self.story = Some(state);
        self.open_prompt(PromptKind::Ask, typed, window, cx);
    }

    /// Ask about a tour's output from its panel: the prompt, and the answer, show there.
    pub(super) fn open_story_in_panel(&mut self, output: String, window: &mut Window, cx: &mut Context<Self>) {
        self.open_story(Some(output), "", window, cx);
        if let Some(state) = self.story.as_mut() {
            state.in_panel = true;
        }
    }

    /// Show `text` as a note under the cursor's line, reviewed like an
    /// answer (`space k` docs); Enter asks Claude about it.
    pub(super) fn show_note(&mut self, text: &str, cx: &mut Context<Self>) {
        self.leave_insert();
        let Some(tab) = self.tab() else { return };
        let line = tab.doc.buffer.line_of(tab.cursor());
        let mut state = StoryState::new(tab.doc.id, line..line + 1, None, self.project.root.clone());
        self.modal.enter(Mode::Ai);
        state.docs_note = true;
        state.story = Some(Story::answered("Explain what's under the cursor; its documentation was shown.", text));
        self.story = Some(state);
        self.settle_story(cx);
    }

    /// Enter in the prompt: the first ask, or (while reviewing) something
    /// said about the current item.
    pub(super) fn ask(&mut self, said: String, window: &mut Window, cx: &mut Context<Self>) {
        if said.trim().is_empty() {
            return;
        }
        let Some(state) = self.story.as_mut() else { return };
        match state.story.as_mut() {
            Some(story) => {
                state.cancel.store(true, Ordering::SeqCst);
                story.follow_up(said);
                state.shown = None;
            }
            None => state.story = Some(Story::new(said, state.output.clone())),
        }
        self.close_prompt(window, cx);
        self.send_request(window, cx);
    }

    /// Send the story's request on a background thread; the reply streams in.
    fn send_request(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Claude reads the project from disk: let it see what's on screen.
        self.project.write_all();
        let Some(backend) = self.backend.clone() else { return };
        let Some(state) = self.story.as_mut() else { return };
        let Some(req) = state.request(&self.project) else { return };
        state.cancel = Arc::new(AtomicBool::new(false));
        let cancel = state.cancel.clone();
        let (tx, mut rx) = futures::channel::mpsc::unbounded();
        cx.background_spawn(async move {
            let result = backend.run(&req, &mut Forward(tx.clone()), &cancel);
            tx.unbounded_send(result.map_or_else(Message::Failed, |()| Message::Done)).ok();
        })
        .detach();
        state._task = Some(cx.spawn_in(window, async move |this, cx| {
            while let Some(message) = rx.next().await {
                let done = matches!(message, Message::Done | Message::Failed(_));
                let alive = this
                    .update(cx, |this, cx| {
                        let Some(state) = this.story.as_mut() else { return false };
                        state.receive(message);
                        this.settle_story(cx);
                        true
                    })
                    .unwrap_or(false);
                if done || !alive {
                    break;
                }
            }
        }));
        cx.notify();
    }

    /// Esc in the prompt: before asking, close; while reviewing, back to the review.
    pub(super) fn cancel_ask(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let asked = self.story.as_ref().is_some_and(StoryState::asked);
        self.close_prompt(window, cx);
        if !asked {
            self.close_story(cx);
        }
    }

    /// A review key for the story. A story only goes forward: Shift+Tab does nothing.
    pub(super) fn story_review(&mut self, key: Review, window: &mut Window, cx: &mut Context<Self>) {
        match key {
            Review::Next => self.decide(Verdict::Accepted, cx),
            Review::Skip => self.decide(Verdict::Rejected, cx),
            Review::Ask => self.open_prompt(PromptKind::Ask, "", window, cx),
            Review::Stop => self.finish_story(cx),
            Review::Back => {}
        }
    }

    fn decide(&mut self, verdict: Verdict, cx: &mut Context<Self>) {
        let Some(state) = self.story.as_mut() else { return };
        if let Err(e) = state.decide(&mut self.project, verdict) {
            self.project.say(e);
            return;
        }
        self.settle_story(cx);
    }

    /// Bring the story's current item into view (or finish).
    fn settle_story(&mut self, cx: &mut Context<Self>) {
        let Some(state) = self.story.as_mut() else { return };
        match state.settle(&mut self.project) {
            Show::Nothing => {}
            Show::Tab(ix) => {
                if ix != self.project.tabs.active_index() {
                    self.activate(ix, cx);
                }
            }
            Show::Tree => self.show_tree = true,
            Show::Finished => self.finish_story(cx),
        }
        cx.notify();
    }

    /// Close the review, saying what came of it, and remember it.
    pub(super) fn finish_story(&mut self, cx: &mut Context<Self>) {
        if let Some(state) = self.story.as_ref() {
            state.finish(&mut self.project);
        }
        self.close_story(cx);
    }

    fn close_story(&mut self, cx: &mut Context<Self>) {
        if let Some(state) = self.story.take() {
            state.cancel.store(true, Ordering::SeqCst);
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
        }
        self.project.tree.propose(None);
        // Fixing an error from `:sh`: on to the next one.
        self.resume_tour(cx);
        cx.notify();
    }
}
