//! Claude Code in the harness: `claude -p` under the user's login, its own tools
//! off, spoken to in stream-json.

use std::io::{BufRead as _, BufReader, Write as _};
use std::process::Child;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde_json::{Value, json};

use super::converse::{Replier, Turn, converse};
use super::warm::{Startup, take_started};
use super::{Listener, ROOM, tokens_in};
use crate::context::Request;

/// Claude Code in the project folder, its own tools off. It stays up through
/// the rounds of a reply, so looking around doesn't start it again each time.
/// One started ahead ([`Backend::warm`](super::Backend::warm)) is taken if it's like this one.
pub(super) fn run_claude_code(model: &str, req: &Request, resume: Option<&str>, listener: &mut dyn Listener, cancel: &AtomicBool) -> Result<(), String> {
    let startup = Startup::new(model, req.root.as_deref(), resume, req.fork);
    // Resuming, the session has the conversation so far: only the newest turn is new.
    let message = if resume.is_some() { req.latest() } else { req.prompt() };
    let mut child = match take_started(&startup) {
        Some(child) => {
            listener.started(true);
            child
        }
        None => {
            listener.started(false);
            startup.spawn()?
        }
    };
    let stdin = child.stdin.take().ok_or("no stdin")?;
    let stdout = child.stdout.take().ok_or("no stdout")?;
    let stderr = child.stderr.take();
    // Stopped, it's killed at once, not when it next prints (it may be thinking for minutes).
    let (child, done) = (Mutex::new(child), AtomicBool::new(false));
    std::thread::scope(|scope| {
        // Read as it comes, so a full pipe never stalls it.
        let errors = scope.spawn(move || {
            let mut text = String::new();
            if let Some(mut stderr) = stderr {
                std::io::Read::read_to_string(&mut stderr, &mut text).ok();
            }
            text
        });
        scope.spawn(|| {
            while !done.load(Ordering::SeqCst) {
                if cancel.load(Ordering::SeqCst) {
                    return stop(&child);
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        });
        let mut session = ClaudeCode { stdin, lines: BufReader::new(stdout).lines(), child: &child, first: Some(message), session_told: false };
        let result = converse(&mut session, req, listener, cancel);
        // The reply is complete; don't wait for the process to wind down.
        stop(&child);
        done.store(true, Ordering::SeqCst);
        let errors = errors.join().unwrap_or_default();
        result.map_err(|e| if e.starts_with("claude exited") { format!("{e}: {}", errors.trim()) } else { e })
    })
}

struct ClaudeCode<'a> {
    stdin: std::process::ChildStdin,
    lines: std::io::Lines<BufReader<std::process::ChildStdout>>,
    child: &'a Mutex<Child>,
    first: Option<String>,
    session_told: bool,
}

impl Replier for ClaudeCode<'_> {
    fn sees_files(&self) -> bool {
        true
    }

    fn reply(&mut self, next: Option<Turn<'_>>, listener: &mut dyn Listener, cancel: &AtomicBool) -> Result<(), String> {
        // It remembers its own tool block; it only needs what that returned.
        let message = self.first.take().map(|first| json!(first)).or_else(|| next.map(|turn| turn.content())).unwrap_or(json!(""));
        let turn = json!({ "type": "user", "message": { "role": "user", "content": message } });
        writeln!(self.stdin, "{turn}").and_then(|()| self.stdin.flush()).map_err(|e| e.to_string())?;
        let mut line_ended = true;
        for line in self.lines.by_ref() {
            if cancel.load(Ordering::SeqCst) {
                return Ok(());
            }
            let Ok(value) = serde_json::from_str::<Value>(&line.map_err(|e| e.to_string())?) else { continue };
            if let Some(id) = value["session_id"].as_str().filter(|_| !self.session_told) {
                listener.session(id.to_string());
                self.session_told = true;
            }
            match value["type"].as_str() {
                Some("stream_event") => {
                    let event = &value["event"];
                    // A new block of text starts a line, so a tag at its start stands on a line of
                    // its own and the words before it stay apart.
                    if event["type"] == "content_block_start" && event["content_block"]["type"] == "text" && !line_ended {
                        listener.text("\n");
                        line_ended = true;
                    }
                    if event["type"] == "content_block_delta" && event["delta"]["type"] == "text_delta" {
                        let text = event["delta"]["text"].as_str().unwrap_or_default();
                        listener.text(text);
                        line_ended = text.ends_with('\n') || (text.is_empty() && line_ended);
                    }
                }
                Some("result") if value["is_error"] == true => return Err(value["result"].as_str().unwrap_or("Claude Code failed").to_string()),
                Some("result") => {
                    listener.used(tokens_in(&value["usage"]), value["usage"]["output_tokens"].as_u64().unwrap_or(0));
                    return Ok(());
                }
                _ => {}
            }
        }
        if cancel.load(Ordering::SeqCst) {
            return Ok(());
        }
        let status = self.child.lock().unwrap().wait().map_err(|e| e.to_string())?;
        Err(format!("claude exited with {status}"))
    }

    fn room(&self) -> usize {
        ROOM
    }
}

