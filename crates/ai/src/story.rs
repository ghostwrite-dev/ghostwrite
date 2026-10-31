//! A story: one request and the conversation it starts, reviewed one item at
//! a time. Every reply is exactly one item (a note, an edit, or a new file),
//! or `<done>`. Once you've accepted the current item (or read it), the story
//! asks for the next one. There's no skipping: what you say instead goes with
//! that request, and the next item follows it.
//! There's no plan to keep in step: the model only ever answers "what's next?".
//!
//! While you review an item, the next ones are already being asked for:
//! up to [`PREFETCH`] ahead, each assuming you accept the ones before it.
//! Accept, and the next is there (or streaming); say something, and the ones
//! asked ahead are dropped and the next is asked for as usual.
//!
//! A reply Ghostwrite can't use (one that breaks the format, an edit whose
//! lines aren't in its file, a file that doesn't exist) goes back to the model
//! the same way, as a correction, until it's right or [`MAX_REVISIONS`] tries
//! in a row fail.
//!
//! This is the bookkeeping only. Applying an accepted edit to a buffer, or
//! creating a file, is the editor's job; it tells the story afterwards with
//! [`Story::accept`] (or [`Story::follow_up`] when the user says something).

use std::collections::HashMap;
use std::ops::Range;

use crate::protocol::{self, FileOp, Note, Reply, Step};

/// Corrections the model gets in a row before an item is left for the user to redirect.
pub const MAX_REVISIONS: usize = 3;
/// Items asked for ahead of the one under review.
pub const PREFETCH: usize = 10;

/// An item to review: the reply it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ItemId(pub usize);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Item {
    Step(Step),
    Note(Note),
}

impl Item {
    /// The lines it quotes from its file: a note's `<at>`, a step's old lines.
    pub fn quoted(&self) -> &[String] {
        match self {
            Item::Note(note) => &note.at,
            Item::Step(step) => &step.old,
        }
    }

    /// Where its quoted lines start, as their numbers (or the editor) said.
    pub fn line(&self) -> Option<usize> {
        match self {
            Item::Note(note) => note.line,
            Item::Step(step) => step.line,
        }
    }

