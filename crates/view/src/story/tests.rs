//! The story driven directly, with no app: the keys go through the modal as
//! they would in AI mode, the scripted replies arrive as [`Event`]s, and
//! what the app would do is done here by hand.

use std::path::PathBuf;

use ghostwrite_ai::ItemId;
use ghostwrite_core::{Key, Modal, Mode, Outcome, Registers};
use ghostwrite_project::{fs, scratch_dir, store};

use super::{Do, Event, Show, StoryState};
use crate::Project;

/// The registers the modal yanks to: a story's keys don't.
struct NoRegisters;

impl Registers for NoRegisters {
    fn yanked(&self, _: Option<char>) -> String {
        String::new()
    }
    fn yank(&mut self, _: Option<char>, _: String) {}
    fn clipboard(&self) -> Option<String> {
        None
    }
    fn set_clipboard(&mut self, _: String) {}
}

/// A project folder with `doc.txt` holding `text`, open, and a story about to be asked.
struct Setup {
    project: Project,
    modal: Modal,
    state: Option<StoryState>,
    dir: PathBuf,
}

fn open(text: &str) -> Setup {
    store::redirect_for_tests();
    fs::delete_instead_of_trash();
    let dir = scratch_dir("story");
    let path = dir.join("doc.txt");
    std::fs::write(&path, text).unwrap();
    let mut project = Project::new(dir.clone(), ghostwrite_lsp::Registry::disabled(dir.clone()), false);
    let ix = project.open(&path).unwrap();
    project.activate(ix);
    Setup { project, modal: Modal::default(), state: None, dir }
}

impl Setup {
    fn state(&mut self) -> &mut StoryState {
        self.state.as_mut().expect("a story is up")
    }

    /// `space a`, and `what` asked: the request goes out.
    fn ask(&mut self, what: &str) -> &mut Self {
        self.modal.enter(Mode::Ai);
        let state = StoryState::about_selection(&mut self.project, false).unwrap();
        self.project.tabs.hold(Some(state.doc));
        self.state = Some(state);
        assert!(self.state().ask(what.into()));
        self.send();
        self
    }

    /// The request is sent, as the app would.
    fn send(&mut self) {
        let Setup { project, state, .. } = self;
        assert!(state.as_mut().unwrap().to_send(project, None).is_some(), "there's a request to send");
    }

    /// The scripted reply to the request on its way arrives, and the story settles.
    fn reply(&mut self, text: &str) -> Show {
        self.state().receive(Event::Text(text.into()));
        self.state().receive(Event::Done);
        self.settle()
    }

    /// The story moves on, and what it shows: a correction or the next item is asked for.
    fn settle(&mut self) -> Show {
        let Setup { project, state, .. } = self;
        let show = state.as_mut().unwrap().settle(project, "the script");
        if show == Show::Revise {
            self.send();
        }
        show
    }

    /// A key in AI mode: what the story says to do about it.
    fn key(&mut self, key: Key) -> Do {
        let Setup { project, modal, state, .. } = self;
        let tab = project.tab_mut().unwrap();
        let Outcome::Review(review) = modal.key(key, &mut NoRegisters, &mut tab.target(1)) else { panic!("{key:?} is a review key") };
        state.as_mut().unwrap().review(review, project)
    }

    /// Accept, as the app does on [`Do::Accept`] (there's no command to run): whether it was.
    fn accept(&mut self) -> bool {
        let Setup { project, state, .. } = self;
        state.as_mut().unwrap().accept(project)
    }

    /// Finish the story, as the app does on [`Do::Finish`].
    fn finish(&mut self) {
        let state = self.state.take().unwrap();
        state.finish(&mut self.project);
        state.close(&mut self.project);
    }

    fn text(&self) -> String {
        self.project.tab().unwrap().doc.buffer.to_string()
    }

    fn current(&self) -> Option<ItemId> {
        self.state.as_ref()?.story.as_ref()?.current()
    }

    fn status(&self) -> String {
        self.project.status.as_deref().unwrap_or_default().to_string()
    }
}

const ADD: &str = "def add(a, b):\n    pass\n";

/// A scripted edit in `doc.txt`: `at` becomes `new`.
fn edit(at: &str, new: &str) -> String {
    format!("<file>\ndoc.txt\n</file>\n<at>\n{at}\n</at>\n<say>\nBecause.\n</say>\n<new>\n{new}\n</new>\n")
}

fn fix() -> String {
    edit("    pass", "    return a + b")
}

fn hints() -> String {
    edit("def add(a, b):", "def add(a: int, b: int) -> int:")
}

#[test]
fn tab_accepts_the_step_and_the_next_one_is_asked_for() {
    let mut t = open(ADD);
    assert_eq!(t.ask("implement it").reply(&fix()), Show::Tab(0), "the step shows in its tab");
    assert_eq!(t.text(), ADD, "nothing changes before accepting");
    assert_eq!(t.key(Key::Tab), Do::Accept);
    assert!(t.accept());
    assert_eq!(t.text(), "def add(a, b):\n    return a + b\n");
    assert_eq!(t.settle(), Show::Revise, "the next item is asked for");
    assert_eq!(t.reply(&hints()), Show::Tab(0));
    assert_eq!(t.current(), Some(ItemId(1)), "the next item came");
    assert!(t.accept());
    assert_eq!(t.text(), "def add(a: int, b: int) -> int:\n    return a + b\n");
}

