//! `space a`: ask Claude about the lines at the cursor (or the selection),
//! then review its reply one item at a time where it applies, across the
//! project's files. Tab accepts (or reads on); there's no skipping: Enter
//! says what you want instead, and the next item follows it. Esc stops. A
//! file to create is shown in the file tree. An item that can't be found or
//! applied goes back to the model to rewrite before it's shown, and so does
//! a reply that breaks the format.
//!
//! [`StoryState`] is the feature: a [`Story`] (the bookkeeping) laid out in
//! the [`Project`] with a [`Layout`]. It applies what's accepted and says
//! what to show next; [`story_view`](super::story_view) draws it. The
//! editor runs the request and switches tabs. When a story ends it's logged
//! for the next ones, and Claude's rewritten notes (if any) are kept.

use std::collections::HashMap;
use std::ops::Range as Lines;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use futures::StreamExt as _;
use ghostwrite_ai::backend::Listener;
use ghostwrite_ai::context::Shown;
use ghostwrite_ai::placement::{self, Asked, Assumed, Files as _, Overlay, Place, Placer};
use ghostwrite_ai::protocol::{FileOp, Note, Step};
use ghostwrite_ai::{Interaction, Item, ItemId, Layout, Request, Settled, Story, Surroundings, Verdict, lines};
use ghostwrite_core::{Mode, Range, Review};
use ghostwrite_project::tree::Proposal;
use ghostwrite_project::{fs, store};
use gpui::{AppContext as _, Context, Task, Window};

use super::project::ProjectFiles;
use super::{Autoscroll, DocId, Editor, Project, PromptKind};

/// Something about the editor Claude is told as it was when the story
/// started, and again in the newest turn whenever it's changed since.
#[derive(Default)]
pub(super) struct Told {
    /// What the first request said: `None` until it's sent. Kept, so the
    /// conversation's start stays the same.
    pub(super) first: Option<Option<String>>,
    /// What Claude was last told.
    last: Option<String>,
}

impl Told {
    /// It's `now` as a request goes out: what's changed since Claude was
    /// last told, if anything (the first request is told it as it starts).
    fn tell(&mut self, now: Option<String>) -> Option<String> {
        let changed = self.first.is_some() && now != self.last;
        self.last.clone_from(&now);
        self.first.get_or_insert_with(|| now.clone());
        now.filter(|_| changed)
    }

    /// What the conversation starts with.
    fn started(&self) -> Option<String> {
        self.first.clone().flatten()
    }
}

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
    /// The tabs the story opened to show an item, closed as it moves on from them.
    opened: Vec<DocId>,
    /// Asked from select mode: the selection to give back afterwards.
    selection: Option<Range>,
    /// The Claude Code session the story is in, resumed for each follow-up.
    session: Option<String>,
    /// The last `:sh` command (with its output), and the terminal, as Claude is told them.
    pub(super) command: Told,
    pub(super) terminal: Told,
    placer: Placer,
    cancel: Arc<AtomicBool>,
    _task: Option<Task<()>>,
    /// The request sent ahead of time that's on its way, and how to stop it.
    prefetching: Option<(u64, Arc<AtomicBool>)>,
    _prefetch_task: Option<Task<()>>,
    /// The sessions requests sent ahead are in: the next one continues from there, and
    /// the story's session becomes one once its reply is needed.
    ahead_sessions: HashMap<u64, String>,
    /// The items each request sent ahead assumed accepted, to check its reply against.
    ahead_accepted: HashMap<u64, Vec<Item>>,
    /// A reply sent ahead was needed before its session was known.
    session_from: Option<u64>,
}

/// Where an item is in the code: its tab (if its file is open), its file
/// (`None`: the one asked about), and its lines (or the lines to look near).
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

/// Why an item can't be shown as it is.
enum Problem {
    /// Its file isn't in the project: the path, and what to say about it.
    NoFile(String),
    /// Its file is a PDF, which Claude doesn't comment on.
    Pdf(String),
    /// Its `<at>` lines aren't in its file.
    NotFound,
}