    /// The item as the model wrote it, to remind it what the user decided.
    pub fn describe(&self) -> String {
        match self {
            Item::Note(note) => format!("a note: {}", note.text),
            Item::Step(step) if step.creates() => format!("creating {}", step.file.as_deref().unwrap_or("a file")),
            Item::Step(Step { op: Some(FileOp::Remove), file, .. }) => format!("removing {}", file.as_deref().unwrap_or("a file")),
            Item::Step(Step { op: Some(FileOp::Rename(to)), file, .. }) => format!("renaming {} to {to}", file.as_deref().unwrap_or("a file")),
            Item::Step(step) => {
                let file = step.file.as_ref().map_or(String::new(), |f| format!(" in {f}"));
                format!("an edit{file}, replacing\n{}\nwith\n{}", step.old.join("\n"), step.new.join("\n"))
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    Pending,
    Accepted,
    /// The user said something instead, or Ghostwrite corrected it.
    Redirected,
}

/// Where the review is after a change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Settled {
    At(ItemId),
    /// The next item has been asked for: send the request.
    Ask,
    /// Nothing to review until more of the reply arrives (or after an error).
    Waiting,
    Finished,
}

/// One turn of the conversation: the model's reply, what the user did with
/// it, and what they said next (nothing: they want the next item).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FollowUp {
    /// The model's reply just before this.
    pub reply: String,
    /// What the user accepted since (`accepted: …`).
    pub decided: Vec<String>,
    pub said: String,
    /// Ghostwrite, not the user, is speaking: the reply couldn't be used.
    pub correction: bool,
    /// A `:sh` command the user ran since that reply, with its output.
    pub command: Option<String>,
    /// The user's terminal, if it's changed since that reply.
    pub terminal: Option<String>,
}

/// A reply asked for ahead of time, assuming everything before it is accepted.
#[derive(Debug)]
struct Ahead {
    id: u64,
    text: String,
    done: bool,
    /// The lines its edit goes at, placed once in the files it was written against.
    anchor: Option<Range<usize>>,
}

/// A request to send ahead of time: the conversation as it would be if the
/// user accepts what's before it, and those items (to apply to the file it
/// shows, as accepting would).
#[derive(Debug)]
pub struct Prefetch {
    pub id: u64,
    /// The prefetch it builds on, whose conversation it continues.
    pub after: Option<u64>,
    pub followups: Vec<FollowUp>,
    pub accepted: Vec<Item>,
}

#[derive(Debug)]
struct Turn {
    text: String,
    /// The item the user said something about, which this reply answers.
    about: Option<ItemId>,
    /// An item given rather than written (docs shown with `space k`).
    given: Option<Item>,
}

#[derive(Debug)]
pub struct Story {
    pub instruction: String,
    history: Vec<FollowUp>,
    /// Reply `k` answers the request with the first `k` follow-ups.
    replies: Vec<Turn>,
    verdicts: HashMap<ItemId, Verdict>,
    /// Decisions since the last request, for the next one.
    decided: Vec<String>,
    current: Option<ItemId>,
    /// The reply being written.
    writing: Option<usize>,
    /// Whether the next item is asked for once this one is decided.
    open: bool,
    pub error: Option<String>,
    /// What the model is looking at in the project right now.
    pub looking: Option<String>,
    /// Corrections sent since the user last decided something.
    revisions: usize,
    /// Replies asked for ahead, in order.
    ahead: Vec<Ahead>,
    next_ahead: u64,
    /// The reply asked ahead that's now the one being written, still streaming.
    adopted: Option<u64>,
    /// One asked ahead that just became the next reply, for the editor to take its session.
    just_adopted: Option<u64>,
    /// Where the current reply's edit goes, if it was asked ahead and placed when it arrived.
    anchor: Option<Range<usize>>,
    /// Items the editor gave that are still to come (the stops of a search, say).
    queued: std::collections::VecDeque<Item>,
}

impl Story {
    /// A request on its way: the first item will arrive in the first reply.
    pub fn new(instruction: impl Into<String>) -> Self {
        Self {
            instruction: instruction.into(),
            history: vec![],
            replies: vec![Turn { text: String::new(), about: None, given: None }],
            verdicts: HashMap::new(),
            decided: vec![],
            current: None,
            writing: Some(0),
            open: true,
            error: None,
            looking: None,
            revisions: 0,
            ahead: vec![],
            next_ahead: 0,
            adopted: None,
            just_adopted: None,
            anchor: None,
            queued: std::collections::VecDeque::new(),
        }
    }

    /// A story of items the editor gives (a file's diagnostics, a search's
    /// matches, docs from `space k`), shown one at a time like any others. It
    /// ends after the last one, unless the user says something: then the model
    /// takes over from there, and the rest are dropped. `None` if there are none.
    pub fn given(instruction: impl Into<String>, items: Vec<Item>) -> Option<Self> {
        let mut queued: std::collections::VecDeque<Item> = items.into();
        let first = queued.pop_front()?;
        let mut story = Self::new(instruction);
        story.replies[0] = Turn { text: render(&first), about: None, given: Some(first) };
        story.queued = queued;
        story.writing = None;
        story.open = false;
        story.settle();
        Some(story)
    }

    /// Whether Claude is in it (asked, or handed over to), not only items
    /// the editor gave.
    pub fn involves_model(&self) -> bool {
        self.replies.iter().any(|turn| turn.given.is_none())
    }

    /// A story that starts with a note already there (docs from `space k`),
    /// and ends with it unless the user asks about it.
    pub fn answered(instruction: impl Into<String>, answer: &str) -> Self {
        let note = Note { file: None, at: vec![], text: answer.trim().to_string(), line: None };
        Self::given(instruction, vec![Item::Note(note)]).expect("one item")
    }

    pub fn followups(&self) -> &[FollowUp] {
        &self.history
    }