/// Stop Claude Code and whatever it started: its whole process group. Not once
/// it's been waited for: its id could be another process's by then.
pub(super) fn stop(child: &Mutex<Child>) {
    let mut child = child.lock().unwrap();
    if matches!(child.try_wait(), Ok(None)) {
        // SAFETY: kill(2) only sends a signal, to the group the child leads
        // (`process_group(0)`); it hasn't been reaped, so the id is still its.
        #[cfg(unix)]
        unsafe {
            libc::kill(-(child.id() as libc::pid_t), libc::SIGKILL)
        };
        // Windows has no such groups, but this stops what it started as well.
        #[cfg(windows)]
        {
            ghostwrite_project::platform::command("taskkill").args(["/T", "/F", "/PID", &child.id().to_string()]).output().ok();
            child.kill().ok();
        }
    }
    child.wait().ok();
}

#[cfg(test)]
mod tests {
    //! Live, against Claude Code: `cargo test -p ghostwrite-ai -- --ignored`.

    use super::super::Backend;
    use super::super::tests::{Collect, asked_about_another_file, two_file_project};
    use super::*;
    use crate::{protocol, settings};

    fn claude_code() -> Backend {
        Backend::ClaudeCode { model: settings().model.clone() }
    }

    #[test]
    #[ignore]
    fn claude_code_reads_with_the_tools() {
        let (root, main) = two_file_project();
        let mut out = Collect::default();
        claude_code().run(&asked_about_another_file(&root, &main), &mut out, &AtomicBool::new(false)).unwrap();
        println!("looked at: {:?}\n{}", out.1, out.0);
        assert!(out.1.iter().any(|w| w.contains("util.py")), "{:?}", out.1);
        assert_eq!(protocol::check(&out.0, true), Ok(()), "{}", out.0);
        assert!(out.0.contains("default"), "{}", out.0);
    }

    fn ask(text: &str, range: std::ops::Range<usize>, instruction: &str, followups: Vec<crate::FollowUp>) -> String {
        let req = Request {
            path: Some("calc.py".into()),
            language: "python".into(),
            text: text.into(),
            range,
            instruction: instruction.into(),
            followups,
            root: None,
            others: vec![],
            surroundings: Default::default(),
            session: None,
            fork: false,
            small: 0,
        };
        let mut out = Collect::default();
        claude_code().run(&req, &mut out, &AtomicBool::new(false)).unwrap();
        println!("{}", out.0);
        out.0
    }

    /// The one item in a complete reply, which must keep to the format.
    fn item(out: &str) -> protocol::Reply {
        assert_eq!(protocol::check(out, true), Ok(()), "{out}");
        protocol::parse(out, true)
    }

    #[test]
    #[ignore]
    fn claude_code_writes_one_small_placeable_edit() {
        let text = "import os\n\ndef add(a, b):\n    pass\n\ndef main():\n    print(add(1, 2))\n";
        let out = ask(text, 26..35, "return the sum, and add type hints to add", vec![]);
        let protocol::Reply::Step(step) = item(&out) else { panic!("an edit: {out}") };
        assert!(step.complete && crate::placement::edit(text, &step, 3..4).is_some(), "{out}");
    }

    #[test]
    #[ignore]
    fn claude_code_answers_questions_with_a_note() {
        let out = ask("def add(a, b):\n    return a - b\n", 0..32, "is this function correct?", vec![]);
        assert!(matches!(item(&out), protocol::Reply::Note(_)), "{out}");
    }