/// What to show after the story moved on.
pub(super) enum Show {
    /// Nothing new (still waiting, or already showing it).
    Nothing,
    /// Claude has been asked to correct its reply: send the request.
    Revise,
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
    Session(String),
    Done,
    Failed(String),
}

struct Forward(futures::channel::mpsc::UnboundedSender<Message>);

/// A request sent ahead of time: what it reports goes to the story's replies asked ahead.
struct Ahead(futures::channel::mpsc::UnboundedSender<Message>);

impl Listener for Ahead {
    fn text(&mut self, chunk: &str) {
        self.0.unbounded_send(Message::Text(chunk.to_string())).ok();
    }

    fn looking(&mut self, _what: String) {}

    fn session(&mut self, id: String) {
        self.0.unbounded_send(Message::Session(id)).ok();
    }
}

impl Listener for Forward {
    fn text(&mut self, chunk: &str) {
        self.0.unbounded_send(Message::Text(chunk.to_string())).ok();
    }

    fn looking(&mut self, what: String) {
        self.0.unbounded_send(Message::Looking(what)).ok();
    }

    fn session(&mut self, id: String) {
        self.0.unbounded_send(Message::Session(id)).ok();
    }
}

impl StoryState {
    fn new(doc: DocId, lines: Lines<usize>, root: PathBuf) -> Self {
        Self {
            story: None,
            doc,
            lines,
            docs_note: false,
            shown: None,
            opened: vec![],
            selection: None,
            session: None,
            command: Told::default(),
            terminal: Told::default(),
            placer: Placer::new(root),
            cancel: Arc::default(),
            _task: None,
            prefetching: None,
            _prefetch_task: None,
            ahead_sessions: HashMap::new(),
            ahead_accepted: HashMap::new(),
            session_from: None,
        }
    }

    /// The first request has been sent.
    pub(super) fn asked(&self) -> bool {
        self.story.is_some()
    }

    /// The request for the story so far.
    pub(super) fn request(&self, project: &Project) -> Option<Request> {
        self.request_ahead(project, &[])
    }

    /// The request as it would be with `accepted` applied too, as a request
    /// sent ahead assumes: the files it shows are as they'd be then.
    pub(super) fn request_ahead(&self, project: &Project, accepted: &[Item]) -> Option<Request> {
        let story = self.story.as_ref()?;
        let doc = &project.tabs.by_id(self.doc)?.doc;
        let assumed = self.assume(project, accepted)?;
        Some(Request {
            path: doc.path.clone(),
            language: doc.language_name(),
            range: doc.buffer.lines_range(self.lines.clone()),
            others: self.others(project, &assumed),
            text: assumed.asked,
            instruction: story.instruction.clone(),
            followups: story.followups().to_vec(),
            root: Some(project.root.clone()),
            surroundings: Surroundings { command: self.command.started(), terminal: self.terminal.started(), ..self.surroundings(project) },
            session: self.session.clone(),
            fork: false,
        })
    }

    /// The files as they'd be with `accepted` applied.
    fn assume(&self, project: &Project, accepted: &[Item]) -> Option<Assumed> {
        let asked = project.tabs.by_id(self.doc)?;
        let asked = Asked { path: asked.doc.path.as_deref(), text: asked.doc.text(), lines: self.lines.clone() };
        Some(self.placer.assume(&asked, &project.files(), accepted))
    }

    /// The files the story is in besides the one asked about, as `assumed`
    /// has them: where its items are, and where the steps assumed accepted
    /// went. Each is shown around the last place the story was at in it.
    fn others(&self, project: &Project, assumed: &Assumed) -> Vec<Shown> {
        let mut spots: Vec<(PathBuf, Lines<usize>)> = vec![];
        if let Some(layout) = self.layout(project) {
            for (id, _) in layout.story.items() {
                if let Some(Place { path: Some(path), near }) = layout.lines(id).or_else(|| layout.spot(id)) {
                    spots.push((path, near));
                }
            }
        }
        spots.extend(assumed.changed_at.iter().cloned());
        let base = project.files();
        let files = Overlay { files: &base, changed: &assumed.others };
        let mut shown: Vec<Shown> = vec![];
        for (path, focus) in spots {
            let path_name = project.display(&path);
            match shown.iter_mut().find(|s| s.path == path_name) {
                Some(seen) => seen.focus = focus,
                None => shown.extend(files.text(&path).map(|text| Shown { path: path_name, text, focus })),
            }
        }
        shown
    }

