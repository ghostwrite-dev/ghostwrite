//! A local model in the harness: held to the format by a grammar, and sent as
//! little as fits its small context.

use std::sync::atomic::AtomicBool;

use serde_json::{Value, json};

use super::converse::{Replier, Turn, add_turn, conversation};
use super::{LOCAL_ROOM, Listener};
use crate::context::Request;
use crate::protocol;

/// A reply is one small step: one that runs on stops here. It has plenty of room,
/// and a small model doesn't get to ramble.
const LOCAL_MAX_TOKENS: usize = 400;
/// Turns kept whole for a local model. Every step is small, and what was accepted is
/// in the files it's shown: earlier replies shrink to a line each.
const LOCAL_TURNS_KEPT: usize = 2;
/// How far what's sent is shrunk, in turn, to fit a local model's context.
const LOCAL_SHRINKS: [usize; 4] = [1, 2, 4, 8];

/// The conversation with only its last turns whole. It starts the same every
/// time, so what's cached of the start stays good.
fn recent(req: &Request) -> Vec<Value> {
    let mut messages = conversation(req);
    let turns = req.followups.len();
    if turns <= LOCAL_TURNS_KEPT {
        return messages;
    }
    let dropped = &req.followups[..turns - LOCAL_TURNS_KEPT];
    let earlier: Vec<String> = dropped.iter().flat_map(|f| &f.decided).map(|d| format!("- {}", d.trim())).collect();
    messages.drain(1..1 + 2 * dropped.len());
    if !earlier.is_empty() {
        let next = messages[2]["content"].as_str().unwrap_or_default();
        messages[2]["content"] = json!(format!("Before that, the user decided:\n{}\n\n{next}", earlier.join("\n")));
    }
    messages
}

/// Ask with what's sent shrunk as far as it takes to fit the model's context, so
/// that never comes to the user: however big the project, a small step's prompt fits.
pub(super) fn fitting(
    req: &Request,
    listener: &mut dyn Listener,
    mut ask: impl FnMut(&Request, &mut dyn Listener) -> Result<(), String>,
) -> Result<(), String> {
    let mut result = Ok(());
    for small in LOCAL_SHRINKS {
        result = ask(&Request { small, ..req.clone() }, listener);
        if !result.as_ref().is_err_and(|why| why.contains(protocol::NO_ROOM)) {
            return result;
        }
    }
    result
}

/// A local model, whose grammar holds it to the format.
pub(super) struct LocalChat<'a> {
    model: &'a str,
    messages: Vec<Value>,
    /// The files it may name: its grammar has them, so it can't make one up.
    files: Vec<String>,
    tools: bool,
    /// It's only being asked something.
    question: bool,
}

impl<'a> LocalChat<'a> {
    pub(super) fn new(model: &'a str, req: &Request) -> Self {
        let mut files = req.root.as_ref().map_or(vec![], |root| ghostwrite_project::files::files(root, root));
        files.extend(
            req.root.as_ref().and_then(|root| req.path.as_ref()?.strip_prefix(root).ok()).map(|p| p.display().to_string()).filter(|p| !files.contains(p)),
        );
        let mut messages = vec![json!({ "role": "system", "content": protocol::system() })];
        messages.extend(recent(req));
        let said = req.followups.iter().rev().filter(|f| !f.correction).map(|f| f.said.trim()).find(|s| !s.is_empty());
        Self { model, messages, files, tools: req.root.is_some(), question: protocol::is_question(said.unwrap_or(&req.instruction)) }
    }

    fn request(&self) -> Value {
        json!({ "messages": self.messages, "grammar": protocol::grammar(&self.files, self.tools, self.question), "temperature": 0.2, "max_tokens": LOCAL_MAX_TOKENS })
    }
}

impl Replier for LocalChat<'_> {
    fn reply(&mut self, next: Option<Turn<'_>>, listener: &mut dyn Listener, cancel: &AtomicBool) -> Result<(), String> {
        add_turn(&mut self.messages, next);
        crate::local::ask(self.model, self.request(), listener, cancel)
    }

    fn room(&self) -> usize {
        LOCAL_ROOM
    }

    fn must_answer(&mut self) {
        self.tools = false;
    }
}

#[cfg(test)]
mod tests {
    use super::super::Backend;
    use super::super::tests::{Collect, asked, asked_about_another_file, asked_in, two_file_project, util_project};
    use super::*;
    use crate::models;
    use crate::protocol::SYSTEM;

