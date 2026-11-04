//! What Claude is asked, and running it. A request is the story so far,
//! the file asked about and the others it's in, and what's around it in the
//! editor; the terminal is told as it was when the story started, and
//! again in a follow-up when it's changed. The
//! reply streams in on a background thread. While an item is reviewed, the
//! next one is asked for ahead, assuming this one's accepted, in a fork of
//! the Claude Code session.

use std::collections::HashMap;
use std::ops::Range as Lines;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use futures::StreamExt as _;
use ghostwrite_ai::backend::Listener;
use ghostwrite_ai::context::Shown;
use ghostwrite_ai::placement::{Asked, Assumed, Files as _, Overlay, Place};
use ghostwrite_ai::timing::{Timer, Wait};
use ghostwrite_ai::{Item, Request, Story, Surroundings, lines};
use gpui::{AppContext as _, Context, Task};

use super::StoryState;
use crate::editor::{Editor, Project};

/// How the story talks with Claude: its session, and the requests on their way.
#[derive(Default)]
pub(super) struct Conversation {
    /// The Claude Code session the story is in, resumed for each follow-up.
    session: Option<String>,
    /// The terminal, as Claude is told it.
    terminal: Told,
    /// Stops the reply on its way.
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

/// A story that's dropped (its window closed) stops what it asked: nothing
/// more is spent on replies no one will read.
impl Drop for Conversation {
    fn drop(&mut self) {
        self.stop();
    }
}

impl Conversation {
    /// Stop the reply on its way (something else was said).
    pub(super) fn stop_reply(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }

    /// Stop everything on its way: the story's over.
    pub(super) fn stop(&self) {
        self.stop_reply();
        if let Some((_, cancel)) = &self.prefetching {
            cancel.store(true, Ordering::SeqCst);
        }
    }
}

/// Something about the editor Claude is told as it was when the story
/// started, and again in the newest turn whenever it's changed since.
#[derive(Default)]
struct Told {
    /// What the first request said: `None` until it's sent. Kept, so the
    /// conversation's start stays the same.
    first: Option<Option<String>>,
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

/// What a running request reports, on its way to the editor.
enum Message {
    Text(String),
    Looking(String),
    Session(String),
    Used(u64, u64),
    /// The model's process is up: started ahead of time, or just now.
    Started(bool),
    /// The model would read this, outside the project: the user's asked, and the answer goes back.
    MayRead(PathBuf, std::sync::mpsc::Sender<bool>),
    Done,
    Failed(String),
}

struct Forward(futures::channel::mpsc::UnboundedSender<Message>, Readable);

/// A request sent ahead of time: what it reports goes to the story's replies asked ahead.
struct Ahead(futures::channel::mpsc::UnboundedSender<Message>, Readable);

/// What the user has let the model read outside the project, during this story.
pub(super) type Readable = Arc<std::sync::Mutex<Vec<PathBuf>>>;

fn allowed(readable: &Readable, path: &std::path::Path) -> bool {
    readable.lock().is_ok_and(|ok| ok.iter().any(|ok| path.starts_with(ok)))
}

impl Listener for Ahead {
    /// Only what's been allowed already: the user isn't asked about a step they aren't at.
    fn may_read(&mut self, path: &std::path::Path) -> bool {
        allowed(&self.1, path)
    }

    fn used(&mut self, input: u64, output: u64) {
        self.0.unbounded_send(Message::Used(input, output)).ok();
    }

    fn text(&mut self, chunk: &str) {
        self.0.unbounded_send(Message::Text(chunk.to_string())).ok();
    }

    fn looking(&mut self, _what: String) {}

    fn session(&mut self, id: String) {
        self.0.unbounded_send(Message::Session(id)).ok();
    }
}

impl Listener for Forward {
    /// Asked in the corner, and waited for; no answer (the story's over) is no.
    fn may_read(&mut self, path: &std::path::Path) -> bool {
        if allowed(&self.1, path) {
            return true;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        if self.0.unbounded_send(Message::MayRead(path.to_path_buf(), tx)).is_err() {
            return false;
        }
        let yes = rx.recv().unwrap_or(false);
        if yes && let Ok(mut ok) = self.1.lock() {
            ok.push(path.to_path_buf());
        }
        yes
    }

    fn used(&mut self, input: u64, output: u64) {
        self.0.unbounded_send(Message::Used(input, output)).ok();
    }

    fn text(&mut self, chunk: &str) {
        self.0.unbounded_send(Message::Text(chunk.to_string())).ok();
    }

    fn looking(&mut self, what: String) {
        self.0.unbounded_send(Message::Looking(what)).ok();
    }

    fn session(&mut self, id: String) {
        self.0.unbounded_send(Message::Session(id)).ok();
    }

    fn started(&mut self, ahead: bool) {
        self.0.unbounded_send(Message::Started(ahead)).ok();
    }
}

impl StoryState {
    /// The Claude Code session a follow-up would resume.
    pub(in crate::editor) fn conversation_session(&self) -> Option<&str> {
        self.conversation.session.as_deref()
    }

