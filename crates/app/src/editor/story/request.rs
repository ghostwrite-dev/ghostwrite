//! Running what the model is asked. What a request is, and what's done with
//! what comes back, is the story's state's
//! ([`ghostwrite_view::story::request`]); here it's sent to the backend
//! and the reply streams in on a background thread, each thing it reports
//! handed on as an [`Event`]. While an item is reviewed, the next one is
//! asked for ahead the same way.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use futures::StreamExt as _;
use ghostwrite_ai::backend::Listener;
use ghostwrite_view::story::Event;
use gpui::{AppContext as _, Context, Task};

use super::Live;
use crate::editor::Editor;

/// The requests on their way, and how to stop them.
#[derive(Default)]
pub(super) struct Requests {
    /// Stops the reply on its way.
    cancel: Arc<AtomicBool>,
    _task: Option<Task<()>>,
    /// The request sent ahead of time that's on its way, and how to stop it.
    prefetching: Option<(u64, Arc<AtomicBool>)>,
    _prefetch_task: Option<Task<()>>,
    /// What the user has let the model read outside the project, during this story.
    readable: Readable,
    /// Where the answer goes, while the model asks to read outside the project.
    pub(super) read_answer: Option<std::sync::mpsc::Sender<bool>>,
}

/// A story that's dropped (its window closed) stops what it asked: nothing
/// more is spent on replies no one will read.
impl Drop for Requests {
    fn drop(&mut self) {
        self.stop();
    }
}

impl Requests {
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

impl Live {
    /// The request asked ahead that's on its way is the one the story's
    /// state has: one it's let go of runs on, but isn't stopped from here.
    fn keep_up_with_ahead(&mut self) {
        if self.requests.prefetching.as_ref().is_some_and(|(id, _)| self.state.asking_ahead() != Some(*id)) {
            self.requests.prefetching = None;
        }
    }
}

/// What a running request reports, on its way to the editor.
enum Message {
    Said(Event),
    /// The model would read this, outside the project: the user's asked, and the answer goes back.
    MayRead(PathBuf, std::sync::mpsc::Sender<bool>),
}

impl Message {
    fn ends(&self) -> bool {
        matches!(self, Message::Said(event) if event.ends())
    }
}

struct Forward(futures::channel::mpsc::UnboundedSender<Message>, Readable);

/// A request sent ahead of time: what it reports goes to the story's replies asked ahead.
struct Ahead(futures::channel::mpsc::UnboundedSender<Message>, Readable);

/// What the user has let the model read outside the project, during this story.
type Readable = Arc<std::sync::Mutex<Vec<PathBuf>>>;

fn allowed(readable: &Readable, path: &std::path::Path) -> bool {
    readable.lock().is_ok_and(|ok| ok.iter().any(|ok| path.starts_with(ok)))
}

fn say(tx: &futures::channel::mpsc::UnboundedSender<Message>, event: Event) {
    tx.unbounded_send(Message::Said(event)).ok();
}

impl Listener for Ahead {
    /// Only what's been allowed already: the user isn't asked about a step they aren't at.
    fn may_read(&mut self, path: &std::path::Path) -> bool {
        allowed(&self.1, path)
    }

    fn used(&mut self, input: u64, output: u64) {
        say(&self.0, Event::Used(input, output));
    }

    fn text(&mut self, chunk: &str) {
        say(&self.0, Event::Text(chunk.to_string()));
    }

    fn looking(&mut self, _what: String) {}

    fn session(&mut self, id: String) {
        say(&self.0, Event::Session(id));
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
        say(&self.0, Event::Used(input, output));
    }

    fn text(&mut self, chunk: &str) {
        say(&self.0, Event::Text(chunk.to_string()));
    }

    fn looking(&mut self, what: String) {
        say(&self.0, Event::Looking(what));
    }

    fn session(&mut self, id: String) {
        say(&self.0, Event::Session(id));
    }

