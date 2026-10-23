//! `space i`: ask Claude, then review its reply as one list you Tab through:
//! edit steps to accept or skip, and notes (answers) to read. A question asked
//! mid-review comes back as notes inserted where you asked; a correction
//! replaces the plan. The last decision finishes.

use std::collections::HashMap;

use super::*;
use crate::assist::{Create, Note, Piece};

/// Marks a note as `space k` docs when opening one (it isn't sent anywhere).
const DOCS: &str = "\u{0}docs";

/// The last request and Claude's replies, kept for when a step won't place.
fn keep_transcript(assist: &Assist) {
    let Some(path) = crate::projects::support_file("last-reply.txt") else { return };
    let mut out = format!("request: {}\n", assist.instruction);
    for ((reply, feedback), n) in assist.history.iter().zip(1..) {
        out.push_str(&format!("\n--- reply {n}\n{reply}\n--- then: {feedback}\n"));
    }
    for source in &assist.sources {
        out.push_str(&format!("\n--- reply\n{}\n", source.reply));
    }
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(path, out);
}

/// What the background request reports.
enum Msg {
    Text(String),
    Look(String),
    Done,
    Failed(String),
}

/// A reply from Claude. The first is the plan; later ones are answers to
/// questions asked during review, inserted before plan item `at`.
pub(super) struct Source {
    pub(super) reply: String,
    pub(super) at: usize,
    /// Asked about a step: if Claude answers with steps, they're a corrected
    /// plan. Asked about a note: any steps are inserted, and the tour goes on.
    pub(super) replaces: bool,
    /// Asked in the file tree about this proposal: the answer shows there.
    pub(super) in_tree: Option<crate::tree::Proposal>,
}

/// An item to review: (source, index within that source).
pub(super) type ItemId = (usize, usize);

#[derive(Clone)]
pub(super) enum Item {
    Step(Step),
    Note(Note),
    /// A new file or folder, shown as a ghost row in the file tree.
    Create(Create),
    /// A file or folder to remove, marked in the file tree.
    Remove(Create),
    /// A file or folder to move, shown on its row in the file tree.
    Rename { from: String, to: String },
}

/// What a review key does, wherever it was pressed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReviewKey {
    Accept,
    Skip,
    Redirect,
    Stop,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Verdict {
    Pending,
    Accepted,
    Rejected,
}

pub(super) struct Assist {
    /// Command output this is about (an error from `:sh`).
    pub(super) context: Option<String>,
    pub(super) state: Entity<EditorState>,
    /// The lines pointed at (zero-based, end exclusive).
    pub(super) lines: std::ops::Range<usize>,
    pub(super) input: Entity<InputState>,
    /// The first request has been sent.
    pub(super) asked: bool,
    /// What the user first asked, and each later round: the reply then being
    /// reviewed, and what the user said about it.
    pub(super) instruction: String,
    pub(super) history: Vec<(String, String)>,
    /// The prompt is open again, to redirect or ask.
    pub(super) redirecting: bool,
    pub(super) sources: Vec<Source>,
    pub(super) verdicts: HashMap<ItemId, Verdict>,
    /// The item under review; `None` while waiting for Claude.
    pub(super) current: Option<ItemId>,
    /// The item the view was last brought to.
    pub(super) shown: Option<ItemId>,
    /// The source Claude is writing into.
    pub(super) writing: Option<usize>,
    pub(super) error: Option<String>,
    pub(super) cancel: Arc<AtomicBool>,
    /// Per buffer touched: strikes the lines a step removes, or marks the
    /// lines a note is about; and which buffers have a gap folded out.
    pub(super) marks: std::cell::RefCell<Vec<(Entity<EditorState>, TextDecorationCollection)>>,
    pub(super) gaps: std::cell::RefCell<Vec<Entity<EditorState>>>,
    /// What Claude is looking at in the project right now, for the hint.
    pub(super) looking: Option<String>,
    /// Where unnamed steps were found in the project, by their `old` lines.
    pub(super) found_in: std::cell::RefCell<HashMap<String, Option<PathBuf>>>,
    /// Leave the cursor where it is (docs from `space k`, which helix shows
    /// without moving anything).
    pub(super) keep_cursor: bool,
    /// What the file tree shows, if a creation or removal is current.
    pub(super) ghost: Option<crate::tree::Proposal>,
    /// Asked from the tree: the keyboard stays there until the reply shows.
    pub(super) asked_in_tree: bool,
    pub(super) _task: Option<Task<()>>,
    pub(super) _subscription: Subscription,
}

impl Assist {
    fn source_items(&self, k: usize) -> Vec<Item> {
        let reply = &self.sources[k].reply;
        match assist::reply_kind(reply) {
            Some(Kind::Question) => assist::parse_notes(reply).into_iter().map(Item::Note).collect(),
            Some(Kind::Edit) => assist::parse_pieces(reply)
                .into_iter()
                .map(|p| match p {
                    Piece::Step(s) => Item::Step(s),
                    Piece::Create(c) => Item::Create(c),
                    Piece::Remove(c) => Item::Remove(c),
                    Piece::Rename { from, to } => Item::Rename { from, to },
                })
                .collect(),
            None => vec![],
        }
    }