    /// The request for the story so far.
    pub(in crate::editor) fn request(&self, project: &Project) -> Option<Request> {
        self.request_ahead(project, &[])
    }

    /// The request as it would be with `accepted` applied too, as a request
    /// sent ahead assumes: the files it shows are as they'd be then.
    pub(in crate::editor) fn request_ahead(&self, project: &Project, accepted: &[Item]) -> Option<Request> {
        let story = self.story.as_ref()?;
        let doc = &project.tabs.by_id(self.doc)?.doc;
        let assumed = self.assume(project, accepted)?;
        Some(Request {
            path: doc.path.clone(),
            language: doc.language_name(),
            // In the text as it's assumed, not as it is: they can be different lengths.
            range: ghostwrite_ai::lines::bytes_of(&assumed.asked, self.lines.clone()),
            others: self.others(project, &assumed),
            text: assumed.asked,
            instruction: story.instruction.clone(),
            followups: story.followups().to_vec(),
            root: Some(project.root.clone()),
            surroundings: Surroundings { terminal: self.conversation.terminal.started(), ..self.surroundings(project) },
            session: self.conversation.session.clone(),
            fork: false,
            small: 0,
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
        Surroundings { open_files, diagnostics, terminal: None }
    }

    fn receive(&mut self, message: Message) {
        if let Message::Session(id) = message {
            self.conversation.session = Some(id);
            return;
        }
        if let Message::Used(input, output) = message {
            self.used.add(input, output);
            return;
        }
        if let Some(timer) = self.timer.as_mut() {
            match message {
                Message::Started(ahead) => timer.started(ahead),
                Message::Text(_) => timer.text(),
                _ => {}
            }
        }
        if let Message::MayRead(path, answer) = message {
            self.asking_to_read = Some((path, answer));
            return;
        }
        let Some(story) = self.story.as_mut() else { return };
        match message {
            Message::Session(_) | Message::Used(..) | Message::Started(_) | Message::MayRead(..) => {}
            Message::Text(chunk) => story.push_text(&chunk),
            Message::Looking(what) => story.looking = Some(what),
            Message::Done => story.finished_writing(),
            Message::Failed(e) => story.failed(e),
        }
        if !story.is_writing() {
            ghostwrite_ai::memory::save_transcript(&story.transcript());
        }
    }

    /// What a request sent ahead (`id`) reports.
    fn receive_ahead(&mut self, id: u64, message: Message) {
        let Some(story) = self.story.as_mut() else { return };
        if let Message::Used(input, output) = message {
            self.used.add(input, output);
            return;
        }
        match message {
            Message::Session(session) => {
                if self.conversation.session_from == Some(id) {
                    self.conversation.session = Some(session.clone());
                }
                self.conversation.ahead_sessions.insert(id, session);
            }
            Message::Text(chunk) => story.ahead_text(id, &chunk),
            Message::Looking(_) | Message::Used(..) | Message::Started(_) | Message::MayRead(..) => {}
            Message::Done => story.ahead_finished(id, None),
            Message::Failed(e) => story.ahead_finished(id, Some(e)),
        }
        if matches!(self.conversation.prefetching, Some((on_way, _)) if on_way == id) && !story.wants(id) {
            self.conversation.prefetching = None;
        }
    }

    /// A reply sent ahead became the next one: its session is the story's now.
    pub(super) fn take_adopted(&mut self) {
        let Some(id) = self.story.as_mut().and_then(Story::take_adopted) else { return };
        match self.conversation.ahead_sessions.get(&id) {
            Some(session) => self.conversation.session = Some(session.clone()),
            None => self.conversation.session_from = Some(id),
        }
    }

    /// Place a reply asked ahead (`id`) once, in the files as it assumed them,
    /// and keep where as its anchor.
    pub(super) fn anchor_ahead(&mut self, project: &Project, id: u64) {
        let Some(item @ Item::Step(_)) = self.story.as_ref().and_then(|s| s.ahead_item(id)) else { return };
        let Some(asked) = project.tabs.by_id(self.doc) else { return };
        let accepted = self.conversation.ahead_accepted.get(&id).map(Vec::as_slice).unwrap_or_default();
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
}

impl Editor {
    /// Send the story's request on a background thread; the reply streams in.
    pub(super) fn send_request(&mut self, cx: &mut Context<Self>) {
        // Claude reads the project from disk: let it see what's on screen.
        self.project.write_all();
        let Some(backend) = self.backend.clone() else { return };
        let terminal = self.terminal_context();
        let Some(state) = self.story.as_mut() else { return };
        // The first request says what's in the terminal; a follow-up, what it shows now.
        let seen = state.conversation.terminal.tell(terminal);
        if let (Some(terminal), Some(story)) = (seen, state.story.as_mut()) {
            story.saw_terminal(terminal);
        }
        let Some(req) = state.request(&self.project) else { return };
        // Timed until there's something to look at; a rewrite of a step that wouldn't place is part of the same wait.
        if state.timer.is_none() {
            state.timer = Some(Timer::start(if req.followups.is_empty() { Wait::Ask } else { Wait::FollowUp }));
        }
        // Whatever was still coming is superseded, and what it asked goes unanswered (no).
        state.asking_to_read = None;
        state.conversation.cancel.store(true, Ordering::SeqCst);
        state.conversation.cancel = Arc::new(AtomicBool::new(false));
        let cancel = state.conversation.cancel.clone();
        let mine = cancel.clone();
        let readable = state.readable.clone();
        let (tx, mut rx) = futures::channel::mpsc::unbounded();
        cx.background_spawn(async move {
            let result =
                ghostwrite_project::contained("asking the model", || backend.run(&req, &mut Forward(tx.clone(), readable), &cancel)).and_then(|result| result);
            tx.unbounded_send(result.map_or_else(Message::Failed, |()| Message::Done)).ok();
        })
        .detach();
        state.conversation._task = Some(cx.spawn(async move |this, cx| {
            while let Some(message) = rx.next().await {
                let done = matches!(message, Message::Done | Message::Failed(_));
                let alive = this
                    .update(cx, |this, cx| {
                        // A newer request (a correction sent mid-reply) supersedes this one.
                        let Some(state) = this.story.as_mut().filter(|s| Arc::ptr_eq(&s.conversation.cancel, &mine)) else { return false };
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

    /// While an item is reviewed, ask ahead for the next one (and, as each
    /// arrives, the one after), assuming each is accepted. One that's no
    /// longer wanted is stopped.
    pub(super) fn prefetch(&mut self, cx: &mut Context<Self>) {
        let Some(backend) = self.backend.clone() else { return };
        let Some(state) = self.story.as_mut() else { return };
        let Some(story) = state.story.as_mut() else { return };
        if let Some((id, cancel)) = &state.conversation.prefetching {
            if story.wants(*id) {
                return;
            }
            cancel.store(true, Ordering::SeqCst);
            state.conversation.prefetching = None;
        }
        let Some(ahead) = story.prefetch(backend.asks_ahead()) else { return };
        let Some(mut req) = state.request_ahead(&self.project, &ahead.accepted) else { return };
        req.followups = ahead.followups;
        req.session = match ahead.after {
            None => state.conversation.session.clone(),
            Some(before) => state.conversation.ahead_sessions.get(&before).cloned(),
        };
        req.fork = req.session.is_some();
        state.conversation.ahead_accepted.insert(ahead.id, ahead.accepted);
        let (id, cancel) = (ahead.id, Arc::new(AtomicBool::new(false)));
        let readable = state.readable.clone();
        state.conversation.prefetching = Some((id, cancel.clone()));
        let (tx, mut rx) = futures::channel::mpsc::unbounded();
        cx.background_spawn(async move {
            let result =
                ghostwrite_project::contained("asking ahead", || backend.run(&req, &mut Ahead(tx.clone(), readable), &cancel)).and_then(|result| result);
            tx.unbounded_send(result.map_or_else(Message::Failed, |()| Message::Done)).ok();
        })
        .detach();
        state.conversation._prefetch_task = Some(cx.spawn(async move |this, cx| {
            while let Some(message) = rx.next().await {
                let done = matches!(message, Message::Done | Message::Failed(_));
                let alive = this
                    .update(cx, |this, cx| {
                        let Some(state) = this.story.as_mut() else { return false };
                        state.receive_ahead(id, message);
                        if done {
                            state.anchor_ahead(&this.project, id);
                        }
                        if done && matches!(state.conversation.prefetching, Some((on_way, _)) if on_way == id) {
                            state.conversation.prefetching = None;
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
}

/// What a story has used of the model, as it's reported.
#[derive(Default, Debug, Clone, Copy, PartialEq)]
pub(super) struct Used {
    input: u64,
    output: u64,
}

impl Used {
    fn add(&mut self, input: u64, output: u64) {
        self.input += input;
        self.output += output;
    }

    /// ` · 12.4k tokens in, 800 out`, if anything's known.
    pub(super) fn said(&self) -> String {
        if self.input + self.output == 0 {
            return String::new();
        }
        let count = |n: u64| if n < 1000 { n.to_string() } else { format!("{:.1}k", n as f64 / 1000.) };
        format!(" · {} tokens in, {} out", count(self.input), count(self.output))
    }
}

#[cfg(test)]
mod used_tests {
    use super::Used;

    #[test]
    fn what_a_story_used_is_added_up_and_said() {
        let mut used = Used::default();
        assert_eq!(used.said(), "");
        used.add(12_000, 500);
        assert_eq!(used.said(), " · 12.0k tokens in, 500 out");
        used.add(400, 300);
        assert_eq!(used.said(), " · 12.4k tokens in, 800 out");
    }
}