    pub fn current(&self) -> Option<ItemId> {
        self.current
    }

    pub fn is_writing(&self) -> bool {
        self.writing.is_some()
    }

    /// What reply `k` is, as far as it has arrived.
    fn reply(&self, k: usize) -> Reply {
        let turn = &self.replies[k];
        match &turn.given {
            Some(Item::Note(note)) => Reply::Note(note.clone()),
            Some(Item::Step(step)) => Reply::Step(step.clone()),
            None => protocol::parse(&turn.text, self.writing != Some(k)),
        }
    }

    pub fn item(&self, id: ItemId) -> Option<Item> {
        match self.reply(id.0) {
            Reply::Note(note) => Some(Item::Note(note)),
            Reply::Step(step) => Some(Item::Step(step)),
            Reply::Partial | Reply::Done { .. } => None,
        }
    }

    /// Every item so far, in order.
    pub fn items(&self) -> Vec<(ItemId, Item)> {
        (0..self.replies.len()).filter_map(|k| Some((ItemId(k), self.item(ItemId(k))?))).collect()
    }

    pub fn current_item(&self) -> Option<Item> {
        self.item(self.current?)
    }

    /// The current item's place among the items (zero-based), and how many
    /// there are so far, counting those the editor gave that are still to come
    /// and those asked ahead that have arrived as items.
    pub fn progress(&self) -> Option<(usize, usize)> {
        let items = self.items();
        let ahead = self.ahead.iter().filter(|a| matches!(protocol::parse(&a.text, a.done), Reply::Step(_) | Reply::Note(_))).count();
        Some((items.iter().position(|(id, _)| Some(*id) == self.current)?, items.len() + self.queued.len() + ahead))
    }

    /// The item a reply answers what the user said about, if it does.
    pub fn asked_about(&self, id: ItemId) -> Option<ItemId> {
        self.replies.get(id.0)?.about
    }

    /// What the reply being written answers, while it's on its way.
    pub fn writing_about(&self) -> Option<ItemId> {
        self.writing.and_then(|k| self.replies[k].about)
    }

    pub fn verdict(&self, id: ItemId) -> Verdict {
        self.verdicts.get(&id).copied().unwrap_or(Verdict::Pending)
    }

    /// The user says something about the current item (or while waiting).
    /// The next item, in a new reply, follows what they said.
    pub fn follow_up(&mut self, said: String) {
        self.revisions = 0;
        self.follow(said, false);
    }

    /// Whether the model may still be asked to correct its reply.
    pub fn can_revise(&self) -> bool {
        self.revisions < MAX_REVISIONS
    }

    /// The current item's `<at>` lines (`quoted`, looked for near the lines
    /// `near`) aren't in its file (`file`, whose text is `text`): the model
    /// writes it again.
    pub fn misplaced(&mut self, file: &str, text: &str, quoted: &[String], near: Range<usize>) {
        self.revise(protocol::misplaced(file, text, quoted, near));
    }

    /// The current item is about a PDF, which can't have items.
    pub fn about_a_pdf(&mut self, file: &str) {
        self.revise(protocol::about_a_pdf(file));
    }

    /// The current item names a file that isn't in the project.
    pub fn file_missing(&mut self, file: &str) {
        self.revise(protocol::missing_file(file));
    }

    /// The reply being written breaks the format (`why`), or the finished one
    /// has nothing to review: the model writes it again, in the format.
    pub fn reply_unusable(&mut self, why: Option<&str>) {
        let said = match why {
            Some(why) => format!("your reply breaks the format: {why}. {}", protocol::NOTHING_TO_REVIEW),
            None => protocol::NOTHING_TO_REVIEW.into(),
        };
        self.revise(said);
    }

    /// Whether the reply so far breaks the format, and how; once it's
    /// complete, also whether it ended where an item can.
    pub fn format_problem(&self) -> Option<String> {
        let k = self.replies.len() - 1;
        let turn = &self.replies[k];
        if turn.given.is_some() || self.error.is_some() {
            return None;
        }
        protocol::check(&turn.text, self.writing != Some(k)).err()
    }

