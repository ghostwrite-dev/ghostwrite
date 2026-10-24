//! A story: one request to Claude and its replies, reviewed as a single list
//! you Tab through. Edit steps are accepted or skipped; notes (answers,
//! walkthroughs) are read.
//!
//! Whatever you say mid-review is a follow-up; Claude decides what it is (a
//! question, a comment, a redirect) and what it changes. Its reply comes in
//! two parts: what answers you, shown right where you are, and, when what you
//! said changes what should come next, a revised `<rest>` that replaces the
//! item you were at (unless it's a note you've read) and everything after.
//! Without one, the rest stays. While the reply is on its way the review
//! waits where you are.
//!
//! A reply Ghostwrite can't use (a step whose lines aren't in its file, or
//! nothing in the reply format) goes back to Claude the same way, as a
//! correction, until it's right or [`MAX_REVISIONS`] tries in a row fail.
//!
//! This is the bookkeeping only. Applying an accepted step to a buffer, or
//! creating a file, is the editor's job; it tells the story afterwards with
//! [`Story::decide`].

use std::collections::HashMap;

use crate::protocol::{self, Kind, Note, Piece, Step};

/// Corrections Claude gets in a row before a step is left for the user to skip.
pub const MAX_REVISIONS: usize = 3;

/// Which part of a reply an item is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Part {
    /// What answers a follow-up, shown where it was asked.
    Answer,
    /// The plan: a first reply, or a follow-up's revised rest.
    Plan,
}

/// An item to review: which reply, which part of it, and where in that part.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ItemId {
    pub reply: usize,
    pub part: Part,
    pub index: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Item {
    Step(Step),
    Note(Note),
    /// A new file, or a folder if it ends in `/`.
    Create(String),
    /// A file or folder to move to the Trash.
    Remove(String),
    /// A file or folder to move.
    Rename {
        from: String,
        to: String,
    },
}

impl Item {
    /// Decided in the file tree rather than in a buffer.
    pub fn is_file_op(&self) -> bool {
        matches!(self, Item::Create(_) | Item::Remove(_) | Item::Rename { .. })
    }

    /// The item as Claude wrote it, to tell it where the user is.
    pub fn describe(&self) -> String {
        match self {
            Item::Note(note) => {
                let at = if note.at.is_empty() { String::new() } else { format!("(about: {})\n", note.at.join("\n")) };
                format!("a note: {at}{}", note.text)
            }
            Item::Step(step) => {
                let file = step.file.as_ref().map_or(String::new(), |f| format!(" in {f}"));
                format!("a step{file}, replacing\n{}\nwith\n{}", step.old.join("\n"), step.new.join("\n"))
            }
            Item::Create(path) => format!("creating {path}"),
            Item::Remove(path) => format!("removing {path}"),
            Item::Rename { from, to } => format!("renaming {from} to {to}"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    Pending,
    Accepted,
    Rejected,
}

/// Where the review is after a change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Settled {
    At(ItemId),
    /// Nothing to review until more of the reply arrives (or after an error).
    Waiting,
    Finished,
}

/// One turn of the conversation: Claude's latest reply, what the user did
/// with it, and what they said next.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FollowUp {
    /// Claude's reply just before this.
    pub reply: String,
    /// What the user accepted and skipped since (`accepted: …`, `skipped: …`).
    pub decided: Vec<String>,
    /// The item they were at, as Claude wrote it.
    pub at: Option<String>,
    pub said: String,
    /// Ghostwrite, not the user, is speaking: Claude's reply couldn't be used.
    pub correction: bool,
    /// A `:sh` command the user ran since that reply, with its output.
    pub command: Option<String>,
}

#[derive(Debug)]
struct Reply {
    text: String,
    /// Where in the list it was asked: its answer goes here.
    at: usize,
    /// The item it was asked about.
    about: Option<ItemId>,
    /// It answers a correction: all of it is the new plan from there.
    correction: bool,
}

#[derive(Debug)]
pub struct Story {
    pub instruction: String,
    history: Vec<FollowUp>,
    /// Decisions since the last follow-up, for the next one.
    decided: Vec<String>,
    replies: Vec<Reply>,
    verdicts: HashMap<ItemId, Verdict>,
    current: Option<ItemId>,
    /// The reply Claude is writing.
    writing: Option<usize>,
    pub error: Option<String>,
    /// What Claude is looking at in the project right now.
    pub looking: Option<String>,
    /// Corrections sent since the user last decided something.
    revisions: usize,
}

/// The items in some reply text: notes for an answer, pieces for a change.
fn items_in(text: &str) -> Vec<Item> {
    match protocol::reply_kind(text) {
        Some(Kind::Question) => protocol::parse_notes(text).into_iter().map(Item::Note).collect(),
        Some(Kind::Edit) => protocol::parse_pieces(text)
            .into_iter()
            .map(|p| match p {
                Piece::Step(s) => Item::Step(s),
                Piece::Create(path) => Item::Create(path),
                Piece::Remove(path) => Item::Remove(path),
                Piece::Rename { from, to } => Item::Rename { from, to },
            })
            .collect(),
        None => vec![],
    }
}

impl Story {
    /// A request on its way: the plan will arrive in the first reply.
    pub fn new(instruction: impl Into<String>) -> Self {
        Self {
            instruction: instruction.into(),
            history: vec![],
            decided: vec![],
            replies: vec![Reply { text: String::new(), at: 0, about: None, correction: false }],
            verdicts: HashMap::new(),
            current: None,
            writing: Some(0),
            error: None,
            looking: None,
            revisions: 0,
        }
    }