    #[test]
    fn a_local_models_request_holds_it_to_the_format() {
        let root = util_project("tools-grammar");
        let req = asked_in(&root);
        let mut chat = LocalChat::new("a model", &req);
        let request = chat.request();
        assert_eq!(request["messages"][0]["content"], SYSTEM);
        assert_eq!(request["messages"][1]["role"], "user");
        let grammar = request["grammar"].as_str().unwrap();
        assert!(grammar.contains("\"util.py\"") && grammar.contains("tool ::="), "{grammar}");
        chat.must_answer();
        assert!(!chat.request()["grammar"].as_str().unwrap().contains("tool ::="), "it can only answer");
        // What's cached is only ever added to.
        let before = chat.messages.clone();
        add_turn(&mut chat.messages, Some(Turn { block: "<tool>\nlist\n</tool>", result: "results", files: &[] }));
        assert_eq!(chat.messages[..before.len()], before[..]);
    }

    #[test]
    fn a_local_model_gets_earlier_steps_as_a_line_each() {
        let turn = |n: usize| crate::FollowUp {
            reply: format!("reply {n}"),
            decided: vec![format!("accepted: step {n}")],
            said: String::new(),
            correction: false,
            terminal: None,
        };
        let req = Request { followups: (0..5).map(turn).collect(), ..asked() };
        let all = conversation(&req);
        let kept = recent(&req);
        assert_eq!(all.len(), 11);
        assert_eq!(kept.len(), 5, "the request, and two turns");
        assert_eq!(kept[0], all[0], "it starts the same every time");
        assert_eq!((&kept[1]["content"], &kept[3]["content"]), (&json!("reply 3"), &json!("reply 4")));
        let summary = kept[2]["content"].as_str().unwrap();
        assert!(summary.contains("- accepted: step 0") && summary.contains("- accepted: step 2"), "{summary}");
        assert!(!summary.contains("reply 0"));
    }

    #[test]
    fn what_doesnt_fit_is_shrunk_until_it_does() {
        let mut tried = vec![];
        let result = fitting(&asked(), &mut Collect::default(), |req, listener| {
            tried.push(req.small);
            if req.small < 4 {
                return Err(format!("{}: 14014 tokens", protocol::NO_ROOM));
            }
            listener.text("fits");
            Ok(())
        });
        assert_eq!((result, tried), (Ok(()), vec![1, 2, 4]));
        let mut tried = 0;
        let error = fitting(&asked(), &mut Collect::default(), |_, _| {
            tried += 1;
            Err(protocol::NO_ROOM.to_string())
        });
        assert_eq!(tried, 4, "as far as it goes");
        assert!(error.unwrap_err().contains(protocol::NO_ROOM));
        let mut tried = 0;
        let other = fitting(&asked(), &mut Collect::default(), |_, _| {
            tried += 1;
            Err("the model crashed".to_string())
        });
        assert_eq!((other.unwrap_err().as_str(), tried), ("the model crashed", 1), "only what's about room");
    }

    /// A change of several steps, each accepted as the editor does: it's applied, and
    /// the next is asked for.
    #[test]
    #[ignore]
    fn a_local_model_goes_on_step_by_step() {
        let root = ghostwrite_project::scratch_dir("live-steps");
        let path = root.join("calc.py");
        std::fs::write(&path, "def add(a, b):\n    pass\n\n\ndef sub(a, b):\n    pass\n\n\ndef mul(a, b):\n    pass\n\n\ndef div(a, b):\n    pass\n").unwrap();
        let mut followups: Vec<crate::FollowUp> = vec![];
        let mut done = 0;
        for round in 0..8 {
            let text = std::fs::read_to_string(&path).unwrap();
            let req = Request {
                path: Some(path.clone()),
                language: "python".into(),
                range: 0..text.len(),
                text: text.clone(),
                instruction: "implement all four functions".into(),
                followups: followups.clone(),
                root: Some(root.clone()),
                ..asked()
            };
            let mut out = Collect::default();
            local_model().run(&req, &mut out, &AtomicBool::new(false)).unwrap();
            println!("--- round {round} {:?}\n{}", out.1, out.0);
            if followups.iter().any(|f: &crate::FollowUp| f.reply.trim() == out.0.trim()) {
                followups.push(crate::FollowUp { reply: out.0, decided: vec![], said: protocol::SAID_ALREADY.into(), correction: true, terminal: None });
                continue;
            }
            let decided = match protocol::parse(&out.0, true) {
                protocol::Reply::Step(step) => {
                    let near = step.line.map_or(0..1, |l| l..l + step.old.len());
                    let Some(edit) = crate::placement::edit(&text, &step, near.clone()) else {
                        let said = protocol::misplaced("calc.py", &text, &step.old, near);
                        followups.push(crate::FollowUp { reply: out.0, decided: vec![], said, correction: true, terminal: None });
                        continue;
                    };
                    let mut text = text;
                    text.replace_range(edit.bytes, &edit.text);
                    std::fs::write(&path, text).unwrap();
                    done += 1;
                    vec![format!("accepted: {}", crate::Item::Step(step).describe())]
                }
                protocol::Reply::Done { .. } => break,
                other => {
                    println!("not a step: {other:?}");
                    vec![]
                }
            };
            followups.push(crate::FollowUp { reply: out.0, decided, said: String::new(), correction: false, terminal: None });
        }
        let text = std::fs::read_to_string(&path).unwrap();
        println!("=== after {done} steps\n{text}");
        assert!(["a + b", "a - b", "a * b", "a / b"].iter().all(|op| text.contains(op)), "{text}");
    }

