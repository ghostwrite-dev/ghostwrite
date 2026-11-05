//! The harness every model is asked in: a reply goes on to the review unless
//! it's a `<tool>` block, which is run here and its results sent back.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::{Value, json};

use super::Listener;
use crate::attach::Attachment;
use crate::context::{Request, Role};
use crate::{protocol, tools};

/// Rounds of looking around before the model has to answer.
const MAX_TOOL_ROUNDS: usize = 12;
/// The most of files a conversation is shown: in base64, about what a request can hold.
const MAX_SHOWN: usize = 24 << 20;
pub(super) const END_TOOL: &str = "</tool>";

/// One way of having a model reply. `next` is the turn after a tool block: that
/// block, as it's to stand in the conversation, and what it returned.
pub(super) trait Replier {
    fn reply(&mut self, next: Option<Turn<'_>>, listener: &mut dyn Listener, cancel: &AtomicBool) -> Result<(), String>;
    /// What one command may show.
    fn room(&self) -> usize;
    /// The next reply has to be an item, if that can be enforced.
    fn must_answer(&mut self) {}
    /// Whether it can be shown images and PDFs.
    fn sees_files(&self) -> bool {
        false
    }
}

/// A turn of looking: the block the model wrote, what came of it, and the files it read
/// that it's to see.
#[derive(Clone, Copy)]
pub(super) struct Turn<'a> {
    pub(super) block: &'a str,
    pub(super) result: &'a str,
    pub(super) files: &'a [Attachment],
}

impl Turn<'_> {
    /// What came of it, as a message says it: the files come before the words about them.
    pub(super) fn content(&self) -> Value {
        if self.files.is_empty() {
            return json!(self.result);
        }
        let mut blocks: Vec<Value> = self.files.iter().map(Attachment::block).collect();
        blocks.push(json!({ "type": "text", "text": self.result }));
        Value::Array(blocks)
    }
}

/// The harness, the same for every model: its replies go to the review until
/// one is a `<tool>` block, whose commands are run here, all at once, for the
/// next turn to show.
pub(super) fn converse(replier: &mut dyn Replier, req: &Request, listener: &mut dyn Listener, cancel: &AtomicBool) -> Result<(), String> {
    let room = replier.room();
    let mut seen = std::collections::HashSet::new();
    let mut looked = 0;
    let mut next: Option<(String, String, Vec<Attachment>)> = None;
    let sees = replier.sees_files();
    let mut shown = 0;
    // What the user let it read outside the project.
    let mut allowed: Vec<PathBuf> = vec![];
    for round in 0..MAX_TOOL_ROUNDS {
        let mut reply = ToolFilter::new(listener, req.root.is_some());
        replier.reply(next.as_ref().map(|(block, result, files)| Turn { block, result, files }), &mut reply, cancel)?;
        let block = reply.finish();
        if cancel.load(Ordering::SeqCst) {
            return Ok(());
        }
        let Some(block) = block else { return Ok(()) };
        let Some(root) = &req.root else { return Err("there's no project to look in".into()) };
        let commands = tools::commands(&block);
        listener.looking(commands.iter().map(|c| tools::describe(c)).collect::<Vec<_>>().join(", "));
        let fresh: Vec<&str> = commands.iter().copied().filter(|c| seen.insert(c.to_string())).collect();
        // Outside the project, the user's asked first, one at a time.
        for path in fresh.iter().filter_map(|command| tools::outside(root, command)) {
            if !allowed.iter().any(|ok| path.starts_with(ok)) && listener.may_read(&path) {
                allowed.push(path);
            }
            if cancel.load(Ordering::SeqCst) {
                return Ok(());
            }
        }
        let allowed = &allowed;
        let looked_at: Vec<tools::Looked> = std::thread::scope(|scope| {
            let running: Vec<_> = fresh.iter().map(|command| scope.spawn(move || tools::look(root, command, room, sees, allowed))).collect();
            running.into_iter().map(|r| r.join().unwrap_or(tools::Looked { text: String::new(), file: None })).collect()
        });
        let mut files = vec![];
        let mut outputs = vec![];
        for tools::Looked { text, file } in looked_at {
            match file {
                // What's been shown stays in the conversation, which has to fit in a request.
                Some(file) if shown + file.data.len() > MAX_SHOWN => {
                    outputs.push(format!("That's more files than can be shown at once: {} MB so far. Go on from what you've seen.", shown >> 20))
                }
                Some(file) => {
                    shown += file.data.len();
                    files.push(file);
                    outputs.push(text);
                }
                None => outputs.push(text),
            }
        }
        looked += outputs.iter().map(String::len).sum::<usize>();
        // A model going in circles, or out of room, is told to stop looking; if it can be, made to.
        let answer_now = fresh.is_empty() || looked >= 2 * room || round + 2 >= MAX_TOOL_ROUNDS;
        if answer_now {
            replier.must_answer();
        }
        let results: String =
            fresh.iter().zip(&outputs).map(|(command, output)| format!("<result of=\"{command}\">\n{}\n</result>\n", output.trim_end())).collect();
        let what = if fresh.is_empty() { "You've already seen what those return.\n" } else { "" };
        let then = if answer_now {
            "Now reply with one item, from what you've seen."
        } else {
            "Now reply with one item, or another <tool> block if you still need to look."
        };
        next = Some((format!("<tool>\n{}\n{END_TOOL}", commands.join("\n")), format!("{results}{what}{then}"), files));
    }
    Err("the model kept looking around without answering".into())
}