#[test]
fn enter_asks_with_what_is_typed() {
    let mut t = open(ADD);
    t.ask("implement it").reply(&fix());
    assert_eq!(t.key(Key::Enter), Do::Ask, "the prompt opens");
    assert!(!t.state().ask("  ".into()), "nothing typed is nothing asked");
    assert!(t.state().ask("with type hints".into()));
    assert_eq!(t.state().said.as_deref(), Some("with type hints"), "shown while waiting");
    let req = t.state.as_ref().unwrap().request(&t.project).unwrap();
    assert_eq!(req.followups.last().map(|f| f.said.as_str()), Some("with type hints"), "Claude is told");
    assert_eq!(t.current(), None, "and the step it was about waits for the reply");
    assert_eq!(t.text(), ADD);
}

#[test]
fn esc_esc_ends_the_story() {
    let mut t = open(ADD);
    t.ask("implement it").reply(&fix());
    assert_eq!(t.key(Key::Escape), Do::Nothing, "Esc out of habit doesn't end it");
    assert_eq!(t.status(), "esc again to end this story");
    assert_eq!(t.key(Key::Escape), Do::Finish);
    t.finish();
    assert_eq!(t.text(), ADD, "nothing was applied");
    assert!(t.project.tabs.iter().count() == 1, "and nothing's left open");
}

#[test]
fn reading_outside_the_project_is_asked_tab_yes_enter_no() {
    let mut t = open(ADD);
    t.ask("implement it").reply(&fix());
    let hosts = PathBuf::from("/etc/hosts");
    t.state().receive(Event::MayRead(hosts.clone()));
    assert_eq!(t.key(Key::Tab), Do::AnswerRead(true), "Tab lets it");
    assert_eq!(t.text(), ADD, "and accepts nothing");
    assert!(t.status().starts_with("let it read"), "{}", t.status());
    t.state().receive(Event::MayRead(hosts.clone()));
    assert_eq!(t.key(Key::Enter), Do::AnswerRead(false), "Enter doesn't, and opens no prompt");
    t.state().receive(Event::MayRead(hosts.clone()));
    assert_eq!(t.key(Key::Escape), Do::Nothing);
    assert_eq!(t.state().asking_to_read.as_ref(), Some(&hosts), "Esc keeps asking");
    assert_eq!(t.key(Key::Escape), Do::Finish, "and Esc again ends the story, with the question");
}

#[test]
fn a_new_request_leaves_the_question_unanswered() {
    let mut t = open(ADD);
    t.ask("implement it").reply(&fix());
    t.state().receive(Event::MayRead("/etc/hosts".into()));
    t.key(Key::Enter);
    t.state().ask("something else".into());
    t.send();
    assert_eq!(t.state().asking_to_read, None);
}

#[test]
fn a_step_waits_for_the_user_who_wandered_off_and_tab_brings_them_back() {
    let mut t = open(ADD);
    t.ask("implement it").reply(&fix());
    t.key(Key::Tab);
    t.accept();
    assert_eq!(t.settle(), Show::Revise);
    // Looking elsewhere while the next one's on its way.
    t.state().wander();
    assert_eq!(t.reply(&hints()), Show::Ready, "it waits for Tab");
    assert_eq!(t.key(Key::Tab), Do::BackToTheStep, "Tab brings them to it, not accepting it");
    assert_eq!(t.settle(), Show::Tab(0));
    assert!(!t.state().wandered);
    assert_eq!(t.text(), "def add(a, b):\n    return a + b\n");
    assert_eq!(t.key(Key::Tab), Do::Accept, "the next Tab accepts it");
}

#[test]
fn a_finished_story_can_be_undone_while_nothing_has_changed_since() {
    let mut t = open(ADD);
    t.ask("implement it").reply(&fix());
    t.accept();
    let state = t.state.take().unwrap();
    let mut undo = state.close(&mut t.project).expect("it changed doc.txt");
    super::undo(&mut t.project, &mut undo).unwrap();
    assert_eq!(t.text(), ADD);
    assert!(super::undo(&mut t.project, &mut undo).is_err(), "and it's done once");
}

#[test]
fn clear_forgets_what_the_stories_before_left() {
    let mut t = open(ADD);
    let note = "<file>\ndoc.txt\n</file>\n<at>\ndef add(a, b):\n</at>\n<say>\nIt adds.\n</say>\n";
    let done = "<done>\n<memory>\nThe user is learning Python.\n</memory>\n</done>\n";
    t.ask("what is this").reply(note);
    assert_eq!(t.key(Key::Tab), Do::Accept);
    t.accept();
    t.settle();
    assert_eq!(t.reply(done), Show::Finished);
    t.finish();
    assert!(!ghostwrite_ai::memory::notes(&t.dir).is_empty() && !ghostwrite_ai::memory::recent(&t.dir, 5).is_empty());
    super::forget(&mut t.project);
    assert!(t.status().starts_with("starting over"), "{}", t.status());
    assert_eq!(ghostwrite_ai::memory::notes(&t.dir), "", "Claude's notes are gone");
    assert!(ghostwrite_ai::memory::recent(&t.dir, 5).is_empty(), "and the earlier requests");
    t.ask("again");
    let prompt = t.state.as_ref().unwrap().request(&t.project).unwrap().prompt();
    assert!(!prompt.contains("what is this") && !prompt.contains("learning Python"), "the next story is told nothing of them");
}