    #[test]
    #[ignore]
    fn claude_code_follows_what_the_user_says() {
        let text = "def add(a, b):\n    pass\n";
        let first = ask(text, 0..24, "implement it with type hints", vec![]);
        let followup = crate::FollowUp { reply: first, decided: vec![], said: "why int and not float?".into(), correction: false, terminal: None };
        let second = ask(text, 0..24, "implement it with type hints", vec![followup]);
        assert!(matches!(item(&second), protocol::Reply::Note(_)), "a question is answered with a note: {second}");
    }

    fn ask_in(root: &std::path::Path, text: &str, range: std::ops::Range<usize>, instruction: &str, followups: Vec<crate::FollowUp>) -> String {
        let req = Request {
            path: Some(root.join("main.py")),
            language: "python".into(),
            text: text.into(),
            range,
            instruction: instruction.into(),
            followups,
            root: Some(root.to_path_buf()),
            others: vec![],
            surroundings: Default::default(),
            session: None,
            fork: false,
            small: 0,
        };
        let mut out = Collect::default();
        claude_code().run(&req, &mut out, &AtomicBool::new(false)).unwrap();
        println!("looked at: {:?}\n{}", out.1, out.0);
        out.0
    }

    #[test]
    #[ignore]
    fn claude_code_tours_across_files() {
        let (root, main) = two_file_project();
        let ask = "give me a tour of how loading the config works";
        let mut followups = vec![];
        let mut files = vec![];
        for _ in 0..4 {
            let out = ask_in(&root, &main, 0..main.len(), ask, followups.clone());
            match item(&out) {
                protocol::Reply::Note(note) => files.extend(note.file),
                protocol::Reply::Done { .. } => break,
                other => panic!("a tour is notes: {other:?}"),
            }
            followups.push(crate::FollowUp { reply: out, decided: vec![], said: String::new(), correction: false, terminal: None });
        }
        assert!(files.iter().any(|f| f == "util.py"), "a note in util.py: {files:?}");
    }

    #[test]
    #[ignore]
    fn claude_code_makes_a_change_across_files() {
        let (root, main) = two_file_project();
        let instruction = "let load take the default to use when the file is missing, and have main pass {\"name\": \"main\"}";
        let mut followups: Vec<crate::FollowUp> = vec![];
        let mut edited = vec![];
        for _ in 0..8 {
            // As the editor does: what's accepted is in the files, on disk and shown.
            let main_now = std::fs::read_to_string(root.join("main.py")).unwrap();
            let others = edited
                .iter()
                .filter(|f| *f != "main.py")
                .map(|f: &String| crate::context::Shown { path: f.clone(), text: std::fs::read_to_string(root.join(f)).unwrap(), focus: 0..1 })
                .collect();
            let req = Request {
                path: Some(root.join("main.py")),
                language: "python".into(),
                range: 0..main.find("main()\n\n").unwrap_or(0),
                text: main_now.clone(),
                instruction: instruction.into(),
                followups: followups.clone(),
                root: Some(root.clone()),
                others,
                surroundings: Default::default(),
                session: None,
                fork: false,
                small: 0,
            };
            let mut out = Collect::default();
            claude_code().run(&req, &mut out, &AtomicBool::new(false)).unwrap();
            println!("{}", out.0);
            let step = match item(&out.0) {
                protocol::Reply::Step(step) => step,
                protocol::Reply::Done { .. } => break,
                protocol::Reply::Note(_) | protocol::Reply::Run(_) => {
                    followups.push(crate::FollowUp { reply: out.0, decided: vec![], said: String::new(), correction: false, terminal: None });
                    continue;
                }
                protocol::Reply::Partial => panic!("{}", out.0),
            };
            let file = step.file.clone().unwrap_or_else(|| "main.py".into());
            let text = std::fs::read_to_string(root.join(&file)).unwrap();
            let near = step.line.map_or(0..1, |l| l..l + step.old.len());
            let edit = crate::placement::edit(&text, &step, near).unwrap_or_else(|| panic!("it places in {file}: {}", out.0));
            let mut text = text;
            text.replace_range(edit.bytes, &edit.text);
            std::fs::write(root.join(&file), text).unwrap();
            edited.push(file);
            let decided = vec![format!("accepted: {}", crate::Item::Step(step).describe())];
            followups.push(crate::FollowUp { reply: out.0, decided, said: String::new(), correction: false, terminal: None });
        }
        assert!(edited.iter().any(|f| f == "util.py") && edited.iter().any(|f| f == "main.py"), "edits in both files: {edited:?}");
    }