/// A reply on its way to the review, unless it's a `<tool>` block: words before
/// that go through, the block doesn't, and an item's lines are never taken for
/// one (it may be code about tags). Only a line that might still become one
/// waits, so an item isn't held up.
struct ToolFilter<'a> {
    listener: &'a mut dyn Listener,
    pending: String,
    block: Option<String>,
    /// Nothing more can be a block: there are no tools, or an item has started.
    through: bool,
    mid_line: bool,
}

impl<'a> ToolFilter<'a> {
    fn new(listener: &'a mut dyn Listener, tools: bool) -> Self {
        Self { listener, pending: String::new(), block: None, through: !tools, mid_line: false }
    }

    /// The block, if the reply was one. What's after its end isn't for anyone.
    fn finish(mut self) -> Option<String> {
        if let Some(block) = self.block.take() {
            let end = block.find(END_TOOL).unwrap_or(block.len());
            return Some(block[..end].to_string());
        }
        let rest = std::mem::take(&mut self.pending);
        if !rest.is_empty() {
            self.listener.text(&rest);
        }
        None
    }

    fn forward(&mut self, upto: usize) {
        let text: String = self.pending.drain(..upto).collect();
        self.mid_line = !text.ends_with('\n');
        self.listener.text(&text);
    }
}

impl Listener for ToolFilter<'_> {
    fn used(&mut self, input: u64, output: u64) {
        self.listener.used(input, output);
    }

    fn text(&mut self, chunk: &str) {
        if self.through {
            return self.listener.text(chunk);
        }
        if let Some(block) = &mut self.block {
            return block.push_str(chunk);
        }
        self.pending.push_str(chunk);
        while !self.pending.is_empty() {
            let end = self.pending.find('\n').map(|i| i + 1);
            if self.mid_line {
                self.forward(end.unwrap_or(self.pending.len()));
                continue;
            }
            let line = self.pending[..end.unwrap_or(self.pending.len())].trim_start();
            if let Some(rest) = line.strip_prefix("<tool>") {
                let start = self.pending.len() - self.pending.trim_start().len() + "<tool>".len();
                debug_assert!(rest.len() <= self.pending.len());
                self.block = Some(self.pending[start..].to_string());
                self.pending.clear();
                return;
            }
            // A tag, or the start of one: wait to see which.
            if end.is_none() && (line.is_empty() || line.starts_with('<')) {
                return;
            }
            if protocol::STARTS.iter().any(|tag| line.trim_end() == *tag) {
                self.through = true;
                let all = std::mem::take(&mut self.pending);
                return self.listener.text(&all);
            }
            self.forward(end.unwrap_or(self.pending.len()));
        }
    }

    fn looking(&mut self, what: String) {
        self.listener.looking(what);
    }

    fn session(&mut self, id: String) {
        self.listener.session(id);
    }
}

/// The whole conversation so far, as sent each turn. Turns are only ever added,
/// so what's cached of it (the model's, the API's) stays good, and only what's
/// new is read.
pub(super) fn conversation(req: &Request) -> Vec<Value> {
    req.messages().into_iter().map(|(role, text)| json!({ "role": if role == Role::User { "user" } else { "assistant" }, "content": text })).collect()
}