    /// Everything to review, in order: the plan, with answers inserted where
    /// they were asked.
    pub(super) fn items(&self) -> Vec<(ItemId, Item)> {
        if self.sources.is_empty() {
            return vec![];
        }
        let plan = self.source_items(0);
        let inserts: Vec<(usize, usize, Vec<Item>)> =
            (1..self.sources.len()).map(|k| (self.sources[k].at.min(plan.len()), k, self.source_items(k))).collect();
        let mut items = vec![];
        for i in 0..=plan.len() {
            for (at, k, notes) in &inserts {
                if *at == i {
                    items.extend(notes.iter().cloned().enumerate().map(|(j, item)| ((*k, j), item)));
                }
            }
            if let Some(item) = plan.get(i) {
                items.push(((0, i), item.clone()));
            }
        }
        items
    }

    fn verdict(&self, id: ItemId) -> Verdict {
        self.verdicts.get(&id).copied().unwrap_or(Verdict::Pending)
    }

    /// Where in the plan something asked now would go.
    fn plan_position(&self) -> usize {
        match self.current {
            Some((0, i)) => i,
            Some((k, _)) => self.sources[k].at,
            None => self.source_items(0).len(),
        }
    }
}

/// Where a step shows: what changes, and the line its ghost text follows.
fn step_view(text: &str, step: &Step, near: std::ops::Range<usize>) -> Option<(assist::Change, usize)> {
    let change = assist::change(assist::locate(text, &step.old, near)?, &step.old, &step.new);
    let after = if change.removed.is_empty() { change.removed.start.saturating_sub(1) } else { change.removed.end - 1 };
    Some((change, after))
}

/// The end of zero-based `line`, before its line break.
fn line_end_byte(text: &str, line: usize) -> usize {
    let range = line_bytes(text, line..line + 1);
    range.end - usize::from(text[range.clone()].ends_with('\n'))
}

/// Bytes of the whole lines `lines`, line breaks included.
pub(super) fn line_bytes(text: &str, lines: std::ops::Range<usize>) -> std::ops::Range<usize> {
    let starts: Vec<usize> = std::iter::once(0).chain(text.match_indices('\n').map(|(i, _)| i + 1)).collect();
    let at = |line: usize| starts.get(line).copied().unwrap_or(text.len());
    at(lines.start)..at(lines.end).max(at(lines.start))
}

/// A code line wrapped to `cols`: at a space where there is one, never
/// touching the spacing inside it; continuations indent one level deeper.
fn wrap_code(line: &str, cols: usize) -> Vec<String> {
    let chars: Vec<char> = line.chars().collect();
    if chars.len() <= cols {
        return vec![line.to_string()];
    }
    let indent = chars.iter().take_while(|c| **c == ' ').count();
    let more: String = " ".repeat(indent + 4);
    let mut rows = vec![];
    let mut rest: &[char] = &chars;
    let mut prefix = String::new();
    while prefix.chars().count() + rest.len() > cols {
        let room = cols.saturating_sub(prefix.chars().count()).max(8).min(rest.len());
        let lead = if rows.is_empty() { indent } else { 0 };
        let cut = rest[..room].iter().rposition(|c| *c == ' ').filter(|&i| i > lead).map_or(room, |i| i + 1);
        rows.push(format!("{prefix}{}", rest[..cut].iter().collect::<String>().trim_end()));
        rest = &rest[cut..];
        prefix.clone_from(&more);
    }
    rows.push(format!("{prefix}{}", rest.iter().collect::<String>()));
    rows
}

/// Lines in `text` (a trailing line break doesn't start one).
fn line_count(text: &str) -> usize {
    text.matches('\n').count() + usize::from(!text.is_empty() && !text.ends_with('\n'))
}

impl TextEditor {
    /// Where an item lives: its buffer (if open), its file (if not the one
    /// being reviewed), and the lines to look for it near.
    fn item_place(&self, assist: &Assist, item: &Item, cx: &App) -> (Option<Entity<EditorState>>, Option<PathBuf>, std::ops::Range<usize>) {
        match item {
            Item::Step(Step { file: Some(file), old, .. }) => {
                let path = self.root.join(file);
                let state = self.buffers.iter().find(|b| b.path.as_ref() == Some(&path)).map(|b| b.state.clone());
                // In another file an empty `old` appends.
                let near = match (&state, old.is_empty()) {
                    (Some(st), true) => {
                        let n = line_count(&st.read(cx).text().to_string());
                        n..n
                    }
                    _ => 0..1,
                };
                (state, Some(path), near)
            }
            // Not named: the file being reviewed, unless the lines are only
            // in one other project file (Claude read it, then forgot <file>).
            Item::Step(Step { file: None, old, .. }) if !old.is_empty() => {
                let here = assist.state.read(cx).text().to_string();
                if assist::locate(&here, old, assist.lines.clone()).is_some() {
                    return (Some(assist.state.clone()), None, assist.lines.clone());
                }
                let key = old.join("\n");
                let found = assist.found_in.borrow_mut().entry(key).or_insert_with(|| self.find_in_project(old, cx)).clone();
                match found {
                    Some(path) => {
                        let state = self.buffers.iter().find(|b| b.path.as_ref() == Some(&path)).map(|b| b.state.clone());
                        (state, Some(path), 0..1)
                    }
                    None => (Some(assist.state.clone()), None, assist.lines.clone()),
                }
            }
            _ => (Some(assist.state.clone()), None, assist.lines.clone()),
        }
    }

    /// The one project file containing `old`, if exactly one does.
    fn find_in_project(&self, old: &[String], cx: &App) -> Option<PathBuf> {
        let mut hits = crate::project::files(&self.root, &self.root).into_iter().map(|rel| self.root.join(rel)).filter(|path| {
            let open = self.buffers.iter().find(|b| b.path.as_ref() == Some(path)).map(|b| b.state.read(cx).text().to_string());
            let text = open.or_else(|| std::fs::read_to_string(path).ok());
            text.is_some_and(|t| assist::locate(&t, old, 0..1).is_some())
        });
        let first = hits.next()?;
        hits.next().is_none().then_some(first)
    }