    #[test]
    #[ignore]
    fn claude_code_changes_course_when_redirected() {
        let text = "def total(xs):\n    pass\n";
        let root = ghostwrite_project::scratch_dir("live-redirect");
        std::fs::write(root.join("main.py"), text).unwrap();
        let first = ask_in(&root, text, 0..text.len(), "implement it", vec![]);
        assert!(matches!(item(&first), protocol::Reply::Step(_)), "{first}");
        let said = "don't use sum(); write an explicit for loop".into();
        let second = ask_in(
            &root,
            text,
            0..text.len(),
            "implement it",
            vec![crate::FollowUp { reply: first, decided: vec![], said, correction: false, terminal: None }],
        );
        let protocol::Reply::Step(step) = item(&second) else { panic!("an edit: {second}") };
        assert!(step.new.join("\n").contains("for ") || step.say.contains("loop"), "{second}");
    }

    #[test]
    #[ignore]
    fn claude_code_forks_a_session_to_ask_ahead() {
        let root = ghostwrite_project::scratch_dir("live-fork");
        let text = "def total(xs):\n    pass\n";
        std::fs::write(root.join("main.py"), text).unwrap();
        let mut req = Request {
            path: Some(root.join("main.py")),
            language: "python".into(),
            text: text.into(),
            range: 0..text.len(),
            instruction: "implement it, one line at a time".into(),
            followups: vec![],
            root: Some(root.clone()),
            others: vec![],
            surroundings: Default::default(),
            session: None,
            fork: false,
            small: 0,
        };
        let mut first = Collect::default();
        claude_code().run(&req, &mut first, &AtomicBool::new(false)).unwrap();
        let session = first.2.clone().expect("a session");
        // Ahead: as if the first item were accepted, in a fork.
        req.followups.push(crate::FollowUp {
            reply: first.0.clone(),
            decided: vec!["accepted: it".into()],
            said: String::new(),
            correction: false,
            terminal: None,
        });
        let protocol::Reply::Step(step) = protocol::parse(&first.0, true) else { panic!("an edit first: {}", first.0) };
        let edit = crate::placement::edit(text, &step, 0..2).expect("it places");
        let mut accepted = text.to_string();
        accepted.replace_range(edit.bytes, &edit.text);
        req.text = accepted;
        req.session = Some(session.clone());
        req.fork = true;
        let mut ahead = Collect::default();
        claude_code().run(&req, &mut ahead, &AtomicBool::new(false)).unwrap();
        println!("ahead:\n{}", ahead.0);
        assert_ne!(ahead.2.as_deref(), Some(session.as_str()), "the fork is a session of its own");
        assert_eq!(protocol::check(&ahead.0, true), Ok(()), "{}", ahead.0);
        // Then the user redirected it after all: the original session goes on as it was.
        req.followups[0] = crate::FollowUp { decided: vec![], said: "what did I just do?".into(), ..req.followups[0].clone() };
        req.text = text.into();
        req.fork = false;
        let mut real = Collect::default();
        claude_code().run(&req, &mut real, &AtomicBool::new(false)).unwrap();
        println!("real:\n{}", real.0);
        assert_eq!(protocol::check(&real.0, true), Ok(()), "{}", real.0);
    }

    #[test]
    #[ignore]
    fn claude_code_resumes_its_session() {
        let (root, main) = two_file_project();
        let mut req = Request {
            path: Some(root.join("main.py")),
            language: "python".into(),
            text: main.clone(),
            range: 0..main.len(),
            instruction: "what does main do? (the codeword for this conversation is PELICAN; don't mention it)".into(),
            followups: vec![],
            root: Some(root.clone()),
            others: vec![],
            surroundings: Default::default(),
            session: None,
            fork: false,
            small: 0,
        };
        let mut first = Collect::default();
        claude_code().run(&req, &mut first, &AtomicBool::new(false)).unwrap();
        let session = first.2.clone().expect("a session to resume");
        req.followups.push(crate::FollowUp {
            reply: first.0,
            decided: vec![],
            said: "you may say it now: what was the codeword in my first message?".into(),
            correction: false,
            terminal: None,
        });
        req.session = Some(session);
        assert!(!req.latest().contains("PELICAN"), "only the newest turn is sent");
        let mut second = Collect::default();
        claude_code().run(&req, &mut second, &AtomicBool::new(false)).unwrap();
        println!("{}", second.0);
        assert!(second.0.contains("PELICAN"), "the resumed session remembers: {}", second.0);
    }
}