pub(super) fn add_turn(messages: &mut Vec<Value>, next: Option<Turn<'_>>) {
    if let Some(turn) = next {
        messages.push(json!({ "role": "assistant", "content": turn.block }));
        messages.push(json!({ "role": "user", "content": turn.content() }));
    }
}

#[cfg(test)]
mod tests {
    use super::super::LOCAL_ROOM;
    use super::super::tests::{Collect, DONE, asked_in, util_project, with_files};
    use super::*;

    #[test]
    fn a_model_that_cant_see_is_told_and_nothing_is_sent() {
        let (root, _) = with_files();
        for file in ["figure.png", "paper.pdf"] {
            let looked = tools::look(&root, &format!("read {file}"), 1000, false, &[]);
            assert!(looked.file.is_none() && looked.text.contains("this model can only read text"), "{}", looked.text);
        }
        let reading = tools::look(&root, "read util.py", 1000, false, &[]);
        assert!(reading.file.is_none() && reading.text.contains("def load"));
        assert!(tools::look(&root, "head figure.png", 1000, true, &[]).file.is_some(), "however it's read");
        assert!(tools::look(&root, "read figure.png:3", 1000, true, &[]).file.is_some());
        assert!(tools::look(&root, "read ../../outside.png", 1000, true, &[]).file.is_none(), "only what's in the project");
    }

    #[test]
    fn the_model_is_never_sent_more_than_a_request_can_hold() {
        struct Sees(Vec<usize>);
        impl Replier for Sees {
            fn reply(&mut self, next: Option<Turn<'_>>, listener: &mut dyn Listener, _cancel: &AtomicBool) -> Result<(), String> {
                match next {
                    None => listener.text("<tool>\nread a.pdf\nread b.pdf\n</tool>"),
                    Some(turn) => {
                        self.0.push(turn.files.len());
                        assert!(turn.result.contains("more files than can be shown"), "{}", turn.result);
                        listener.text("<done>\n</done>\n");
                    }
                }
                Ok(())
            }
            fn room(&self) -> usize {
                1000
            }
            fn sees_files(&self) -> bool {
                true
            }
        }
        let (root, req) = with_files();
        let big = vec![b' '; MAX_SHOWN / 4 * 3];
        for name in ["a.pdf", "b.pdf"] {
            std::fs::write(root.join(name), [&b"%PDF-1.4\n"[..], &big].concat()).unwrap();
        }
        let mut replier = Sees(vec![]);
        converse(&mut replier, &req, &mut Collect::default(), &AtomicBool::new(false)).unwrap();
        assert_eq!(replier.0, [1], "one fits, and it's said of the other");
    }

    /// A model that replies with these, in turn, noting what it's given.
    #[derive(Default)]
    struct Canned {
        replies: Vec<&'static str>,
        next: Vec<(String, String)>,
        must_answer_at: Vec<usize>,
    }

    impl Replier for Canned {
        fn reply(&mut self, next: Option<Turn<'_>>, listener: &mut dyn Listener, _cancel: &AtomicBool) -> Result<(), String> {
            if let Some(Turn { block, result, .. }) = next {
                self.next.push((block.to_string(), result.to_string()));
            }
            listener.text(self.replies.remove(0));
            Ok(())
        }
        fn room(&self) -> usize {
            LOCAL_ROOM
        }
        fn must_answer(&mut self) {
            self.must_answer_at.push(self.next.len());
        }
    }

    const READS: &str = "<tool>\nread util.py\n</tool>\n";

    #[test]
    fn a_tool_block_is_run_here_and_never_shown() {
        let root = util_project("tools-run");
        let mut model = Canned { replies: vec![READS, DONE], ..Default::default() };
        let mut out = Collect::default();
        converse(&mut model, &asked_in(&root), &mut out, &AtomicBool::new(false)).unwrap();
        assert_eq!(out.0, DONE, "the block isn't shown");
        assert_eq!(out.1, ["reading util.py"]);
        let (block, result) = &model.next[0];
        assert_eq!(block, "<tool>\nread util.py\n</tool>");
        assert!(result.contains("<result of=\"read util.py\">\n1│def load(): pass\n</result>"), "{result}");
    }

