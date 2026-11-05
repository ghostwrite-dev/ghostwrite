//! What the model is asked, and what it says back. A request is the story so
//! far, the file asked about and the others it's in, and what's around it in
//! the editor; the terminal is told as it was when the story started, and
//! again in a follow-up when it's changed. While an item is reviewed, the
//! next one is asked for ahead, assuming this one's accepted, in a fork of
//! the Claude Code session.
//!
//! Running a request is the app's: it sends what's built here to the backend,
//! in the background, and what comes back arrives here as [`Event`]s.

use std::collections::HashMap;
use std::ops::Range as Lines;
use std::path::PathBuf;

use ghostwrite_ai::context::Shown;
use ghostwrite_ai::placement::{Asked, Assumed, Files as _, Overlay, Place};
use ghostwrite_ai::timing::{Timer, Wait};
use ghostwrite_ai::{Item, Request, Story, Surroundings, lines};

use super::StoryState;
use crate::Project;

/// How the story talks with the model: its session, and the requests asked ahead.
#[derive(Default)]
pub(super) struct Conversation {
    /// The Claude Code session the story is in, resumed for each follow-up.
    session: Option<String>,
    /// The terminal, as the model is told it.
    terminal: Told,
    /// The request sent ahead of time that's on its way.
    prefetching: Option<u64>,
    /// The sessions requests sent ahead are in: the next one continues from there, and
    /// the story's session becomes one once its reply is needed.
    ahead_sessions: HashMap<u64, String>,
    /// The items each request sent ahead assumed accepted, to check its reply against.
    ahead_accepted: HashMap<u64, Vec<Item>>,
    /// A reply sent ahead was needed before its session was known.
    session_from: Option<u64>,
}

/// Something about the editor the model is told as it was when the story
/// started, and again in the newest turn whenever it's changed since.
#[derive(Default)]
struct Told {
    /// What the first request said: `None` until it's sent. Kept, so the
    /// conversation's start stays the same.
    first: Option<Option<String>>,
    /// What the model was last told.
    last: Option<String>,
}

impl Told {
    /// It's `now` as a request goes out: what's changed since the model was
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

/// What a running request reports, as the app hands it on.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Text(String),
    Looking(String),
    Session(String),
    Used(u64, u64),
    /// The model's process is up: started ahead of time, or just now.
    Started(bool),
    /// The model would read this, outside the project: the user's asked (the app keeps
    /// where the answer goes, and gets it as [`Do::AnswerRead`](super::Do::AnswerRead)).
    MayRead(PathBuf),
    Done,
    Failed(String),
}

impl Event {
    /// The request is over.
    pub fn ends(&self) -> bool {
        matches!(self, Event::Done | Event::Failed(_))
    }
}

/// What to do about asking ahead, while an item is reviewed.
#[derive(Default)]
pub struct AskAhead {
    /// The one on its way is no longer wanted: stop it.
    pub stop: bool,
    /// Ask this ahead, as this id.
    pub ask: Option<(u64, Request)>,
}

impl StoryState {
    /// The Claude Code session a follow-up would resume.
    pub fn conversation_session(&self) -> Option<&str> {
        self.conversation.session.as_deref()
    }

    /// The request sent ahead of time that's on its way, if any.
    pub fn asking_ahead(&self) -> Option<u64> {
        self.conversation.prefetching
    }

    /// The request for the story so far.
    pub fn request(&self, project: &Project) -> Option<Request> {
        self.request_ahead(project, &[])
    }

