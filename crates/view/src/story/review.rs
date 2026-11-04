//! The story in the code: where each item is (opening its file when it's
//! shown), what's wrong with one that can't be shown as it is (it goes back
//! to Claude to rewrite), carrying out what's accepted, and moving on to
//! what to show next ([`Show`], for the app to bring into view).

use std::ops::Range as Lines;
use std::path::PathBuf;

use ghostwrite_ai::placement::{self, Asked, Files as _, Place};
use ghostwrite_ai::protocol::{FileOp, Step};
use ghostwrite_ai::{Interaction, Item, ItemId, Layout, Settled, Story, Verdict};
use ghostwrite_core::Range;
use ghostwrite_project::fs;
use ghostwrite_project::tree::Proposal;

use super::StoryState;
use crate::project::ProjectFiles;
use crate::{Autoscroll, DocId, Project};

/// Where an item is in the code: its tab (if its file is open), its file
/// (`None`: the one asked about), and its lines (or the lines to look near).
pub struct Located {
    pub tab: Option<usize>,
    pub path: Option<PathBuf>,
    pub near: Lines<usize>,
}

/// How a file in the tree is waiting on the story.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Waiting {
    /// The current item is here (or Claude's answer is coming here).
    Here,
    /// Later items are here.
    Later,
}

/// Why an item can't be shown as it is.
enum Problem {
    /// Its file isn't in the project: the path, and what to say about it.
    NoFile(String),
    /// Its file is only pages, which Claude doesn't comment on.
    OnlyPages(String),
    /// Its `<at>` lines aren't in its file.
    NotFound,
}

/// What to show after the story moved on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Show {
    /// Nothing new (still waiting, or already showing it).
    Nothing,
    /// A step's ready, and the user is looking elsewhere: it waits for Tab.
    Ready,
    /// Claude has been asked to correct its reply: send the request.
    Revise,
    /// This tab, where the current item is (its selection already set).
    Tab(usize),
    /// The file tree, where a proposal about a file is.
    Tree,
    /// It's over.
    Finished,
}