    fn started(&mut self, ahead: bool) {
        say(&self.0, Event::Started(ahead));
    }
}

impl Live {
    /// The model asks to read `path`, outside the project: the story asks the
    /// user, and `answer` is where what they say goes.
    pub(in crate::editor) fn may_read(&mut self, path: PathBuf, answer: std::sync::mpsc::Sender<bool>) {
        self.requests.read_answer = Some(answer);
        self.state.receive(Event::MayRead(path));
    }

    fn receive(&mut self, message: Message) {
        match message {
            Message::Said(event) => self.state.receive(event),
            Message::MayRead(path, answer) => self.may_read(path, answer),
        }
    }
}

impl Editor {
    /// Send the story's request on a background thread; the reply streams in.
    pub(in crate::editor) fn send_request(&mut self, cx: &mut Context<Self>) {
        // The story's command is running: it's asked once that's done, with what it printed.
        if let Some(running) = self.terminal.running.as_mut() {
            running.ask = true;
            return cx.notify();
        }
        // The model reads the project from disk: let it see what's on screen.
        self.project.write_all();
        let Some(backend) = self.backend.clone() else { return };
        let terminal = self.terminal_now();
        let Some(live) = self.story.as_mut() else { return };
        let Some(req) = live.state.to_send(&self.project, terminal) else { return };
        // Whatever was still coming is superseded, and what it asked goes unanswered (no).
        let requests = &mut live.requests;
        requests.read_answer = None;
        requests.cancel.store(true, Ordering::SeqCst);
        requests.cancel = Arc::new(AtomicBool::new(false));
        let cancel = requests.cancel.clone();
        let mine = cancel.clone();
        let readable = requests.readable.clone();
        let (tx, mut rx) = futures::channel::mpsc::unbounded();
        cx.background_spawn(async move {
            let result =
                ghostwrite_project::contained("asking the model", || backend.run(&req, &mut Forward(tx.clone(), readable), &cancel)).and_then(|result| result);
            say(&tx, result.map_or_else(Event::Failed, |()| Event::Done));
        })
        .detach();
        requests._task = Some(cx.spawn(async move |this, cx| {
            while let Some(message) = rx.next().await {
                let done = message.ends();
                let alive = this
                    .update(cx, |this, cx| {
                        // A newer request (a correction sent mid-reply) supersedes this one.
                        let Some(live) = this.story.as_mut().filter(|s| Arc::ptr_eq(&s.requests.cancel, &mine)) else { return false };
                        live.receive(message);
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
        let Some(live) = self.story.as_mut() else { return };
        let ahead = live.state.ask_ahead(&self.project, backend.asks_ahead());
        if ahead.stop
            && let Some((_, cancel)) = live.requests.prefetching.take()
        {
            cancel.store(true, Ordering::SeqCst);
        }
        live.keep_up_with_ahead();
        let Some((id, req)) = ahead.ask else { return };
        let cancel = Arc::new(AtomicBool::new(false));
        let readable = live.requests.readable.clone();
        live.requests.prefetching = Some((id, cancel.clone()));
        let (tx, mut rx) = futures::channel::mpsc::unbounded();
        cx.background_spawn(async move {
            let result =
                ghostwrite_project::contained("asking ahead", || backend.run(&req, &mut Ahead(tx.clone(), readable), &cancel)).and_then(|result| result);
            say(&tx, result.map_or_else(Event::Failed, |()| Event::Done));
        })
        .detach();
        live.requests._prefetch_task = Some(cx.spawn(async move |this, cx| {
            while let Some(message) = rx.next().await {
                let done = message.ends();
                let alive = this
                    .update(cx, |this, cx| {
                        let Some(live) = this.story.as_mut() else { return false };
                        // Asked ahead, it doesn't ask to read: it's only told what's allowed.
                        if let Message::Said(event) = message {
                            live.state.receive_ahead(&this.project, id, event);
                            live.keep_up_with_ahead();
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