    /// Whether the last reply, now complete, has nothing to review.
    pub fn is_unusable(&self) -> bool {
        let k = self.replies.len() - 1;
        self.writing.is_none() && self.error.is_none() && self.reply(k) == Reply::Partial
    }

    fn revise(&mut self, said: String) {
        self.revisions += 1;
        self.follow(said, true);
    }

    fn follow(&mut self, said: String, correction: bool) {
        let k = self.replies.len() - 1;
        let item = self.item(ItemId(k));
        // A note is read once it's been asked about; anything else undecided is set aside.
        if self.verdict(ItemId(k)) == Verdict::Pending && item.is_some() {
            let read = !correction && matches!(item, Some(Item::Note(_)));
            self.verdicts.insert(ItemId(k), if read { Verdict::Accepted } else { Verdict::Redirected });
        }
        let about = (!correction && item.is_some()).then_some(ItemId(k));
        self.ask(said, correction, about);
    }

    /// The next turn: what was decided and said, and a reply on its way.
    /// What was asked ahead assumed otherwise, so it's dropped.
    fn ask(&mut self, said: String, correction: bool, about: Option<ItemId>) {
        self.ahead.clear();
        self.adopted = None;
        self.anchor = None;
        self.queued.clear();
        let reply = self.replies.last().map(|t| t.text.clone()).unwrap_or_default();
        self.history.push(FollowUp { reply, decided: std::mem::take(&mut self.decided), said, correction, command: None, terminal: None });
        self.replies.push(Turn { text: String::new(), about, given: None });
        self.error = None;
        self.current = None;
        self.open = true;
        self.writing = Some(self.replies.len() - 1);
    }

    /// The user ran `command` (with its output) since the last reply: the
    /// newest turn says so.
    pub fn ran(&mut self, command: String) {
        if let Some(followup) = self.history.last_mut() {
            followup.command = Some(command);
        }
    }

    /// The user's terminal has changed since the last reply: the newest turn shows it.
    pub fn saw_terminal(&mut self, terminal: String) {
        if let Some(followup) = self.history.last_mut() {
            followup.terminal = Some(terminal);
        }
    }

    /// More of the reply being written.
    pub fn push_text(&mut self, chunk: &str) {
        if let Some(k) = self.writing {
            self.replies[k].text.push_str(chunk);
        }
    }

    pub fn finished_writing(&mut self) {
        self.writing = None;
        self.looking = None;
    }

    pub fn failed(&mut self, error: String) {
        self.finished_writing();
        self.error = Some(error);
    }

    /// The current item was accepted (and applied), or read.
    pub fn accept(&mut self) {
        let Some(id) = self.current.take() else { return };
        self.revisions = 0;
        // Reading a note isn't worth telling the model about.
        if let Some(item) = self.item(id).filter(|i| !matches!(i, Item::Note(_))) {
            self.decided.push(format!("accepted: {}", item.describe()));
        }
        self.verdicts.insert(id, Verdict::Accepted);
    }

    /// What the model said, in brief, for the log of interactions: its notes
    /// (cut short), or the changes it proposed and how many were applied.
    pub fn recap(&self) -> String {
        const MAX: usize = 600;
        let items = self.items();
        let notes: Vec<String> = items.iter().filter_map(|(_, i)| if let Item::Note(n) = i { Some(n.text.replace('\n', " ")) } else { None }).collect();
        let mut files: Vec<String> = items
            .iter()
            .filter_map(|(_, i)| if let Item::Step(s) = i { Some(s.file.clone().unwrap_or_else(|| "the file asked about".into())) } else { None })
            .collect();
        files.dedup();
        let mut recap = notes.join(" / ");
        if !files.is_empty() {
            let (kept, total) = self.summary();
            recap.push_str(&format!("{}proposed {total} change(s) in {}, {kept} applied", if recap.is_empty() { "" } else { " / " }, files.join(", ")));
        }
        if recap.len() > MAX {
            recap.truncate(recap.floor_char_boundary(MAX));
            recap.push('…');
        }
        recap
    }