    /// A story that starts with an answer already there (docs from `space k`).
    pub fn answered(instruction: impl Into<String>, answer: &str) -> Self {
        let mut story = Self::new(instruction);
        story.replies[0].text = format!("<answer>\n{answer}\n</answer>");
        story.writing = None;
        story.settle();
        story
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

    /// Reply `k`'s two parts: what answers now, and the plan (all of a first reply).
    fn parts(&self, k: usize) -> (Vec<Item>, Option<Vec<Item>>) {
        let text = &self.replies[k].text;
        if k == 0 {
            return (vec![], Some(items_in(text)));
        }
        let (now, rest) = protocol::split_rest(text);
        match rest {
            // Asked for only a <rest> and given the steps without one: they still replace.
            None if self.replies[k].correction => (vec![], Some(items_in(text))),
            rest => (items_in(&now), rest.map(|r| items_in(&r))),
        }
    }

    /// Everything to review, in order. Each follow-up's answer goes where it
    /// was asked; its rest (if it has one) replaces what came after.
    pub fn items(&self) -> Vec<(ItemId, Item)> {
        let mut order: Vec<(ItemId, Item)> = vec![];
        for (k, reply) in self.replies.iter().enumerate() {
            let (now, rest) = self.parts(k);
            let ids = |part, items: Vec<Item>| items.into_iter().enumerate().map(move |(index, item)| (ItemId { reply: k, part, index }, item));
            let after = order.split_off(reply.at.min(order.len()));
            order.extend(ids(Part::Answer, now));
            match rest {
                Some(rest) => order.extend(ids(Part::Plan, rest)),
                None => order.extend(after),
            }
        }
        order
    }

    pub fn item(&self, id: ItemId) -> Option<Item> {
        self.items().into_iter().find(|(i, _)| *i == id).map(|(_, item)| item)
    }

    pub fn current_item(&self) -> Option<Item> {
        self.item(self.current?)
    }

    /// The current item's place in the list (zero-based) and the list's length.
    pub fn progress(&self) -> Option<(usize, usize)> {
        let items = self.items();
        Some((items.iter().position(|(id, _)| Some(*id) == self.current)?, items.len()))
    }

    /// The item a reply was asked about, if it was.
    pub fn asked_about(&self, id: ItemId) -> Option<ItemId> {
        self.replies.get(id.reply)?.about
    }

    /// What the reply being written was asked about, while a follow-up is on its way.
    pub fn writing_about(&self) -> Option<ItemId> {
        self.writing.filter(|&k| k > 0).and_then(|k| self.replies[k].about)
    }

    pub fn verdict(&self, id: ItemId) -> Verdict {
        self.verdicts.get(&id).copied().unwrap_or(Verdict::Pending)
    }

    /// The user says something about the current item (or the reply so far).
    /// The answer streams into a new reply, which may revise what comes after.
    pub fn follow_up(&mut self, said: String) {
        self.revisions = 0;
        self.follow(said, false);
    }

    /// Whether Claude may still be asked to correct its reply.
    pub fn can_revise(&self) -> bool {
        self.revisions < MAX_REVISIONS
    }

    /// The current step's `<old>` lines aren't in its file (`file`, whose
    /// text is `text` unless it's the file asked about, which Claude is
    /// shown anyway): Claude rewrites it and what follows.
    pub fn step_misplaced(&mut self, file: &str, text: Option<&str>) {
        self.revise(protocol::misplaced_step(file, text));
    }

    /// The latest reply is complete but there's nothing in it to review:
    /// Claude writes it again, in the format.
    pub fn reply_unusable(&mut self) {
        self.revise(protocol::NOTHING_TO_REVIEW.into());
    }

    /// Whether the last reply, now complete, has nothing to review and isn't
    /// an intended empty revision (`<rest></rest>`) or just notes for later.
    pub fn is_unusable(&self) -> bool {
        let Some(reply) = self.replies.last().filter(|_| self.writing.is_none() && self.error.is_none()) else { return false };
        let text = &reply.text;
        let kept = protocol::split_rest(text).1.is_some() || protocol::memory_update(text).is_some();
        !text.trim().is_empty() && !kept && protocol::reply_kind(text).is_none()
    }

    fn revise(&mut self, said: String) {
        self.revisions += 1;
        self.follow(said, true);
    }

    fn follow(&mut self, said: String, correction: bool) {
        let items = self.items();
        let here = self.current.and_then(|c| items.iter().position(|(id, _)| *id == c));
        let item = here.map(|i| &items[i].1);
        // The answer goes where the user is. A note there has been read
        // (it's behind them); anything still to decide comes after the answer.
        let read = matches!(item, Some(Item::Note(_)));
        let at = here.map_or(items.len(), |i| i + usize::from(read));
        if let (true, Some(id)) = (read, self.current) {
            self.verdicts.insert(id, Verdict::Accepted);
        }
        let reply = self.replies.last().map(|r| r.text.clone()).unwrap_or_default();
        self.history.push(FollowUp { reply, decided: std::mem::take(&mut self.decided), at: item.map(Item::describe), said, correction, command: None });
        self.replies.push(Reply { text: String::new(), at, about: self.current, correction });
        self.error = None;
        self.current = None;
        self.writing = Some(self.replies.len() - 1);
    }

    /// The user ran `command` (with its output) since Claude's last reply:
    /// the newest follow-up says so.
    pub fn ran(&mut self, command: String) {
        if let Some(followup) = self.history.last_mut() {
            followup.command = Some(command);
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

    /// The current item was accepted (and applied) or skipped.
    pub fn decide(&mut self, verdict: Verdict) {
        let Some(id) = self.current.take() else { return };
        self.revisions = 0;
        // Reading a note isn't a decision worth telling Claude about.
        if let Some(item) = self.item(id).filter(|i| !matches!(i, Item::Note(_))) {
            let what = if verdict == Verdict::Accepted { "accepted" } else { "skipped" };
            self.decided.push(format!("{what}: {}", item.describe()));
        }
        self.verdicts.insert(id, verdict);
    }

    /// What Claude said, in brief, for the log of interactions: its notes
    /// (cut short), or the changes it proposed and how many were applied.
    pub fn recap(&self) -> String {
        const MAX: usize = 600;
        let items = self.items();
        let notes: Vec<String> = items.iter().filter_map(|(_, i)| if let Item::Note(n) = i { Some(n.text.replace('\n', " ")) } else { None }).collect();
        let mut files: Vec<String> = items
            .iter()
            .filter_map(|(_, i)| match i {
                Item::Step(s) => Some(s.file.clone().unwrap_or_else(|| "the file asked about".into())),
                Item::Create(p) | Item::Remove(p) => Some(p.clone()),
                Item::Rename { from, .. } => Some(from.clone()),
                Item::Note(_) => None,
            })
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

    /// Move on to the next item to review, or finish when nothing is left.
    /// While a follow-up's reply is on its way, only what it brings is up
    /// next: it may change everything after it, so the review waits for it.
    pub fn settle(&mut self) -> Settled {
        let items = self.items();
        let answering = self.writing.filter(|&k| k > 0 && !protocol::rest_settled(&self.replies[k].text));
        let pending = |id: ItemId| self.verdict(id) == Verdict::Pending && answering.is_none_or(|k| id.reply == k);
        if self.current.is_none_or(|c| !items.iter().any(|(id, _)| *id == c) || !pending(c)) {
            self.current = items.iter().map(|(id, _)| *id).find(|&id| pending(id));
        }
        match self.current {
            Some(id) => Settled::At(id),
            None if self.writing.is_none() && self.error.is_none() => Settled::Finished,
            None => Settled::Waiting,
        }
    }

    /// Changes accepted, of changes proposed (in what's still the plan).
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

    /// Claude's rewritten notes, if a reply ended with them.
    pub fn memory_update(&self) -> Option<String> {
        self.replies.iter().rev().find_map(|r| protocol::memory_update(&r.text))
    }

    /// The request and every reply, kept for when a step won't place.
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

#[cfg(test)]
mod tests {
    use super::*;

    const TWO_STEPS: &str = "<step>\n<old>\n    pass\n</old>\n<new>\n    return a + b\n</new>\n</step>\n<step>\n<old>\ndef add(a, b):\n</old>\n<new>\ndef add(a: int, b: int) -> int:\n</new>\n</step>\n";
    const TOUR: &str = "<answer>\n<note>\nFirst.\n</note>\n<note>\nSecond.\n</note>\n<note>\nThird.\n</note>\n</answer>\n";

    fn plan(index: usize) -> ItemId {
        ItemId { reply: 0, part: Part::Plan, index }
    }

    fn arrived(story: &mut Story, reply: &str) -> Settled {
        story.push_text(reply);
        story.finished_writing();
        story.settle()
    }

    fn texts(story: &Story) -> Vec<String> {
        story
            .items()
            .into_iter()
            .map(|(_, item)| match item {
                Item::Note(n) => n.text,
                Item::Step(s) => s.new.join(" "),
                other => other.describe(),
            })
            .collect()
    }

    #[test]
    fn steps_are_decided_in_order_and_the_last_finishes() {
        let mut story = Story::new("implement it");
        assert_eq!(story.settle(), Settled::Waiting);
        assert_eq!(arrived(&mut story, TWO_STEPS), Settled::At(plan(0)));
        story.decide(Verdict::Accepted);
        assert_eq!(story.settle(), Settled::At(plan(1)));
        story.decide(Verdict::Rejected);
        assert_eq!(story.settle(), Settled::Finished);
        assert_eq!(story.outcome(), "applied 1 of 2 steps");
    }

    #[test]
    fn steps_show_up_while_streaming() {
        let mut story = Story::new("implement it");
        story.push_text(&TWO_STEPS[..40]);
        assert_eq!(story.settle(), Settled::At(plan(0)));
        assert!(matches!(story.current_item(), Some(Item::Step(s)) if !s.complete));
    }

    #[test]
    fn a_question_mid_plan_is_answered_before_the_next_step() {
        let mut story = Story::new("implement it");
        arrived(&mut story, TWO_STEPS);
        story.decide(Verdict::Accepted);
        story.settle();
        story.follow_up("why int?".into());
        assert_eq!(story.writing_about(), Some(plan(1)), "the answer is on its way to where it was asked");
        assert!(story.followups()[0].at.as_deref().is_some_and(|at| at.contains(") -> int:")), "Claude is told where the user is");
        assert_eq!(story.followups()[0].decided.len(), 1, "and what they accepted");
        assert!(story.followups()[0].decided[0].starts_with("accepted: a step"));
        assert_eq!(story.followups()[0].reply, TWO_STEPS, "and the latest reply, whole");
        let answer = ItemId { reply: 1, part: Part::Answer, index: 0 };
        assert_eq!(arrived(&mut story, "<answer>\n<note>\nBecause both are ints.\n</note>\n</answer>\n"), Settled::At(answer));
        assert_eq!(story.asked_about(answer), Some(plan(1)));
        story.decide(Verdict::Accepted);
        assert_eq!(story.settle(), Settled::At(plan(1)), "back to the plan, unchanged");
    }

    #[test]
    fn the_review_waits_for_the_answer() {
        let mut story = Story::new("walk me through this");
        arrived(&mut story, TOUR);
        story.follow_up("is there a size limit?".into());
        assert_eq!(story.settle(), Settled::Waiting, "not on to the next note while Claude answers");
        story.push_text("<answer>\nYes, 1 MB.\n");
        story.settle();
        assert!(matches!(story.current_item(), Some(Item::Note(n)) if n.text == "Yes, 1 MB."), "the answer, as it streams");
        story.decide(Verdict::Accepted);
        assert_eq!(story.settle(), Settled::Waiting, "the rest may still change");
        story.push_text("</answer>\n");
        story.finished_writing();
        assert!(matches!(story.settle(), Settled::At(id) if id == plan(1)), "then on with the walkthrough");

        // Notes after the answer can't change the rest: no need to wait for them.
        let mut story = Story::new("walk me through this");
        arrived(&mut story, TOUR);
        story.follow_up("ok".into());
        story.push_text("<answer>\nGood.\n</answer>\n");
        story.settle();
        story.decide(Verdict::Accepted);
        assert_eq!(story.settle(), Settled::Waiting);
        story.push_text("<memory>\nLikes brevity.");
        assert!(matches!(story.settle(), Settled::At(id) if id == plan(1)), "on while Claude writes its notes");
    }

    #[test]
    fn a_redirected_step_is_replaced_by_the_rest() {
        let mut story = Story::new("implement it");
        arrived(&mut story, TWO_STEPS);
        story.follow_up("use sum()".into());
        let corrected = "<rest>\n<step>\n<old>\n    pass\n</old>\n<new>\n    return sum((a, b))\n</new>\n</step>\n</rest>\n";
        arrived(&mut story, corrected);
        assert_eq!(texts(&story), ["    return sum((a, b))"], "the old steps are gone");
        story.decide(Verdict::Accepted);
        assert_eq!(story.settle(), Settled::Finished);
        assert_eq!(story.summary(), (1, 1));
    }

    #[test]
    fn a_comment_mid_walkthrough_revises_the_rest() {
        let mut story = Story::new("walk me through this");
        arrived(&mut story, TOUR);
        story.decide(Verdict::Accepted);
        story.settle();
        // At "Second": the user steers the walkthrough.
        story.follow_up("skip the setup, show me the error handling".into());
        let reply = "<answer>\n<note>\nSure, focusing on errors.\n</note>\n</answer>\n<rest>\n<answer>\n<note>\nErrors start here.\n</note>\n<note>\nAnd end here.\n</note>\n</answer>\n</rest>\n";
        arrived(&mut story, reply);
        assert_eq!(texts(&story), ["First.", "Second.", "Sure, focusing on errors.", "Errors start here.", "And end here."], "Third is gone");
        assert!(matches!(story.current_item(), Some(Item::Note(n)) if n.text.starts_with("Sure")));
    }

    #[test]
    fn a_question_without_a_new_rest_keeps_the_rest() {
        let mut story = Story::new("walk me through this");
        arrived(&mut story, TOUR);
        story.follow_up("what's First about?".into());
        arrived(&mut story, "<answer>\nSetup.\n</answer>\n");
        assert_eq!(texts(&story), ["First.", "Setup.", "Second.", "Third."]);
    }

    #[test]
    fn an_edit_asked_mid_walkthrough_comes_next_and_the_walkthrough_goes_on() {
        let mut story = Story::new("walk me through this");
        arrived(&mut story, TOUR);
        story.follow_up("implement it".into());
        arrived(&mut story, "<step>\n<old>\n    pass\n</old>\n<new>\n    return 1\n</new>\n</step>\n");
        assert_eq!(texts(&story), ["First.", "    return 1", "Second.", "Third."]);
    }

    #[test]
    fn a_revised_plan_can_change_steps_after_a_question() {
        let mut story = Story::new("implement it");
        arrived(&mut story, TWO_STEPS);
        story.decide(Verdict::Accepted);
        story.settle();
        story.follow_up("actually keep it untyped".into());
        arrived(&mut story, "<answer>\nOK, no types.\n</answer>\n<rest>\n</rest>\n");
        assert_eq!(texts(&story), ["    return a + b", "OK, no types."], "the typing step is dropped");
        assert_eq!(story.summary(), (1, 1));
    }

    #[test]
    fn errors_wait_and_memory_is_picked_up() {
        let mut story = Story::new("x");
        story.failed("offline".into());
        assert_eq!(story.settle(), Settled::Waiting, "an error stays on screen");
        let mut story = Story::answered("docs", "It adds.");
        assert_eq!(story.settle(), Settled::At(plan(0)));
        story.follow_up("remember that I like doctests".into());
        arrived(&mut story, "<answer>\nNoted.\n</answer>\n<memory>\nLikes doctests.\n</memory>\n");
        assert_eq!(story.memory_update().as_deref(), Some("Likes doctests."));
        assert_eq!(story.outcome(), "answered");
        assert_eq!(story.recap(), "It adds. / Noted.");
        let mut story = Story::new("implement it");
        arrived(&mut story, TWO_STEPS);
        story.decide(Verdict::Accepted);
        assert_eq!(story.recap(), "proposed 2 change(s) in the file asked about, 1 applied");
    }
}