    #[test]
    fn reading_outside_the_project_is_asked_about_first() {
        /// Says yes or no, and notes what it was asked.
        struct Asks(bool, Vec<std::path::PathBuf>);
        impl Listener for Asks {
            fn text(&mut self, _: &str) {}
            fn looking(&mut self, _: String) {}
            fn may_read(&mut self, path: &std::path::Path) -> bool {
                self.1.push(path.to_path_buf());
                self.0
            }
        }
        let root = util_project("tools-asks");
        let elsewhere = ghostwrite_project::scratch_dir("tools-asks-elsewhere");
        std::fs::write(elsewhere.join("notes.txt"), "remember this\n").unwrap();
        let file = ghostwrite_project::platform::real(&elsewhere.join("notes.txt")).unwrap();
        let block: &'static str = Box::leak(format!("<tool>\nread {}\nread util.py\n</tool>\n", file.display()).into_boxed_str());
        for yes in [true, false] {
            let mut model = Canned { replies: vec![block, DONE], ..Default::default() };
            let mut asks = Asks(yes, vec![]);
            converse(&mut model, &asked_in(&root), &mut asks, &AtomicBool::new(false)).unwrap();
            assert_eq!(asks.1, std::slice::from_ref(&file), "asked once, about that file (not the project's)");
            let result = &model.next[0].1;
            assert_eq!(result.contains("remember this"), yes, "{result}");
            assert!(result.contains("def load"), "the project's file is read either way");
        }
    }

    #[test]
    fn a_model_going_in_circles_is_made_to_answer() {
        let root = util_project("tools-circles");
        let mut model = Canned { replies: vec![READS, READS, DONE], ..Default::default() };
        converse(&mut model, &asked_in(&root), &mut Collect::default(), &AtomicBool::new(false)).unwrap();
        assert_eq!(model.must_answer_at, [1], "the second time, not the first");
        assert!(model.next[1].1.contains("already seen"), "{}", model.next[1].1);
    }

    #[test]
    fn several_commands_all_come_back() {
        let root = util_project("tools-several");
        let mut model = Canned { replies: vec!["<tool>\nread util.py\nlist\n</tool>\n", DONE], ..Default::default() };
        converse(&mut model, &asked_in(&root), &mut Collect::default(), &AtomicBool::new(false)).unwrap();
        let result = &model.next[0].1;
        assert!(result.contains("<result of=\"read util.py\">") && result.contains("<result of=\"list\">"), "{result}");
    }

    /// Forwards what's streamed, to see just how it's split.
    fn filtered(chunks: &[&str]) -> (String, Option<String>) {
        let mut out = Collect::default();
        let mut filter = ToolFilter::new(&mut out, true);
        for chunk in chunks {
            filter.text(chunk);
        }
        let block = filter.finish();
        (out.0, block)
    }

    #[test]
    fn only_a_tool_block_is_held_back() {
        assert_eq!(filtered(&["<to", "ol>\nread a.py\n", "</tool>\n"]), (String::new(), Some("\nread a.py\n".into())));
        assert_eq!(filtered(&["<tool>read a.py</tool>"]), (String::new(), Some("read a.py".into())), "all on a line");
        assert_eq!(filtered(&["<tool>\nread a.py\n</tool>\nand then it made up the result"]).1, Some("\nread a.py\n".into()));
        assert_eq!(filtered(&["Let me look.\n", "<tool>\nls\n</tool>\n"]), ("Let me look.\n".into(), Some("\nls\n".into())), "words before it go on");
        assert_eq!(filtered(&["<file>\n", "a.py\n</file>\n"]), ("<file>\na.py\n</file>\n".into(), None));
        assert_eq!(filtered(&["<file>\na.py\n</file>\n<new>\n<tool>\n</new>\n"]).1, None, "code in an item isn't a block");
        assert_eq!(filtered(&["<sa"]), ("<sa".into(), None), "what's waiting isn't lost");
    }

    #[test]
    fn an_item_isnt_held_up_once_its_started() {
        let mut out = Collect::default();
        let mut filter = ToolFilter::new(&mut out, true);
        filter.text("<file>\n");
        filter.text("half a li");
        drop(filter);
        assert_eq!(out.0, "<file>\nhalf a li");
    }
}