    /// Where the review is now. An edit shows as soon as its new lines start
    /// streaming, a note once its reply is complete. Once the current item is
    /// decided, the next one is asked for ([`Settled::Ask`]: send the request);
    /// `<done>` (or a given note, decided) finishes the story.
    pub fn settle(&mut self) -> Settled {
        if let Some(id) = self.current.filter(|id| self.verdict(*id) == Verdict::Pending) {
            return Settled::At(id);
        }
        self.current = None;
        let k = self.replies.len() - 1;
        let id = ItemId(k);
        match self.reply(k) {
            Reply::Step(_) | Reply::Note(_) if self.verdict(id) == Verdict::Pending => {
                self.current = Some(id);
                Settled::At(id)
            }
            _ if self.writing.is_some() || self.error.is_some() => Settled::Waiting,
            Reply::Step(_) | Reply::Note(_) if !self.queued.is_empty() => {
                self.reveal();
                self.settle()
            }
            Reply::Step(_) | Reply::Note(_) if self.open && self.verdict(id) == Verdict::Accepted && !self.ahead.is_empty() => {
                self.adopt();
                self.settle()
            }
            Reply::Step(_) | Reply::Note(_) if self.open => {
                self.ask(String::new(), false, None);
                Settled::Ask
            }
            _ => Settled::Finished,
        }
    }

    /// The next item the editor gave becomes the next reply, as if the model had written it.
    fn reveal(&mut self) {
        let Some(item) = self.queued.pop_front() else { return };
        let reply = self.replies.last().map(|t| t.text.clone()).unwrap_or_default();
        self.history.push(FollowUp {
            reply,
            decided: std::mem::take(&mut self.decided),
            said: String::new(),
            correction: false,
            command: None,
            terminal: None,
        });
        self.replies.push(Turn { text: render(&item), about: None, given: Some(item) });
    }

    /// The first reply asked ahead becomes the next one: it assumed just what happened.
    fn adopt(&mut self) {
        let next = self.ahead.remove(0);
        let rest = std::mem::take(&mut self.ahead);
        self.ask(String::new(), false, None);
        self.ahead = rest;
        self.anchor = next.anchor;
        let k = self.replies.len() - 1;
        self.replies[k].text = next.text;
        if next.done {
            self.writing = None;
        } else {
            self.adopted = Some(next.id);
        }
        self.just_adopted = Some(next.id);
    }

    /// The reply asked ahead that just became the next one, once.
    pub fn take_adopted(&mut self) -> Option<u64> {
        self.just_adopted.take()
    }

    /// Where the current item's edit goes, if it was placed when it arrived ahead.
    pub fn anchor(&self) -> Option<Range<usize>> {
        self.anchor.clone()
    }

    /// The next request to send ahead of time, if it's time for one: the
    /// current item is complete, nothing else is on its way, and fewer than
    /// [`PREFETCH`] are asked ahead, the last of them an item (not `<done>`).
    pub fn prefetch(&mut self) -> Option<Prefetch> {
        let current = self.current?;
        let item = self.item(current)?;
        let ready = self.open && self.error.is_none() && self.writing.is_none() && !matches!(&item, Item::Step(s) if !s.complete);
        let last_is_item = self.ahead.last().is_none_or(|a| a.done && matches!(protocol::parse(&a.text, true), Reply::Step(_) | Reply::Note(_)));
        if !ready || self.ahead.len() >= PREFETCH || !last_is_item {
            return None;
        }
        let mut followups = self.history.clone();
        let mut accepted = vec![];
        let mut decided = self.decided.clone();
        let mut reply = self.replies[current.0].text.clone();
        let mut next = Some(item);
        for a in self.ahead.iter().map(Some).chain([None]) {
            if let Some(item) = next.take() {
                if let Item::Step(_) = &item {
                    decided.push(format!("accepted: {}", item.describe()));
                }
                followups.push(FollowUp {
                    reply: std::mem::take(&mut reply),
                    decided: std::mem::take(&mut decided),
                    said: String::new(),
                    correction: false,
                    command: None,
                    terminal: None,
                });
                accepted.push(item);
            }
            if let Some(a) = a {
                reply = a.text.clone();
                next = match protocol::parse(&a.text, true) {
                    Reply::Step(step) => Some(Item::Step(step)),
                    Reply::Note(note) => Some(Item::Note(note)),
                    _ => None,
                };
            }
        }
        let id = self.next_ahead;
        self.next_ahead += 1;
        let after = self.ahead.last().map(|a| a.id);
        self.ahead.push(Ahead { id, text: String::new(), done: false, anchor: None });
        Some(Prefetch { id, after, followups, accepted })
    }