    /// As in a data project: thousands of files, a long file, a full terminal.
    #[test]
    #[ignore]
    fn a_local_model_fits_a_big_project() {
        let root = ghostwrite_project::scratch_dir("live-big");
        for i in 0..5000 {
            std::fs::write(root.join(format!("mesh_{i:05}.off")), "x").unwrap();
        }
        let text: String = (0..20_000).map(|i| format!("def f{i}(mesh_dir, data_dir, fig_dir, mesh_ext=\".off\"):\n    return {i}\n")).collect();
        std::fs::write(root.join("big.py"), &text).unwrap();
        let at = text.len() / 2;
        let surroundings = crate::Surroundings { terminal: Some((0..50_000).map(|i| format!("output {i}\n")).collect()), ..Default::default() };
        let req = Request {
            path: Some(root.join("big.py")),
            language: "python".into(),
            range: at..at,
            text,
            instruction: "what does this function return?".into(),
            root: Some(root.clone()),
            surroundings,
            ..asked()
        };
        let mut out = Collect::default();
        local_model().run(&req, &mut out, &AtomicBool::new(false)).unwrap();
        println!("{:?}\n{}", out.1, out.0);
        assert_eq!(protocol::check(&out.0, true), Ok(()), "{}", out.0);
    }

    /// What's asked of a live local model: `GHOSTWRITE_LOCAL_MODEL`, a Hugging Face
    /// name or a `.gguf`, else the 14B one. `GHOSTWRITE_MODEL_WORKER` has to be a
    /// built ghostwrite.
    fn local_model() -> Backend {
        let model = std::env::var("GHOSTWRITE_LOCAL_MODEL").unwrap_or_else(|_| "bartowski/Qwen2.5-Coder-14B-Instruct-GGUF:Q3_K_M".into());
        Backend::Local { label: models::local_label(&model), model }
    }

    #[test]
    #[ignore]
    fn a_local_model_makes_an_edit() {
        let root = ghostwrite_project::scratch_dir("live-local-edit");
        let text = "def add(a, b):\n    pass\n";
        std::fs::write(root.join("calc.py"), text).unwrap();
        let req = Request {
            path: Some(root.join("calc.py")),
            language: "python".into(),
            range: 0..text.len(),
            text: text.into(),
            instruction: "make it return the sum".into(),
            root: Some(root.clone()),
            ..asked()
        };
        let mut out = Collect::default();
        local_model().run(&req, &mut out, &AtomicBool::new(false)).unwrap();
        println!("{:?}\n{}", out.1, out.0);
        assert_eq!(protocol::check(&out.0, true), Ok(()), "{}", out.0);
        let protocol::Reply::Step(step) = protocol::parse(&out.0, true) else { panic!("an edit: {}", out.0) };
        assert!(step.new.join("\n").contains("return") && crate::placement::edit(text, &step, 0..2).is_some(), "{}", out.0);
    }

    #[test]
    #[ignore]
    fn a_local_model_reads_with_the_tools() {
        let (root, main) = two_file_project();
        let mut out = Collect::default();
        local_model().run(&asked_about_another_file(&root, &main), &mut out, &AtomicBool::new(false)).unwrap();
        println!("looked at: {:?}\n{}", out.1, out.0);
        assert_eq!(protocol::check(&out.0, true), Ok(()), "{}", out.0);
        assert!(out.1.iter().any(|w| w.contains("util.py")), "{:?}", out.1);
    }
}