impl StoryState {
    /// Where the story's items show, in `project` as it is now.
    pub fn layout<'a>(&'a self, project: &'a Project) -> Option<Layout<'a, ProjectFiles<'a>>> {
        let asked = project.tabs.by_id(self.doc)?;
        Some(Layout {
            story: self.story.as_ref()?,
            asked: Asked { path: asked.doc.path.as_deref(), text: asked.doc.text(), lines: self.lines.clone() },
            files: project.files(),
            placer: &self.placer,
        })
    }

    /// Where item `id` is in the code.
    pub fn locate(&self, project: &Project, id: ItemId) -> Option<Located> {
        Some(self.in_tab(project, self.layout(project)?.spot(id)?))
    }

    /// The lines item `id` shows at, in the code.
    pub fn lines_of(&self, project: &Project, id: ItemId) -> Option<Located> {
        Some(self.in_tab(project, self.layout(project)?.lines(id)?))
    }

    /// Where Claude's answer is coming while there's nothing to review:
    /// under what was asked about, or the marked lines.
    pub fn waiting_spot(&self, project: &Project) -> Option<Place> {
        match self.layout(project) {
            Some(layout) => layout.waiting_spot(),
            None => Some(Place { path: None, near: self.lines.clone() }),
        }
    }

    pub fn in_tab(&self, project: &Project, place: Place) -> Located {
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
    pub fn files(&self, project: &Project) -> Vec<(PathBuf, Waiting)> {
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

    /// Whether item `id` is the one brought into view.
    pub fn showing(&self, id: ItemId) -> bool {
        self.shown == Some(id)
    }

    /// What the tree should show for item `id`: a new file made by an edit,
    /// or a file removed or renamed.
    pub fn proposal(&self, project: &Project, id: ItemId, item: &Item) -> Option<Proposal> {
        let path = self.locate(project, id)?.path;
        match item {
            Item::Step(Step { op: Some(FileOp::Remove), .. }) => path.map(Proposal::Remove),
            Item::Step(Step { op: Some(FileOp::Rename(to)), .. }) => path.map(|p| Proposal::Rename(p, to.trim_start_matches("./").to_string())),
            Item::Step(_) => path.filter(|p| !p.exists()).map(Proposal::Create),
            Item::Note(_) | Item::Run(_) => None,
        }
    }

    /// What's wrong with item `id`, if anything: a file that isn't there (or,
    /// for a new one, already is), or `<at>` lines that aren't in its file.
    fn problem(&self, project: &Project, id: ItemId, item: &Item) -> Option<Problem> {
        let located = self.locate(project, id)?;
        let exists = located.path.as_ref().is_none_or(|p| p.exists());
        let named = || located.path.as_deref().map_or("the file".into(), |p| project.display(p));
        let outside = |path: &std::path::Path| !ghostwrite_project::within(&project.root, path);
        match item {
            // Nothing outside the project: not by an absolute path, `..`, or the project itself.
            _ if located.path.as_deref().is_some_and(outside) => Some(Problem::NoFile(format!("{} (it's outside the project: name a file in it)", named()))),
            Item::Step(Step { op: Some(FileOp::Rename(to)), .. }) if outside(&project.root.join(to.trim_start_matches("./"))) => {
                Some(Problem::NoFile(format!("{to} (it's outside the project: rename to a path in it)")))
            }
            Item::Step(step) if step.creates() && exists => Some(Problem::NoFile(format!("{} (it exists already, so edit it with <file>)", named()))),
            Item::Step(step) if !step.creates() && !exists => Some(Problem::NoFile(named())),
            Item::Step(Step { op: Some(FileOp::Rename(to)), .. }) if project.root.join(to.trim_start_matches("./")).exists() => {
                Some(Problem::NoFile(format!("{to} (it exists already, so rename to a path that's free)")))
            }
            Item::Step(Step { op: Some(FileOp::Remove | FileOp::Rename(_)), .. }) => None,
            _ if located.path.as_ref().is_some_and(|p| ghostwrite_core::Language::for_path(p).is_some_and(|language| language.only_pages)) => {
                Some(Problem::OnlyPages(named()))
            }
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

    /// The command accepting the current item runs, if it's one: the app
    /// runs it in the terminal before it's [accepted](Self::accept).
    pub fn command_to_run(&self) -> Option<ghostwrite_ai::protocol::Run> {
        match self.story.as_ref()?.current_item()? {
            Item::Run(run) => Some(run),
            _ => None,
        }
    }

    /// Accept the current item, carrying it out (a command, the app runs).
    pub(super) fn accept_item(&mut self, project: &mut Project) -> Result<(), String> {
        let Some((id, item)) = self.story.as_ref().and_then(|s| Some((s.current()?, s.current_item()?))) else { return Ok(()) };
        self.apply(project, id, &item)?;
        if let Some(story) = self.story.as_mut() {
            story.accept();
        }
        Ok(())
    }

    fn apply(&self, project: &mut Project, id: ItemId, item: &Item) -> Result<(), String> {
        match item {
            Item::Note(_) | Item::Run(_) => Ok(()),
            Item::Step(step) => self.apply_step(project, id, step),
        }
    }

    fn apply_step(&self, project: &mut Project, id: ItemId, step: &Step) -> Result<(), String> {
        if !step.complete {
            return Err("that step is still being written".into());
        }
        // Checked before it was shown, and again: nothing outside the project is touched.
        let outside = |path: &std::path::Path| !ghostwrite_project::within(&project.root, path);
        if self.locate(project, id).and_then(|l| l.path).is_some_and(|p| outside(&p))
            || matches!(&step.op, Some(FileOp::Rename(to)) if outside(&project.root.join(to.trim_start_matches("./"))))
        {
            return Err("that's outside the project; ⏎ to say what you want instead".into());
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
        let start = edit.bytes.start;
        tab.doc.buffer.edit(edit.bytes, &edit.text);
        // What it changed lights up for a moment (a removal, the line it was taken from).
        let lines = tab.doc.buffer.line_of(start)..tab.doc.buffer.line_of(start + edit.text.len().saturating_sub(1)) + 1;
        tab.view.motion.flash = Some((tab.doc.buffer.lines_range(lines), std::time::Instant::now()));
        let end = tab.doc.buffer.line_end(edit.cursor_line);
        tab.select(Range::point(end), Autoscroll::Fit);
        tab.doc.buffer.commit(tab.view.selection);
        tab.sync();
        self.applied.borrow_mut().push(tab.doc.id);
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
    /// its proposal in the tree. Something to look at ends the wait for it,
    /// timed for `model`.
    pub fn settle(&mut self, project: &mut Project, model: &str) -> Show {
        let show = self.move_on(project);
        self.take_adopted();
        if matches!(show, Show::Tab(_) | Show::Tree | Show::Finished)
            && let Some(timer) = self.timer.take()
            && !matches!(show, Show::Finished)
        {
            timer.shown(model);
        }
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
        if story.is_repeated() && story.can_revise() {
            story.reply_repeated();
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
        let wandered = self.wandered;
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
                Some(Problem::OnlyPages(file)) => {
                    if let Some(story) = self.story.as_mut() {
                        story.about_only_pages(&file);
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
        // Looking elsewhere: it waits, and Tab brings them to it.
        if wandered {
            return Show::Ready;
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
    pub fn finish(&self, project: &mut Project) {
        let Some(story) = self.story.as_ref() else { return };
        let (kept, total) = story.summary();
        if total > 0 {
            project.say(format!("applied {kept} of {total} steps{}", self.used.said()));
        } else if !self.used.said().is_empty() {
            project.say(format!("done{}", self.used.said()));
        }
        if let Some(notes) = story.memory_update() {
            ghostwrite_ai::memory::save_notes(&project.root, &notes);
        }
        if !self.docs_note || !story.followups().is_empty() {
            let path = project.tabs.by_id(self.doc).and_then(|t| t.doc.path.clone());
            let file = path.map_or("untitled".into(), |p| project.display(&p));
            let said = story.followups().iter().filter(|f| !f.correction && !f.said.is_empty()).map(|f| f.said.clone()).collect();
            Interaction::new(file, story.instruction.clone(), said, story.outcome(), story.recap()).log(&project.root);
        }
    }
}