    /// What's around the request in the editor: the open files, and
    /// diagnostics within a few lines of the marked ones.
    fn surroundings(&self, project: &Project) -> Surroundings {
        const NEAR: usize = 5;
        let open_files = project.tabs.iter().filter_map(|t| t.doc.path.as_deref().map(|p| project.display(p))).collect();
        let near = self.lines.start.saturating_sub(NEAR)..self.lines.end + NEAR;
        let diagnostics = project
            .tabs
            .by_id(self.doc)
            .map(|t| {
                t.doc
                    .diagnostics
                    .iter()
                    .filter_map(|d| {
                        let line = t.doc.buffer.line_of(d.range.start);
                        near.contains(&line).then(|| format!("line {}: {}: {}", line + 1, d.severity.label(), d.message.lines().next().unwrap_or_default()))
                    })
                    .collect()
            })
            .unwrap_or_default();
        Surroundings { open_files, diagnostics, command: None, terminal: None }
    }

    fn receive(&mut self, message: Message) {
        if let Message::Session(id) = message {
            self.session = Some(id);
            return;
        }
        let Some(story) = self.story.as_mut() else { return };
        match message {
            Message::Session(_) => {}
            Message::Text(chunk) => story.push_text(&chunk),
            Message::Looking(what) => story.looking = Some(what),
            Message::Done => story.finished_writing(),
            Message::Failed(e) => story.failed(e),
        }
        if !story.is_writing() {
            store::save_transcript(&story.transcript());
        }
    }

    /// What a request sent ahead (`id`) reports.
    fn receive_ahead(&mut self, id: u64, message: Message) {
        let Some(story) = self.story.as_mut() else { return };
        match message {
            Message::Session(session) => {
                if self.session_from == Some(id) {
                    self.session = Some(session.clone());
                }
                self.ahead_sessions.insert(id, session);
            }
            Message::Text(chunk) => story.ahead_text(id, &chunk),
            Message::Looking(_) => {}
            Message::Done => story.ahead_finished(id, None),
            Message::Failed(e) => story.ahead_finished(id, Some(e)),
        }
        if matches!(self.prefetching, Some((on_way, _)) if on_way == id) && !story.wants(id) {
            self.prefetching = None;
        }
    }

    /// A reply sent ahead became the next one: its session is the story's now.
    fn take_adopted(&mut self) {
        let Some(id) = self.story.as_mut().and_then(Story::take_adopted) else { return };
        match self.ahead_sessions.get(&id) {
            Some(session) => self.session = Some(session.clone()),
            None => self.session_from = Some(id),
        }
    }

    /// Place a reply asked ahead (`id`) once, in the files as it assumed them,
    /// and keep where as its anchor.
    fn anchor_ahead(&mut self, project: &Project, id: u64) {
        let Some(item @ Item::Step(_)) = self.story.as_ref().and_then(|s| s.ahead_item(id)) else { return };
        let Some(asked) = project.tabs.by_id(self.doc) else { return };
        let accepted = self.ahead_accepted.get(&id).map(Vec::as_slice).unwrap_or_default();
        let Some(assumed) = self.assume(project, accepted) else { return };
        let base = project.files();
        let files = Overlay { files: &base, changed: &assumed.others };
        let place = self.placer.place(&item, &Asked { path: asked.doc.path.as_deref(), text: &assumed.asked, lines: self.lines.clone() }, &files);
        let text = match &place.path {
            None => Some(assumed.asked.clone()),
            Some(path) => files.text(path),
        };
        if let Some(found) = text.and_then(|text| lines::find(&text, item.quoted(), place.near, item.line().is_some()))
            && let Some(story) = self.story.as_mut()
        {
            story.anchor_ahead(id, found.lines);
        }
    }