    /// Whether a request sent ahead (`id`) is still wanted.
    pub fn wants(&self, id: u64) -> bool {
        self.adopted == Some(id) || self.ahead.iter().any(|a| a.id == id)
    }

    /// More of a reply asked ahead (or, once it's the next reply, of that).
    pub fn ahead_text(&mut self, id: u64, chunk: &str) {
        if self.adopted == Some(id) {
            self.push_text(chunk);
        } else if let Some(a) = self.ahead.iter_mut().find(|a| a.id == id) {
            a.text.push_str(chunk);
        }
    }

    /// A reply asked ahead is complete, or failed (and what was asked after it with it).
    pub fn ahead_finished(&mut self, id: u64, error: Option<String>) {
        if self.adopted == Some(id) {
            self.adopted = None;
            match error {
                Some(e) => self.failed(e),
                None => self.finished_writing(),
            }
        } else if let Some(i) = self.ahead.iter().position(|a| a.id == id) {
            match error {
                Some(_) => self.ahead.truncate(i),
                None => self.ahead[i].done = true,
            }
        }
    }

    /// The item in a reply asked ahead (`id`), once it's complete.
    pub fn ahead_item(&self, id: u64) -> Option<Item> {
        match protocol::parse(&self.ahead.iter().find(|a| a.id == id && a.done)?.text, true) {
            Reply::Step(step) => Some(Item::Step(step)),
            Reply::Note(note) => Some(Item::Note(note)),
            _ => None,
        }
    }

    /// A reply asked ahead (`id`) placed at `lines` of its file, in the files it was written against.
    pub fn anchor_ahead(&mut self, id: u64, lines: Range<usize>) {
        if let Some(a) = self.ahead.iter_mut().find(|a| a.id == id) {
            a.anchor = Some(lines);
        }
    }

    /// Changes accepted, of changes proposed.
    pub fn summary(&self) -> (usize, usize) {
        let changes: Vec<ItemId> = self.items().into_iter().filter(|(_, i)| !matches!(i, Item::Note(_))).map(|(id, _)| id).collect();
        (changes.iter().filter(|id| self.verdict(**id) == Verdict::Accepted).count(), changes.len())
    }

    /// What came of it, in a few words.
    pub fn outcome(&self) -> String {
        match self.summary() {
            (kept, total) if total > 0 => format!("applied {kept} of {total} steps"),
            _ if self.items().is_empty() => self.error.clone().map_or("no reply".into(), |e| format!("failed: {e}")),
            _ => "answered".into(),
        }
    }

    /// The model's rewritten notes, if it ended with them.
    pub fn memory_update(&self) -> Option<String> {
        (0..self.replies.len()).rev().find_map(|k| if let Reply::Done { memory } = self.reply(k) { memory } else { None })
    }