    /// The request as it would be with `accepted` applied too, as a request
    /// sent ahead assumes: the files it shows are as they'd be then.
    pub fn request_ahead(&self, project: &Project, accepted: &[Item]) -> Option<Request> {
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

    /// The request to send now, with the terminal showing `terminal`: the first
    /// request says what's in the terminal; a follow-up, what it shows now. It's
    /// timed until there's something to look at, and what the model was still
    /// asking goes unanswered (no).
    pub fn to_send(&mut self, project: &Project, terminal: Option<String>) -> Option<Request> {
        let seen = self.conversation.terminal.tell(terminal);
        if let (Some(terminal), Some(story)) = (seen, self.story.as_mut()) {
            story.saw_terminal(terminal);
        }
        let req = self.request(project)?;
        // A rewrite of a step that wouldn't place is part of the same wait.
        if self.timer.is_none() {
            self.timer = Some(Timer::start(if req.followups.is_empty() { Wait::Ask } else { Wait::FollowUp }));
        }
        self.asking_to_read = None;
        Some(req)
    }

    /// While an item is reviewed, ask ahead for the next one (and, as each
    /// arrives, the one after), assuming each is accepted, up to `most`. One
    /// that's no longer wanted is stopped.
    pub fn ask_ahead(&mut self, project: &Project, most: usize) -> AskAhead {
        let mut ahead = AskAhead::default();
        let Some(story) = self.story.as_mut() else { return ahead };
        if let Some(id) = self.conversation.prefetching {
            if story.wants(id) {
                return ahead;
            }
            ahead.stop = true;
            self.conversation.prefetching = None;
        }
        let Some(next) = story.prefetch(most) else { return ahead };
        let Some(mut req) = self.request_ahead(project, &next.accepted) else { return ahead };
        req.followups = next.followups;
        req.session = match next.after {
            None => self.conversation.session.clone(),
            Some(before) => self.conversation.ahead_sessions.get(&before).cloned(),
        };
        req.fork = req.session.is_some();
        self.conversation.ahead_accepted.insert(next.id, next.accepted);
        self.conversation.prefetching = Some(next.id);
        ahead.ask = Some((next.id, req));
        ahead
    }

    /// The files as they'd be with `accepted` applied.
    fn assume(&self, project: &Project, accepted: &[Item]) -> Option<Assumed> {
        let asked = project.tabs.by_id(self.doc)?;
        let asked = Asked { path: asked.doc.path.as_deref(), text: asked.doc.buffer.whole_text(), lines: self.lines.clone() };
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

    /// What the request on its way reports.
    pub fn receive(&mut self, event: Event) {
        if let Event::Session(id) = event {
            self.conversation.session = Some(id);
            return;
        }
        if let Event::Used(input, output) = event {
            self.used.add(input, output);
            return;
        }
        if let Some(timer) = self.timer.as_mut() {
            match event {
                Event::Started(ahead) => timer.started(ahead),
                Event::Text(_) => timer.text(),
                _ => {}
            }
        }
        if let Event::MayRead(path) = event {
            self.asking_to_read = Some(path);
            return;
        }
        let Some(story) = self.story.as_mut() else { return };
        match event {
            Event::Session(_) | Event::Used(..) | Event::Started(_) | Event::MayRead(_) => {}
            Event::Text(chunk) => story.push_text(&chunk),
            Event::Looking(what) => story.looking = Some(what),
            Event::Done => story.finished_writing(),
            Event::Failed(e) => story.failed(e),
        }
        if !story.is_writing() {
            ghostwrite_ai::memory::save_transcript(&story.transcript());
        }
    }

    /// What a request sent ahead (`id`) reports. Once it's over, its reply is
    /// placed in the files as it assumed them.
    pub fn receive_ahead(&mut self, project: &Project, id: u64, event: Event) {
        let done = event.ends();
        self.hear_ahead(id, event);
        if done {
            self.anchor_ahead(project, id);
            if self.conversation.prefetching == Some(id) {
                self.conversation.prefetching = None;
            }
        }
    }

    fn hear_ahead(&mut self, id: u64, event: Event) {
        let Some(story) = self.story.as_mut() else { return };
        if let Event::Used(input, output) = event {
            self.used.add(input, output);
            return;
        }
        match event {
            Event::Session(session) => {
                if self.conversation.session_from == Some(id) {
                    self.conversation.session = Some(session.clone());
                }
                self.conversation.ahead_sessions.insert(id, session);
            }
            Event::Text(chunk) => story.ahead_text(id, &chunk),
            Event::Looking(_) | Event::Used(..) | Event::Started(_) | Event::MayRead(_) => {}
            Event::Done => story.ahead_finished(id, None),
            Event::Failed(e) => story.ahead_finished(id, Some(e)),
        }
        if self.conversation.prefetching == Some(id) && !story.wants(id) {
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
    fn anchor_ahead(&mut self, project: &Project, id: u64) {
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