    /// Where the story's items show, in `project` as it is now.
    fn layout<'a>(&'a self, project: &'a Project) -> Option<Layout<'a, ProjectFiles<'a>>> {
        let asked = project.tabs.by_id(self.doc)?;
        Some(Layout {
            story: self.story.as_ref()?,
            asked: Asked { path: asked.doc.path.as_deref(), text: asked.doc.text(), lines: self.lines.clone() },
            files: project.files(),
            placer: &self.placer,
        })
    }

    /// Where item `id` is in the code.
    pub(super) fn locate(&self, project: &Project, id: ItemId) -> Option<Located> {
        Some(self.in_tab(project, self.layout(project)?.spot(id)?))
    }

    /// The lines item `id` shows at, in the code.
    pub(super) fn lines_of(&self, project: &Project, id: ItemId) -> Option<Located> {
        Some(self.in_tab(project, self.layout(project)?.lines(id)?))
    }

    /// Where Claude's answer is coming while there's nothing to review:
    /// under what was asked about, or the marked lines.
    pub(super) fn waiting_spot(&self, project: &Project) -> Option<Place> {
        match self.layout(project) {
            Some(layout) => layout.waiting_spot(),
            None => Some(Place { path: None, near: self.lines.clone() }),
        }
    }

    pub(super) fn in_tab(&self, project: &Project, place: Place) -> Located {
        let tab = match &place.path {
            None => project.tabs.index_of_id(self.doc),
            Some(path) => project.tabs.index_of_path(path),
        };
        Located { tab, path: place.path, near: place.near }
    }

    /// Where item `id` is in the code, opening its file (in a tab, not switched to) if it isn't open.
    fn open(&self, project: &mut Project, id: ItemId) -> Option<Located> {
        let located = self.locate(project, id)?;
        match &located.path {
            Some(path) if located.tab.is_none() && path.is_file() => {
                project.open(path)?;
                self.locate(project, id)
            }
            _ => Some(located),
        }
    }

    /// The files the story is waiting in: where the current item is (or
    /// where Claude's answer is coming), and where later items are.
    pub(super) fn files(&self, project: &Project) -> Vec<(PathBuf, Waiting)> {
        let asked = || project.tabs.by_id(self.doc)?.doc.path.clone();
        let file_of = |located: Located| located.path.or_else(asked);
        let current = self.story.as_ref().and_then(Story::current);
        let mut files: Vec<(PathBuf, Waiting)> = self
            .story
            .iter()
            .flat_map(|story| story.items().into_iter().filter(|(id, _)| story.verdict(*id) == Verdict::Pending))
            .filter_map(|(id, _)| Some((file_of(self.locate(project, id)?)?, if Some(id) == current { Waiting::Here } else { Waiting::Later })))
            .collect();
        // Nothing to review yet: Claude is answering where it was asked.
        if current.is_none()
            && self.story.as_ref().is_none_or(Story::is_writing)
            && let Some(place) = self.waiting_spot(project)
        {
            files.extend(file_of(self.in_tab(project, place)).map(|p| (p, Waiting::Here)));
        }
        files
    }

    /// What the tree should show for item `id`: a new file made by an edit,
    /// or a file removed or renamed.
    pub(super) fn proposal(&self, project: &Project, id: ItemId, item: &Item) -> Option<Proposal> {
        let path = self.locate(project, id)?.path;
        match item {
            Item::Step(Step { op: Some(FileOp::Remove), .. }) => path.map(Proposal::Remove),
            Item::Step(Step { op: Some(FileOp::Rename(to)), .. }) => path.map(|p| Proposal::Rename(p, to.trim_start_matches("./").to_string())),
            Item::Step(_) => path.filter(|p| !p.exists()).map(Proposal::Create),
            Item::Note(_) => None,
        }
    }

    /// What's wrong with item `id`, if anything: a file that isn't there (or,
    /// for a new one, already is), or `<at>` lines that aren't in its file.
    fn problem(&self, project: &Project, id: ItemId, item: &Item) -> Option<Problem> {
        let located = self.locate(project, id)?;
        let exists = located.path.as_ref().is_none_or(|p| p.exists());
        let named = || located.path.as_deref().map_or("the file".into(), |p| project.display(p));
        match item {
            Item::Step(step) if step.creates() && exists => Some(Problem::NoFile(format!("{} (it exists already, so edit it with <file>)", named()))),
            Item::Step(step) if !step.creates() && !exists => Some(Problem::NoFile(named())),
            Item::Step(Step { op: Some(FileOp::Rename(to)), .. }) if project.root.join(to.trim_start_matches("./")).exists() => {
                Some(Problem::NoFile(format!("{to} (it exists already, so rename to a path that's free)")))
            }
            Item::Step(Step { op: Some(FileOp::Remove | FileOp::Rename(_)), .. }) => None,
            _ if located.path.as_ref().is_some_and(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("pdf"))) => Some(Problem::Pdf(named())),
            Item::Step(step) if step.complete && !self.placeable(project, id, step) => Some(Problem::NotFound),
            Item::Note(_) if !exists => Some(Problem::NoFile(named())),
            Item::Note(note) if !note.at.is_empty() && self.lines_of(project, id).is_none() => Some(Problem::NotFound),
            _ => None,
        }
    }

    /// Whether step `id` applies to its file as it is now (or makes a new file).
    fn placeable(&self, project: &Project, id: ItemId, step: &Step) -> bool {
        let Some(located) = self.locate(project, id) else { return false };
        let text = match (located.tab.and_then(|ix| project.tabs.get(ix)), &located.path) {
            (Some(tab), _) => Some(tab.doc.text().to_string()),
            (None, Some(path)) => std::fs::read_to_string(path).ok(),
            (None, None) => None,
        };
        match text {
            Some(text) => placement::edit(&text, step, located.near).is_some(),
            None => step.old.is_empty(),
        }
    }

    /// Item `id`'s lines aren't in its file: ask Claude to rewrite it, shown
    /// the file around where they should be.
    fn revise_lines(&mut self, project: &Project, id: ItemId, item: &Item) {
        let Some(located) = self.locate(project, id) else { return };
        let asked = project.tabs.by_id(self.doc).map(|t| &t.doc);
        let (file, text) = match &located.path {
            Some(path) => (project.display(path), project.files().text(path)),
            None => (asked.and_then(|d| d.path.as_deref()).map_or("the file".into(), |p| project.display(p)), asked.map(|d| d.text().to_string())),
        };
        if let Some(story) = self.story.as_mut() {
            story.misplaced(&file, &text.unwrap_or_default(), item.quoted(), located.near);
        }
        self.shown = None;
    }

    /// Accept the current item, carrying it out.
    fn accept(&mut self, project: &mut Project) -> Result<(), String> {
        let Some((id, item)) = self.story.as_ref().and_then(|s| Some((s.current()?, s.current_item()?))) else { return Ok(()) };
        self.apply(project, id, &item)?;
        if let Some(story) = self.story.as_mut() {
            story.accept();
        }
        Ok(())
    }

    fn apply(&self, project: &mut Project, id: ItemId, item: &Item) -> Result<(), String> {
        match item {
            Item::Note(_) => Ok(()),
            Item::Step(step) => self.apply_step(project, id, step),
        }
    }

    fn apply_step(&self, project: &mut Project, id: ItemId, step: &Step) -> Result<(), String> {
        if !step.complete {
            return Err("that step is still being written".into());
        }
        // A whole file removed or renamed.
        match (&step.op, self.locate(project, id).and_then(|l| l.path)) {
            (Some(FileOp::Remove), Some(path)) => {
                fs::trash(&path)?;
                project.path_deleted(&path);
                project.tree.refresh();
                return Ok(());
            }
            (Some(FileOp::Rename(to)), Some(path)) => {
                let to = fs::move_to(&path, &project.root.join(to.trim_start_matches("./")))?;
                project.path_moved(&path, &to);
                project.tree.refresh();
                return Ok(());
            }
            _ => {}
        }
        // A new file: make it, then write its first lines.
        if let Some(path) = self.locate(project, id).and_then(|l| l.path).filter(|p| step.creates() && !p.exists()) {
            let rel = path.strip_prefix(&project.root).map_err(|_| format!("{} is outside the project", path.display()))?;
            fs::create(&project.root, &rel.to_string_lossy())?;
            project.tree.refresh();
        }
        let missing = "that edit's file doesn't exist; ⏎ to say what you want instead";
        let located = self.open(project, id).ok_or(missing)?;
        let tab = located.tab.and_then(|ix| project.tabs.get_mut(ix)).ok_or(missing)?;
        let edit = placement::edit(tab.doc.text(), step, located.near).ok_or("couldn't find where that edit goes; ⏎ to say what you want instead")?;
        tab.doc.buffer.begin(tab.view.selection);
        tab.doc.buffer.edit(edit.bytes, &edit.text);
        let end = tab.doc.buffer.line_end(edit.cursor_line);
        tab.select(Range::point(end), Autoscroll::Fit);
        tab.doc.buffer.commit(tab.view.selection);
        tab.sync();
        // Claude Code reads the disk: write what was accepted so it never works
        // from the old file. It's accepted either way (it's in the buffer).
        if let Some(path) = tab.doc.path.clone()
            && let Err(e) = tab.doc.save(&path, false)
        {
            project.say(format!("accepted, but couldn't save: {e}"));
        }
        Ok(())
    }

    /// After anything changes: move on, and put a newly current item where
    /// it'll be seen: its lines selected and centered in its file's tab, or
    /// its proposal in the tree.
    fn settle(&mut self, project: &mut Project) -> Show {
        let show = self.move_on(project);
        self.take_adopted();
        show
    }

    fn move_on(&mut self, project: &mut Project) -> Show {
        let Some(story) = self.story.as_mut() else { return Show::Nothing };
        // A reply that breaks the format is stopped there and written again.
        if story.is_writing()
            && story.can_revise()
            && let Some(why) = story.format_problem()
        {
            story.reply_unusable(Some(&why));
            return Show::Revise;
        }
        let id = match story.settle() {
            // Nothing in the reply to review: the model writes it again.
            Settled::Finished if story.is_unusable() && story.can_revise() => {
                let why = story.format_problem();
                story.reply_unusable(why.as_deref());
                return Show::Revise;
            }
            Settled::Ask => return Show::Revise,
            Settled::Finished => return Show::Finished,
            Settled::Waiting => return Show::Nothing,
            Settled::At(id) => id,
        };
        if self.shown == Some(id) {
            return Show::Nothing;
        }
        let Some(item) = story.item(id) else { return Show::Nothing };
        // Never show an item that can't be found or applied: the model writes it again first.
        if story.can_revise() && !self.docs_note {
            match self.problem(project, id, &item) {
                Some(Problem::NoFile(file)) => {
                    if let Some(story) = self.story.as_mut() {
                        story.file_missing(&file);
                    }
                    self.shown = None;
                    return Show::Revise;
                }
                Some(Problem::Pdf(file)) => {
                    if let Some(story) = self.story.as_mut() {
                        story.about_a_pdf(&file);
                    }
                    self.shown = None;
                    return Show::Revise;
                }
                Some(Problem::NotFound) => {
                    self.revise_lines(project, id, &item);
                    return Show::Revise;
                }
                None => {}
            }
        }
        self.shown = Some(id);
        let proposal = self.proposal(project, id, &item);
        let in_tree = proposal.is_some();
        project.tree.propose(proposal);
        if in_tree {
            self.close_opened(project, None);
            return Show::Tree;
        }
        let was_open = self.locate(project, id).is_some_and(|l| l.tab.is_some());
        let Some(Located { tab: Some(ix), near, .. }) = self.open(project, id) else { return Show::Nothing };
        let Some(here) = project.tabs.get(ix).map(|t| t.doc.id) else { return Show::Nothing };
        if !was_open {
            self.opened.push(here);
        }
        self.close_opened(project, Some(here));
        let Some(ix) = project.tabs.index_of_id(here) else { return Show::Nothing };
        // Its own lines, or (if they aren't found) where it was placed: either way, go there.
        let lines = self.lines_of(project, id).map_or(near, |l| l.near);
        let Some(tab) = project.tabs.get_mut(ix) else { return Show::Nothing };
        let last = tab.doc.buffer.last_line();
        let lines = lines.start.min(last)..lines.end.clamp(lines.start.min(last) + 1, last + 1);
        if self.docs_note {
            tab.view.autoscroll = Some(Autoscroll::Fit);
            return Show::Tab(ix);
        }
        let cursor = match item {
            Item::Note(_) => tab.doc.buffer.line_start(lines.start),
            _ => tab.doc.buffer.line_end(lines.end - 1),
        };
        tab.select(Range::point(cursor), Autoscroll::Center(lines));
        Show::Tab(ix)
    }

    /// Close the tabs the story opened for earlier items, but `keep` (the
    /// one it's showing now) and any with unsaved changes.
    fn close_opened(&mut self, project: &mut Project, keep: Option<DocId>) {
        let tabs = &project.tabs;
        let closing: Vec<DocId> = self.opened.iter().copied().filter(|&d| Some(d) != keep && tabs.by_id(d).is_some_and(|t| !t.doc.dirty)).collect();
        if closing.is_empty() {
            return;
        }
        self.opened.retain(|d| !closing.contains(d));
        project.tabs.retain(|t| !closing.contains(&t.doc.id));
        project.tree.set_active(project.active_path());
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
            let said = story.followups().iter().filter(|f| !f.correction && !f.said.is_empty()).map(|f| f.said.clone()).collect();
            Interaction::new(file, story.instruction.clone(), said, story.outcome(), story.recap()).log(&project.root);
        }
    }
}