    pub(super) fn open_assist(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.ai.is_none() {
            self.status = Some("asking Claude needs ANTHROPIC_API_KEY or Claude Code (`claude`) installed".into());
            return;
        }
        self.open_assist_with(None, "", window, cx);
    }

    /// Show `text` as a note under the cursor's line, reviewed like an answer
    /// (`space k` docs); Enter asks Claude about it.
    pub(super) fn show_note(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.open_assist_with(Some(DOCS.into()), "", window, cx);
        let Some(assist) = self.assist.as_mut() else { return };
        assist.keep_cursor = true;
        assist.asked = true;
        assist.instruction = "Explain what's under the cursor; its documentation was shown.".into();
        assist.sources = vec![Source { reply: format!("<answer>\n{text}\n</answer>"), at: 0, replaces: false, in_tree: None }];
        self.focus_active(window, cx);
        self.settle_review(window, cx);
        cx.notify();
    }

    /// Open the prompt, optionally about some command output and with text typed in.
    pub(super) fn open_assist_with(&mut self, context: Option<String>, typed: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(buffer) = self.buffers.get(self.active) else { return };
        let state = buffer.state.clone();
        let bytes = self.modal.assist_target(state.read(cx));
        let text = state.read(cx).text().to_string();
        let first = text[..bytes.start].matches('\n').count();
        let last = first + text[bytes.start..bytes.end.max(bytes.start)].trim_end_matches('\n').matches('\n').count();
        let lines = first..last + 1;
        let keep_cursor = context.as_deref() == Some(DOCS);
        let mark = state.update(cx, |st, cx| {
            if !keep_cursor {
                let bytes = line_bytes(&text, lines.clone());
                let end = bytes.end - usize::from(text[bytes.clone()].ends_with('\n'));
                st.set_selected_range(bytes.start..end.max(bytes.start), cx);
            }
            st.create_decorations_collection(vec![], cx)
        });
        let context = context.filter(|c| c != DOCS);
        let typed = typed.to_string();
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Ask Claude, or tell it what to change…").default_value(typed));
        let subscription = cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| {
            if let InputEvent::PressEnter { .. } = event {
                this.run_assist(window, cx);
            }
        });
        input.update(cx, |s, cx| s.focus(window, cx));
        let reviewed = state.clone();
        self.assist = Some(Assist {
            context,
            state,
            lines,
            input,
            asked: false,
            instruction: String::new(),
            history: vec![],
            redirecting: false,
            sources: vec![],
            verdicts: HashMap::new(),
            current: None,
            shown: None,
            writing: None,
            error: None,
            cancel: Arc::new(AtomicBool::new(false)),
            marks: std::cell::RefCell::new(vec![(reviewed, mark)]),
            gaps: std::cell::RefCell::new(vec![]),
            ghost: None,
            asked_in_tree: false,
            keep_cursor: false,
            looking: None,
            found_in: Default::default(),
            _task: None,
            _subscription: subscription,
        });
        cx.notify();
    }

    /// Enter in the prompt: the first ask, or (while reviewing) a question or
    /// a correction. Claude's reply decides which: notes get inserted here,
    /// steps replace the plan.
    fn run_assist(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(backend), Some(assist)) = (self.ai.clone(), self.assist.as_mut()) else { return };
        let typed = assist.input.read(cx).value().to_string();
        if typed.trim().is_empty() {
            return;
        }
        let target = if assist.redirecting {
            let about = assist.current.map_or(assist.sources.len() - 1, |(k, _)| k);
            assist.history.push((assist.sources[about].reply.clone(), typed));
            assist.cancel.store(true, Ordering::SeqCst);
            assist.cancel = Arc::new(AtomicBool::new(false));
            assist.redirecting = false;
            assist.error = None;
            // Asked about a file proposal: it goes until the reply says what's next.
            assist.shown = None;
            let in_tree = assist.ghost.take();
            if in_tree.is_some() {
                assist.asked_in_tree = true;
                cx.emit(EditorEvent::ProposeCreate(None));
                cx.emit(EditorEvent::AskInTree(None));
            }
            // From a note (a tour of an answer): read it, and insert whatever
            // comes back right after it. From a step: before it, or replacing the plan.
            let items = assist.items();
            let on_note = assist.current.and_then(|c| items.iter().find(|(id, _)| *id == c)).is_some_and(|(_, item)| matches!(item, Item::Note(_)));
            let at = match (on_note, assist.current) {
                (true, Some((0, i))) => i + 1,
                _ => assist.plan_position(),
            };
            if let (true, Some(c)) = (on_note, assist.current) {
                assist.verdicts.insert(c, Verdict::Accepted);
            }
            assist.sources.push(Source { reply: String::new(), at, replaces: !on_note, in_tree });
            assist.current = None;
            assist.sources.len() - 1
        } else {
            assist.instruction = typed;
            assist.sources = vec![Source { reply: String::new(), at: 0, replaces: false, in_tree: None }];
            0
        };
        assist.writing = Some(target);
        let buffer = self.buffers.iter().find(|b| b.state == assist.state);
        let text = assist.state.read(cx).text().to_string();
        let req = EditRequest {
            path: buffer.and_then(|b| b.path.clone()),
            language: buffer.map_or("text".into(), |b| language_for(b.path.as_deref()).to_string()),
            range: line_bytes(&text, assist.lines.clone()),
            text,
            instruction: assist.instruction.clone(),
            followups: assist.history.clone(),
            context: assist.context.clone(),
            root: Some(self.root.clone()),
        };
        assist.asked = true;
        let cancel = assist.cancel.clone();
        let (tx, mut rx) = futures::channel::mpsc::unbounded::<Msg>();
        // The request blocks while it streams, so it runs on a background thread.
        cx.background_executor()
            .spawn(async move {
                let (chunks, looks) = (tx.clone(), tx.clone());
                let result = assist::run_with(
                    &backend,
                    &req,
                    &mut |t| drop(chunks.unbounded_send(Msg::Text(t.to_string()))),
                    &mut |what| drop(looks.unbounded_send(Msg::Look(what))),
                    &cancel,
                );
                tx.unbounded_send(result.map_or_else(Msg::Failed, |()| Msg::Done)).ok();
            })
            .detach();
        assist._task = Some(cx.spawn_in(window, async move |this, cx| {
            while let Some(msg) = rx.next().await {
                let done = matches!(msg, Msg::Done | Msg::Failed(_));
                let alive = this
                    .update_in(cx, |this, window, cx| {
                        let Some(assist) = this.assist.as_mut() else { return false };
                        let Some(k) = assist.writing else { return false };
                        match msg {
                            Msg::Look(what) => assist.looking = Some(what),
                            Msg::Text(chunk) => {
                                let decided = assist::reply_kind(&assist.sources[k].reply).is_some();
                                assist.sources[k].reply.push_str(&chunk);
                                // Steps in answer to a correction are the new plan.
                                if k > 0 && !decided && assist.sources[k].replaces && assist::reply_kind(&assist.sources[k].reply) == Some(Kind::Edit) {
                                    let plan = assist.sources.remove(k);
                                    for j in 1..assist.sources.len() {
                                        for (i, _) in assist.source_items(j).iter().enumerate() {
                                            assist.verdicts.insert((j, i), Verdict::Accepted);
                                        }
                                    }
                                    assist.verdicts.retain(|(s, _), _| *s != 0);
                                    assist.sources[0] = Source { reply: plan.reply, at: 0, replaces: false, in_tree: None };
                                    assist.writing = Some(0);
                                    assist.current = None;
                                }
                            }
                            Msg::Done => {
                                assist.writing = None;
                                keep_transcript(assist);
                            }
                            Msg::Failed(e) => {
                                keep_transcript(assist);
                                assist.writing = None;
                                assist.error = Some(e);
                            }
                        }
                        this.settle_review(window, cx);
                        cx.notify();
                        true
                    })
                    .unwrap_or(false);
                if done || !alive {
                    break;
                }
            }
        }));
        // The keyboard goes back to where review keys work: the buffer, or the tree.
        if !self.assist.as_ref().is_some_and(|a| a.asked_in_tree) {
            self.focus_active(window, cx);
        }
        cx.notify();
    }

    /// Review keys: Tab forward (accepting a step), Backspace skips, Enter
    /// redirects or asks, Esc stops early. A story only goes forward, so
    /// Shift+Tab does nothing (it's swallowed rather than dedenting).
    pub(super) fn review_key(&mut self, keystroke: &Keystroke, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !self.assist.as_ref().is_some_and(|a| a.asked) {
            return false;
        }
        if keystroke.modifiers.platform || keystroke.modifiers.control || keystroke.modifiers.alt {
            return false;
        }
        let key = match keystroke.key.as_str() {
            "enter" if !keystroke.modifiers.shift => ReviewKey::Redirect,
            "tab" if keystroke.modifiers.shift => return true,
            "tab" => ReviewKey::Accept,
            "backspace" => ReviewKey::Skip,
            "escape" => ReviewKey::Stop,
            _ => return false,
        };
        self.review_command(key, window, cx);
        true
    }

    /// One review action, from the editor's keys or the file tree's (a
    /// proposed file is decided there).
    pub fn review_command(&mut self, key: ReviewKey, window: &mut Window, cx: &mut Context<Self>) {
        let Some(assist) = self.assist.as_mut().filter(|a| a.asked) else { return };
        match key {
            ReviewKey::Redirect => self.start_redirect(window, cx),
            ReviewKey::Accept => self.decide(Verdict::Accepted, window, cx),
            ReviewKey::Skip => self.decide(Verdict::Rejected, window, cx),
            ReviewKey::Stop if assist.redirecting => self.stop_redirect(window, cx),
            ReviewKey::Stop => self.finish_review(window, cx),
        }
        cx.notify();
    }

    /// Accept (apply a step, or mark a note read) or skip the current item.
    fn decide(&mut self, verdict: Verdict, window: &mut Window, cx: &mut Context<Self>) {
        let Some(assist) = self.assist.as_mut() else { return };
        let Some(id) = assist.current else { return };
        let Some((_, item)) = assist.items().into_iter().find(|(i, _)| *i == id) else { return };
        if let (Verdict::Accepted, Item::Create(create)) = (verdict, &item) {
            let path = self.root.join(create.path.trim_end_matches('/'));
            if !path.exists() {
                let made = crate::tree::create_entry(&self.root, &create.path);
                if let Err(e) = made {
                    self.status = Some(e.into());
                    return;
                }
            }
            cx.emit(EditorEvent::FilesChanged);
            if path.is_file() {
                self.open_paths(vec![path], window, cx);
            }
        }
        if let (Verdict::Accepted, Item::Rename { from, to }) = (verdict, &item) {
            let (from, to) = (self.root.join(from.trim_end_matches('/')), self.root.join(to.trim_end_matches('/')));
            if !to.starts_with(&self.root) || to.exists() {
                self.status = Some(format!("can't rename to {}: it exists or is outside the project", to.display()).into());
                return;
            }
            let moved = to.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|()| std::fs::rename(&from, &to));
            if let Err(e) = moved {
                self.status = Some(format!("couldn't rename {}: {e}", from.display()).into());
                return;
            }
            self.path_moved(&from, &to, cx);
            cx.emit(EditorEvent::FilesChanged);
        }
        if let (Verdict::Accepted, Item::Remove(target)) = (verdict, &item) {
            let path = self.root.join(target.path.trim_end_matches('/'));
            if path.exists() {
                if let Err(e) = crate::tree::move_to_trash(&path) {
                    self.status = Some(e.into());
                    return;
                }
                self.path_deleted(&path, window, cx);
            }
            cx.emit(EditorEvent::FilesChanged);
        }
        if let (Verdict::Accepted, Item::Step(step)) = (verdict, &item) {
            if !step.complete {
                return;
            }
            let (mut state, path, _) = self.item_place(self.assist.as_ref().expect("reviewing"), &item, cx);
            if state.is_none()
                && let Some(path) = path.filter(|p| p.is_file())
            {
                self.open_paths(vec![path], window, cx);
                state = self.item_place(self.assist.as_ref().expect("reviewing"), &item, cx).0;
            }
            let Some(state) = state else {
                self.status = Some("that step's file doesn't exist; backspace skips it".into());
                return;
            };
            let near = self.item_place(self.assist.as_ref().expect("reviewing"), &item, cx).2;
            let text = state.read(cx).text().to_string();
            let Some(at) = assist::locate(&text, &step.old, near) else {
                self.status = Some("couldn't find where that step goes; backspace skips it".into());
                return;
            };
            let bytes = line_bytes(&text, at.clone());
            let replaced = &text[bytes.clone()];
            let mut new = step.new.join("\n");
            if !step.new.is_empty() && (replaced.ends_with('\n') || replaced.is_empty()) {
                new.push('\n');
            }
            // Like accepting a completion: the cursor lands after what was written.
            let change = assist::change(at, &step.old, &step.new);
            let last = if change.added.is_empty() { change.removed.start.saturating_sub(1) } else { change.removed.start + change.added.len() - 1 };
            state.update(cx, |st, cx| {
                st.set_selected_range(bytes, cx);
                st.replace(new, window, cx);
                let end = line_end_byte(&st.text().to_string(), last);
                self.modal.select_bytes(end..end, st, cx);
            });
            // Changed now, not when the edit's event comes round: the story may
            // move to another file first, and leaving this one should write it.
            if let Some(buffer) = self.buffers.iter_mut().find(|b| b.state == state) {
                buffer.dirty = true;
            }
        }
        let Some(assist) = self.assist.as_mut() else { return };
        assist.verdicts.insert(id, verdict);
        assist.current = None;
        self.settle_review(window, cx);
    }

    /// After anything changes: pick the next item to review, finish when
    /// nothing is left, and bring a newly current item into view.
    fn settle_review(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(assist) = self.assist.as_mut().filter(|a| a.asked) else { return };
        let items = assist.items();
        let pending = |id: ItemId| assist.verdict(id) == Verdict::Pending;
        if assist.current.is_none_or(|c| !items.iter().any(|(id, _)| *id == c) || !pending(c)) {
            assist.current = items.iter().map(|(id, _)| *id).find(|&id| pending(id));
        }
        if assist.current.is_none() && assist.writing.is_none() && assist.error.is_none() {
            return self.finish_review(window, cx);
        }
        let (Some(id), true) = (assist.current, assist.current != assist.shown) else { return };
        let Some((_, item)) = items.into_iter().find(|(i, _)| *i == id) else { return };
        // A creation or removal is decided in the file tree; nothing moves in the editor.
        let ghost = match &item {
            Item::Create(c) => Some(crate::tree::Proposal::Create(self.root.join(&c.path))),
            Item::Remove(c) => Some(crate::tree::Proposal::Remove(self.root.join(c.path.trim_end_matches('/')))),
            Item::Rename { from, to } => Some(crate::tree::Proposal::Rename(self.root.join(from.trim_end_matches('/')), to.clone())),
            // An answer to a question asked in the tree stays there, under what it's about.
            Item::Note(_) => assist.sources[id.0].in_tree.clone(),
            _ => None,
        };
        let tree_note = match &item {
            Item::Note(note) if ghost.is_some() => Some(note.text.clone()),
            _ => None,
        };
        let leaving = (assist.ghost.is_some() || std::mem::take(&mut assist.asked_in_tree)) && ghost.is_none();
        if assist.ghost != ghost {
            assist.ghost = ghost.clone();
            cx.emit(EditorEvent::ProposeCreate(ghost));
        }
        cx.emit(EditorEvent::TreeNote(tree_note.clone()));
        if leaving {
            self.focus_active(window, cx);
        }
        if tree_note.is_some() || matches!(item, Item::Create(_) | Item::Remove(_) | Item::Rename { .. }) {
            if let Some(assist) = self.assist.as_mut() {
                assist.shown = Some(id);
            }
            return cx.notify();
        }
        // Into the item's file: open it, or go back to the one being reviewed.
        let (mut state, path, _) = self.item_place(self.assist.as_ref().expect("reviewing"), &item, cx);
        if state.is_none()
            && let Some(path) = path.filter(|p| p.is_file())
        {
            self.open_paths(vec![path], window, cx);
            state = self.item_place(self.assist.as_ref().expect("reviewing"), &item, cx).0;
        }
        let Some(state) = state else { return };
        if let Some(ix) = self.buffers.iter().position(|b| b.state == state)
            && ix != self.active
        {
            self.activate(ix, window, cx);
        }
        let near = self.item_place(self.assist.as_ref().expect("reviewing"), &item, cx).2;
        let text = state.read(cx).text().to_string();
        // Center the change (or the lines a note is about) and put the cursor there.
        let (first, after, extra, cursor) = match &item {
            Item::Step(step) => {
                let Some((change, after)) = step_view(&text, step, near) else { return };
                let first = if change.removed.is_empty() { after } else { change.removed.start };
                (first, after, change.added.len().max(1), line_end_byte(&text, after))
            }
            Item::Note(note) => {
                let Some(at) = assist::locate(&text, &note.at, near) else { return };
                (at.start, at.end - 1, 4, line_bytes(&text, at).start)
            }
            Item::Create(_) | Item::Remove(_) | Item::Rename { .. } => return,
        };
        let keep_cursor = self.assist.as_ref().is_some_and(|a| a.keep_cursor) && matches!(item, Item::Note(_));
        if let Some(assist) = self.assist.as_mut() {
            assist.shown = Some(id);
        }
        if keep_cursor {
            return cx.notify();
        }
        state.update(cx, |st, cx| {
            self.modal.select_bytes(cursor..cursor, st, cx);
            let line_height = st.line_height().unwrap_or(px(20.));
            st.center_on_lines(first..after + 1, line_height * extra as f32, cx);
        });
    }

    /// Close the review, saying what was applied.
    fn finish_review(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(assist) = self.assist.as_ref() else { return };
        let steps: Vec<ItemId> = assist.items().into_iter().filter(|(_, i)| !matches!(i, Item::Note(_))).map(|(id, _)| id).collect();
        let kept = steps.iter().filter(|id| assist.verdict(**id) == Verdict::Accepted).count();
        let total = steps.len();
        let asked = assist.asked;
        self.close_assist(window, cx);
        if asked && total > 0 {
            self.status = Some(format!("applied {kept} of {total} steps").into());
        }
    }

    fn close_assist(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(assist) = self.assist.take() {
            assist.cancel.store(true, Ordering::SeqCst);
            for (_, mark) in assist.marks.borrow().iter() {
                mark.clear(cx);
            }
            for state in assist.gaps.borrow().iter() {
                state.update(cx, |st, cx| st.set_block_gap(None, cx));
            }
            if assist.ghost.is_some() {
                cx.emit(EditorEvent::ProposeCreate(None));
            }
        }
        self.focus_active(window, cx);
        // Fixing an error from `:sh`: on to the next one.
        self.resume_shell(window, cx);
        cx.notify();
    }

    /// Esc in the prompt: close it; while reviewing, just go back to the review.
    fn cancel_assist(&mut self, _: &CancelAssist, window: &mut Window, cx: &mut Context<Self>) {
        match self.assist.as_ref() {
            Some(assist) if assist.redirecting => self.stop_redirect(window, cx),
            _ => self.close_assist(window, cx),
        }
    }

    /// Back from the prompt to the review: the editor, or the tree for a file.
    fn stop_redirect(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(assist) = self.assist.as_mut() else { return };
        assist.redirecting = false;
        if assist.ghost.is_some() {
            cx.emit(EditorEvent::AskInTree(None));
        } else {
            self.focus_active(window, cx);
        }
        cx.notify();
    }

    /// Enter while reviewing: open the prompt to ask or steer.
    fn start_redirect(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(assist) = self.assist.as_mut() else { return };
        assist.redirecting = true;
        assist.input.update(cx, |s, cx| {
            s.set_value("", window, cx);
            s.focus(window, cx);
        });
        // A file proposal is asked about where it's shown: in the tree.
        if assist.ghost.is_some() {
            cx.emit(EditorEvent::AskInTree(Some(assist.input.clone())));
        }
        cx.notify();
    }

    /// The prompt, or the current item folded out under its lines: a step's
    /// removed lines struck through and new lines as ghost text, or a note
    /// under the lines it's about, with its keys on a faint line below.
    pub(super) fn render_assist(&self, buffer: &Entity<EditorState>, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let assist = self.assist.as_ref()?;
        let theme = cx.theme();
        let (mono, size, muted, danger, warning, info) =
            (theme.mono_font_family.clone(), theme.mono_font_size, theme.muted_foreground, theme.danger, theme.warning, theme.info);
        let items = if assist.asked { assist.items() } else { vec![] };
        let here = assist.current.and_then(|c| items.iter().position(|(id, _)| *id == c));
        let item = here.map(|i| items[i].1.clone());
        let accent = if matches!(item, Some(Item::Note(_))) { info } else { warning };
        // A file proposal, or an answer asked about one, shows in the tree:
        // nothing folds out in the editor.
        let in_tree = match &item {
            Some(Item::Create(_) | Item::Remove(_) | Item::Rename { .. }) => true,
            Some(Item::Note(_)) => here.is_some_and(|i| assist.sources[items[i].0.0].in_tree.is_some()),
            _ => false,
        };
        if in_tree {
            for state in assist.gaps.borrow_mut().drain(..) {
                state.update(cx, |st, cx| st.set_block_gap(None, cx));
            }
            for (_, mark) in assist.marks.borrow().iter() {
                mark.clear(cx);
            }
            return None;
        }

        // The buffer this shows in: a step's own file, else the one being reviewed.
        let (target, near) = match &item {
            Some(i @ Item::Step(_)) => {
                let (state, _, near) = self.item_place(assist, i, cx);
                (state, near)
            }
            _ => (Some(assist.state.clone()), assist.lines.clone()),
        };
        // Fold the space and marks away in any other buffer.
        for state in assist.gaps.borrow().iter().filter(|s| Some(*s) != target.as_ref()) {
            state.update(cx, |st, cx| st.set_block_gap(None, cx));
        }
        assist.gaps.borrow_mut().retain(|s| Some(s) == target.as_ref());
        for (_, mark) in assist.marks.borrow().iter().filter(|(s, _)| Some(s) != target.as_ref()) {
            mark.clear(cx);
        }
        let target = target?;
        if &target != buffer {
            return None;
        }
        let in_reviewed = target == assist.state;
        let st = target.read(cx);
        let text = st.text().to_string();
        let line_height = st.range_to_bounds(&(0..0)).map_or(size * 1.5, |b| b.size.height);
        let last_line = assist.lines.end.saturating_sub(1);

        // What to show, and under which line.
        let (gap_line, rows, marked, placed) = match &item {
            Some(Item::Step(step)) => match step_view(&text, step, near.clone()) {
                Some((change, after)) => (after, change.added.clone(), (!change.removed.is_empty()).then(|| line_bytes(&text, change.removed.clone())), true),
                None => (near.end.saturating_sub(1), vec![], None, false),
            },
            Some(Item::Note(note)) => {
                let font_id = window.text_system().resolve_font(&font(mono.clone()));
                let char_width = window.text_system().advance(font_id, size, 'm').map_or(size * 0.6, |a| a.width);
                let at = assist::locate(&text, &note.at, near.clone()).unwrap_or(near.clone());
                // Wrap to what's left right of the line's indentation.
                let indent = {
                    let line = &text[line_bytes(&text, at.end - 1..at.end)];
                    line.len() - line.trim_start_matches([' ', '\t']).len()
                };
                let width = self.area.get().map_or(px(600.), |a| a.size.width) - px(160.) - char_width * indent as f32;
                let rows = assist::wrap(&note.text, (f32::from(width) / f32::from(char_width)) as usize);
                (at.end - 1, rows, Some(line_bytes(&text, at)), true)
            }
            None => (last_line, vec![], None, true),
            Some(Item::Create(_) | Item::Remove(_) | Item::Rename { .. }) => return None,
        };
        // Long suggested lines wrap at the editor's edge, like soft-wrapped code.
        let rows = match &item {
            Some(Item::Step(_)) => {
                let font_id = window.text_system().resolve_font(&font(mono.clone()));
                let char_width = window.text_system().advance(font_id, size, 'm').map_or(size * 0.6, |a| a.width);
                let line_start = line_bytes(&text, gap_line..gap_line + 1).start;
                let text_x = st.range_to_bounds(&(line_start..line_start)).map(|b| b.origin.x);
                let cols = self.area.get().zip(text_x).map_or(120, |(a, x)| (f32::from(a.right() - x - px(200.)) / f32::from(char_width)).max(24.) as usize);
                rows.iter().flat_map(|l| wrap_code(l, cols)).collect()
            }
            _ => rows,
        };
        let prompting = !assist.asked || assist.redirecting;
        // The keys get a line of their own under what they're about.
        let keys_line = !rows.is_empty() && !assist.redirecting;
        let rows_shown = if !assist.asked { 1 } else { rows.len().max(1) + usize::from(assist.redirecting) + usize::from(keys_line) };
        // A step's keys line up with its code.
        let keys_indent = match (&item, rows.first()) {
            (Some(Item::Step(_)), Some(first)) => {
                let font_id = window.text_system().resolve_font(&font(mono.clone()));
                let char_width = window.text_system().advance(font_id, size, 'm').map_or(size * 0.6, |a| a.width);
                let lead = first.replace('\t', "    ");
                char_width * (lead.len() - lead.trim_start().len()) as f32
            }
            _ => px(0.),
        };
        let height = line_height * rows_shown as f32;

        let progress = match here {
            Some(i) => format!("{}/{}{}", i + 1, items.len(), if assist.writing.is_some() { "+" } else { "" }),
            None => String::new(),
        };
        let hint = match (&item, &assist.error) {
            _ if !assist.asked => "⏎ ask · esc".to_string(),
            _ if assist.redirecting => "⏎ send · esc".to_string(),
            (_, Some(e)) => format!("{e} · esc"),
            (None, None) => assist.looking.as_ref().filter(|_| assist.writing.is_some()).map_or("…".to_string(), |what| format!("{what}…")),
            (Some(Item::Step(_)), None) if !placed => "can't place this one · ⌫ skip".to_string(),
            (Some(Item::Step(s)), None) if !s.complete => format!("…  {progress}"),
            (Some(Item::Step(_)), None) => format!("⇥ accept · ⌫ skip · ⏎ redirect  {progress}"),
            (Some(Item::Create(_)), None) => format!("⇥ create · ⌫ skip · ⏎ redirect  {progress}"),
            (Some(Item::Remove(_)), None) => format!("⇥ remove · ⌫ keep · ⏎ redirect  {progress}"),
            (Some(Item::Rename { .. }), None) => format!("⇥ rename · ⌫ keep · ⏎ redirect  {progress}"),
            (Some(Item::Note(_)), None) => format!("⇥ next · ⏎ ask  {progress}"),
        };

        // Steps strike the lines they remove; notes softly mark the lines they're about.
        let style = match &item {
            Some(Item::Note(_)) => HighlightStyle { background_color: Some(info.opacity(0.12)), ..Default::default() },
            _ => HighlightStyle {
                color: Some(muted),
                background_color: Some(danger.opacity(0.14)),
                strikethrough: Some(StrikethroughStyle { thickness: px(1.), color: Some(danger) }),
                ..Default::default()
            },
        };
        let existing = assist.marks.borrow().iter().find(|(s, _)| *s == target).map(|(_, m)| m.clone());
        let mark = match existing {
            Some(mark) => mark,
            None => {
                let mark = target.update(cx, |st, cx| st.create_decorations_collection(vec![], cx));
                assist.marks.borrow_mut().push((target.clone(), mark.clone()));
                mark
            }
        };
        mark.set(marked.map(|r| vec![TextDecoration::new(r, style)]).unwrap_or_default(), cx);
        target.update(cx, |st, cx| st.set_block_gap(Some((gap_line, height)), cx));
        if !assist.gaps.borrow().contains(&target) {
            assist.gaps.borrow_mut().push(target.clone());
        }

        // Place it from the editor's own line positions: under the gap line,
        // ghost lines at the text column, the prompt at the line's indentation.
        let st = target.read(cx);
        let line_start = line_bytes(&text, gap_line..gap_line + 1).start;
        let row = st.range_to_bounds(&(line_start..line_start))?;
        let end = line_end_byte(&text, gap_line);
        let bottom = st.range_to_bounds(&(end..end))?.bottom();
        let indent_x = if in_reviewed {
            let first = line_bytes(&text, assist.lines.start..assist.lines.start + 1);
            let indent = text[first.clone()].len() - text[first.clone()].trim_start_matches([' ', '\t']).len();
            st.range_to_bounds(&(first.start + indent..first.start + indent)).map_or(row.origin.x, |b| b.origin.x)
        } else {
            row.origin.x
        };
        // Prose (notes, the waiting line, a creation) starts at the indentation
        // of the line it hangs from; code (a step's lines) carries its own.
        let gap_first = line_start + text[line_start..end].len() - text[line_start..end].trim_start_matches([' ', '\t']).len();
        let gap_indent_x = st.range_to_bounds(&(gap_first..gap_first)).map_or(row.origin.x, |b| b.origin.x);
        let area = self.area.get()?;
        let left = match &item {
            _ if !assist.asked => indent_x,
            Some(Item::Step(_)) => row.origin.x,
            _ => gap_indent_x,
        } - area.origin.x;
        let prompt_indent = if assist.redirecting { (indent_x - row.origin.x).max(px(0.)) } else { px(0.) };

        let hint_el = |hint: String| div().flex_none().ml_3().text_xs().text_color(muted).opacity(0.7).child(hint);
        let ghost_row = |line: String| {
            div()
                .h(line_height)
                .flex()
                .items_center()
                .whitespace_nowrap()
                .font_family(mono.clone())
                .text_size(size)
                .text_color(muted)
                .child(if line.is_empty() { " ".to_string() } else { line.replace('\t', "    ") })
        };
        let mut body: Vec<AnyElement> = vec![];
        if assist.asked {
            if rows.is_empty() {
                if !assist.redirecting {
                    body.push(div().h(line_height).flex().items_center().child(div().text_color(accent).text_xs().child("✦")).child(hint_el(hint.clone())).into_any_element());
                }
            } else {
                body.extend(rows.into_iter().map(|line| ghost_row(line).into_any_element()));
                if keys_line {
                    body.push(
                        div().h(line_height).flex().items_center().pl(keys_indent).text_xs().text_color(muted).opacity(0.7).child(hint.clone()).into_any_element(),
                    );
                }
            }
        }
        if prompting {
            body.push(
                div()
                    .key_context("Assist")
                    .on_action(cx.listener(Self::cancel_assist))
                    .h(line_height)
                    .pl(prompt_indent)
                    .flex()
                    .items_center()
                    .gap_1()
                    .font_family(mono.clone())
                    .text_size(size)
                    .child(div().flex_none().text_color(accent).child("✦"))
                    .child(
                        Input::new(&assist.input)
                            .appearance(false)
                            .px_0()
                            .py_0()
                            .h(line_height)
                            .flex_1()
                            .font_family(mono.clone())
                            .text_size(size),
                    )
                    .child(hint_el(hint))
                    .into_any_element(),
            );
        }
        Some(div().absolute().left(left).top(bottom - area.origin.y).right(px(16.)).h(height).flex().flex_col().children(body).into_any_element())
    }
}

#[cfg(test)]
mod tests {
    use super::wrap_code;

    #[test]
    fn code_wraps_at_spaces_and_indents_continuations() {
        assert_eq!(wrap_code("    short", 40), ["    short"]);
        let rows = wrap_code("    result = compute(alpha, beta, gamma, delta)", 28);
        assert_eq!(rows, ["    result = compute(alpha,", "        beta, gamma, delta)"]);
        assert!(rows.iter().all(|r| r.chars().count() <= 28));
        // No spaces: a hard break.
        assert_eq!(wrap_code("abcdefghijklmnopqrstuvwxyz0123456789", 30).len(), 2);
    }
}