    /// The request and every reply, kept for when an edit won't place.
    pub fn transcript(&self) -> String {
        let mut out = format!("request: {}\n", self.instruction);
        for reply in &self.replies {
            out.push_str(&format!("\n--- reply\n{}\n", reply.text));
        }
        for (said, n) in self.history.iter().zip(1..) {
            let who = if said.correction { "correction" } else { "follow-up" };
            out.push_str(&format!("\n--- {who} {n}: {}\n", said.said));
        }
        out
    }
}

/// A given item in the reply format, for the conversation the model sees if it takes over.
fn render(item: &Item) -> String {
    match item {
        Item::Note(note) => note.render(),
        Item::Step(step) => format!("<say>\n{}\n</say>\n<new>\n{}\n</new>\n", step.say, step.new.join("\n")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIX: &str = "<file>\ncalc.py\n</file>\n<at>\n    pass\n</at>\n<say>\nReturn the sum.\n</say>\n<new>\n    return a + b\n</new>\n";
    const HINTS: &str = "<file>\ncalc.py\n</file>\n<at>\ndef add(a, b):\n</at>\n<say>\nType hints.\n</say>\n<new>\ndef add(a: int, b: int) -> int:\n</new>\n";
    const NOTE: &str = "<file>\ncalc.py\n</file>\n<at>\ndef add(a, b):\n</at>\n<say>\nBoth are ints here.\n</say>\n";
    const DONE: &str = "<done>\n</done>\n";

    fn arrived(story: &mut Story, reply: &str) -> Settled {
        story.push_text(reply);
        story.finished_writing();
        story.settle()
    }

    #[test]
    fn each_decision_asks_for_the_next_item() {
        let mut story = Story::new("implement it");
        assert_eq!(story.settle(), Settled::Waiting);
        assert_eq!(arrived(&mut story, FIX), Settled::At(ItemId(0)));
        story.accept();
        assert_eq!(story.settle(), Settled::Ask, "the next item is asked for");
        assert_eq!(story.followups().len(), 1);
        assert!(story.followups()[0].decided[0].starts_with("accepted: an edit in calc.py"), "the model is told what happened");
        assert_eq!(story.followups()[0].said, "", "and nothing else");
        assert_eq!(arrived(&mut story, HINTS), Settled::At(ItemId(1)));
        story.follow_up("no hints".into());
        assert_eq!(arrived(&mut story, DONE), Settled::Finished);
        assert_eq!(story.outcome(), "applied 1 of 2 steps");
    }

    #[test]
    fn an_edit_shows_while_its_new_lines_stream() {
        let mut story = Story::new("implement it");
        story.push_text(&FIX[..FIX.len() - 12]);
        assert_eq!(story.settle(), Settled::At(ItemId(0)));
        assert!(matches!(story.current_item(), Some(Item::Step(s)) if !s.complete));
    }

    #[test]
    fn what_the_user_says_goes_with_the_next_request() {
        let mut story = Story::new("implement it");
        arrived(&mut story, FIX);
        story.follow_up("why not float?".into());
        assert_eq!(story.verdict(ItemId(0)), Verdict::Redirected, "the edit is set aside");
        assert_eq!(story.writing_about(), Some(ItemId(0)), "the answer comes where it was asked");
        assert_eq!(story.followups()[0].said, "why not float?");
        assert_eq!(arrived(&mut story, NOTE), Settled::At(ItemId(1)));
        assert_eq!(story.asked_about(ItemId(1)), Some(ItemId(0)));
        story.accept();
        assert_eq!(story.settle(), Settled::Ask, "after the answer, the next item");
    }

    #[test]
    fn a_reply_that_isnt_an_item_is_corrected() {
        let mut story = Story::new("implement it");
        story.push_text("Sure! Here you go:\n```\n");
        assert_eq!(story.format_problem(), None, "a word before the item is let through while it streams");
        story.finished_writing();
        assert!(story.format_problem().is_some());
        assert!(story.is_unusable());
        story.reply_unusable(story.format_problem().as_deref());
        assert!(story.followups()[0].correction);
        assert_eq!(arrived(&mut story, FIX), Settled::At(ItemId(1)));
    }

    #[test]
    fn given_items_are_shown_one_at_a_time_and_end_the_story() {
        let note =
            |line: usize| Item::Note(Note { file: Some("a.py".into()), at: vec![format!("line {line}")], text: format!("stop {line}"), line: Some(line) });
        assert!(Story::given("nothing", vec![]).is_none());
        let mut story = Story::given("the matches", vec![note(1), note(5), note(9)]).unwrap();
        assert_eq!(story.current(), Some(ItemId(0)));
        story.accept();
        assert_eq!(story.settle(), Settled::At(ItemId(1)), "the next one, without asking the model");
        story.accept();
        story.settle();
        story.accept();
        assert_eq!(story.settle(), Settled::Finished, "after the last");
        assert!(story.followups().iter().all(|f| f.said.is_empty() && !f.correction));
    }

    #[test]
    fn asking_about_a_given_item_hands_over_to_the_model() {
        let note =
            |line: usize| Item::Note(Note { file: Some("a.py".into()), at: vec![format!("line {line}")], text: format!("stop {line}"), line: Some(line) });
        let mut story = Story::given("the matches", vec![note(1), note(5), note(9)]).unwrap();
        story.follow_up("why is this one here?".into());
        assert!(story.is_writing(), "the model answers");
        assert!(story.followups()[0].reply.contains("<say>\nstop 1\n</say>"), "and sees the stop as if it had shown it");
        assert_eq!(arrived(&mut story, NOTE), Settled::At(ItemId(1)));
        story.accept();
        assert_eq!(story.settle(), Settled::Ask, "it carries on; the other stops are dropped");
    }

    #[test]
    fn a_given_note_ends_the_story_unless_asked_about() {
        let mut story = Story::answered("explain", "Adds two numbers.");
        assert_eq!(story.current(), Some(ItemId(0)));
        story.accept();
        assert_eq!(story.settle(), Settled::Finished);
        let mut story = Story::answered("explain", "Adds two numbers.");
        story.follow_up("and if they overflow?".into());
        assert_eq!(arrived(&mut story, NOTE), Settled::At(ItemId(1)));
    }

    #[test]
    fn items_are_asked_for_ahead_assuming_each_is_accepted() {
        let mut story = Story::new("implement it");
        assert!(story.prefetch().is_none(), "nothing to build on yet");
        arrived(&mut story, FIX);
        let first = story.prefetch().expect("the next item, ahead");
        assert_eq!(first.after, None);
        assert!(first.followups[0].decided[0].starts_with("accepted: an edit"), "as if the edit were accepted");
        assert!(story.prefetch().is_none(), "one at a time: the next builds on this one");
        story.ahead_text(first.id, HINTS);
        story.ahead_finished(first.id, None);
        let second = story.prefetch().expect("and the one after");
        assert_eq!((second.after, second.followups.len(), second.accepted.len()), (Some(first.id), 2, 2));
        story.accept();
        assert_eq!(story.settle(), Settled::At(ItemId(1)), "accepting brings the next item at once");
        assert_eq!(story.take_adopted(), Some(first.id));
        assert!(story.wants(second.id), "what was asked after it is still wanted");
        story.ahead_text(second.id, NOTE);
        story.follow_up("no hints".into());
        assert!(story.is_writing(), "a redirect: the next is asked for anew");
        assert!(!story.wants(second.id), "and what was asked ahead is dropped");
    }

    #[test]
    fn an_item_asked_ahead_can_still_be_streaming_when_it_is_needed() {
        let mut story = Story::new("implement it");
        arrived(&mut story, FIX);
        let next = story.prefetch().unwrap();
        story.ahead_text(next.id, &HINTS[..HINTS.len() - 12]);
        story.accept();
        assert_eq!(story.settle(), Settled::At(ItemId(1)), "it shows as it streams");
        story.ahead_text(next.id, &HINTS[HINTS.len() - 12..]);
        story.ahead_finished(next.id, None);
        assert!(!story.is_writing());
        assert!(matches!(story.current_item(), Some(Item::Step(s)) if s.complete));
    }

    #[test]
    fn done_may_update_the_notes() {
        let mut story = Story::new("implement it");
        arrived(&mut story, "<done>\n<memory>\nPrefers tabs.\n</memory>\n</done>\n");
        assert_eq!(story.memory_update().as_deref(), Some("Prefers tabs."));
    }
}