impl Editor {
    /// Open the prompt about the cursor's lines (or the selection's), with
    /// text typed in.
    pub(super) fn open_story(&mut self, typed: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.tab().is_some_and(|t| t.doc.pdf.is_some()) {
            self.project.say("Claude doesn't comment on PDFs: ask in the file that makes it");
            return;
        }
        if self.backends.is_empty() {
            self.project.say("asking Claude needs ANTHROPIC_API_KEY or Claude Code (`claude`) installed");
            return;
        }
        if self.tab().is_none() {
            self.project.say("open a file to ask about");
            return;
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
        self.story = Some(state);
        self.open_prompt(PromptKind::Ask, typed, window, cx);
    }

    /// A story of `items` the editor gives (replacing any story under
    /// review), from the file in view: each is shown in turn, and asking about
    /// one hands over to the model.
    pub(super) fn open_given(&mut self, instruction: String, items: Vec<Item>, cx: &mut Context<Self>) {
        if self.reviewing_claude() {
            self.project.say("Claude's story is still under review: esc esc ends it first");
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
        self.story = Some(state);
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
            None => state.story = Some(Story::new(said)),
        }
        self.close_prompt(window, cx);
        self.send_request(cx);
    }

    /// Send the story's request on a background thread; the reply streams in.
    fn send_request(&mut self, cx: &mut Context<Self>) {
        // Claude reads the project from disk: let it see what's on screen.
        self.project.write_all();
        let Some(backend) = self.backends.first().cloned() else { return };
        let (command, terminal) = (self.shell.as_ref().map(|s| s.context()), self.terminal_context());
        let Some(state) = self.story.as_mut() else { return };
        // The first request says what the last command was and what's in the
        // terminal; a follow-up, what's been run since and what the terminal shows now.
        let ran = state.command.tell(command);
        let seen = state.terminal.tell(terminal);
        if let (Some(command), Some(story)) = (ran, state.story.as_mut()) {
            story.ran(command);
        }
        if let (Some(terminal), Some(story)) = (seen, state.story.as_mut()) {
            story.saw_terminal(terminal);
        }
        let Some(req) = state.request(&self.project) else { return };
        // Whatever was still coming is superseded.
        state.cancel.store(true, Ordering::SeqCst);
        state.cancel = Arc::new(AtomicBool::new(false));
        let cancel = state.cancel.clone();
        let mine = cancel.clone();
        let (tx, mut rx) = futures::channel::mpsc::unbounded();
        cx.background_spawn(async move {
            let result = backend.run(&req, &mut Forward(tx.clone()), &cancel);
            tx.unbounded_send(result.map_or_else(Message::Failed, |()| Message::Done)).ok();
        })
        .detach();
        state._task = Some(cx.spawn(async move |this, cx| {
            while let Some(message) = rx.next().await {
                let done = matches!(message, Message::Done | Message::Failed(_));
                let alive = this
                    .update(cx, |this, cx| {
                        // A newer request (a correction sent mid-reply) supersedes this one.
                        let Some(state) = this.story.as_mut().filter(|s| Arc::ptr_eq(&s.cancel, &mine)) else { return false };
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

    /// A review key for the story, which only goes forward.
    pub(super) fn story_review(&mut self, key: Review, window: &mut Window, cx: &mut Context<Self>) {
        match key {
            Review::Next => self.accept(cx),
            Review::Ask => self.open_prompt(PromptKind::Ask, "", window, cx),
            Review::Stop => self.finish_story(cx),
            Review::Stopping => self.project.say("esc again to end this story"),
        }
    }

    fn accept(&mut self, cx: &mut Context<Self>) {
        let Some(state) = self.story.as_mut() else { return };
        if let Err(e) = state.accept(&mut self.project) {
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
            Show::Revise => self.send_request(cx),
            Show::Tab(ix) => {
                if ix != self.project.tabs.active_index() {
                    self.activate(ix, cx);
                }
            }
            Show::Tree => self.tree_pane.shown = true,
            Show::Finished => self.finish_story(cx),
        }
        self.prefetch(cx);
        cx.notify();
    }

    /// While an item is reviewed, ask ahead for the next one (and, as each
    /// arrives, the one after), assuming each is accepted. One that's no
    /// longer wanted is stopped.
    fn prefetch(&mut self, cx: &mut Context<Self>) {
        let Some(backend) = self.backends.first().cloned() else { return };
        let Some(state) = self.story.as_mut() else { return };
        let Some(story) = state.story.as_mut() else { return };
        if let Some((id, cancel)) = &state.prefetching {
            if story.wants(*id) {
                return;
            }
            cancel.store(true, Ordering::SeqCst);
            state.prefetching = None;
        }
        let Some(ahead) = story.prefetch() else { return };
        let Some(mut req) = state.request_ahead(&self.project, &ahead.accepted) else { return };
        req.followups = ahead.followups;
        req.session = match ahead.after {
            None => state.session.clone(),
            Some(before) => state.ahead_sessions.get(&before).cloned(),
        };
        req.fork = req.session.is_some();
        state.ahead_accepted.insert(ahead.id, ahead.accepted);
        let (id, cancel) = (ahead.id, Arc::new(AtomicBool::new(false)));
        state.prefetching = Some((id, cancel.clone()));
        let (tx, mut rx) = futures::channel::mpsc::unbounded();
        cx.background_spawn(async move {
            let result = backend.run(&req, &mut Ahead(tx.clone()), &cancel);
            tx.unbounded_send(result.map_or_else(Message::Failed, |()| Message::Done)).ok();
        })
        .detach();
        state._prefetch_task = Some(cx.spawn(async move |this, cx| {
            while let Some(message) = rx.next().await {
                let done = matches!(message, Message::Done | Message::Failed(_));
                let alive = this
                    .update(cx, |this, cx| {
                        let Some(state) = this.story.as_mut() else { return false };
                        state.receive_ahead(id, message);
                        if done {
                            state.anchor_ahead(&this.project, id);
                        }
                        if done && matches!(state.prefetching, Some((on_way, _)) if on_way == id) {
                            state.prefetching = None;
                        }
                        this.settle_story(cx);
                        true
                    })
                    .unwrap_or(false);
                if done || !alive {
                    break;
                }
            }
        }));
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
        if let Some(state) = self.story.take() {
            state.cancel.store(true, Ordering::SeqCst);
            if let Some((_, cancel)) = &state.prefetching {
                cancel.store(true, Ordering::SeqCst);
            }
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
