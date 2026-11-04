//! Stories with a scripted model: asking, reviewing item by item, steps that apply across files, corrections, and what Claude is told.

use super::*;

#[gpui::test]
fn each_item_is_asked_for_once_the_last_is_decided(cx: &mut TestAppContext) {
    let mut t = open(ADD, &[&fix(), &hints(), DONE], cx);
    t.keys("j space a").input("implement it");
    assert_eq!(t.text(), ADD, "typing in the prompt doesn't drive normal mode");
    t.keys("enter");
    assert_eq!(t.text(), ADD, "nothing changes before accepting");
    t.keys("tab");
    assert_eq!(t.text(), "def add(a, b):\n    return a + b\n");
    assert_eq!(t.current(), Some(1), "the next item came");
    t.keys("shift-tab");
    assert_eq!(t.current(), Some(1), "a story only goes forward");
    // Not that one: saying so asks for more, and <done> finishes.
    t.keys("enter").input("no type hints").keys("enter");
    assert!(!t.reviewing());
    assert_eq!(t.status().as_deref(), Some("applied 1 of 2 steps"));
    t.keys("u");
    assert_eq!(t.text(), ADD, "each accepted edit is its own undo");
}

#[gpui::test]
fn the_next_items_are_asked_for_ahead(cx: &mut TestAppContext) {
    let mut t = open(ADD, &[&fix(), &hints(), DONE], cx);
    t.ask("implement it");
    let wanted = t.read(|e| {
        let story = e.story.as_ref().unwrap().story.as_ref().unwrap();
        (story.wants(0), story.wants(1), story.wants(2))
    });
    assert_eq!(wanted, (true, true, false), "the next item and the one after, up to <done>");
    let item = t.read(|e| e.story.as_ref().unwrap().story.as_ref().unwrap().current_item().unwrap());
    let ahead = t.read(|e| e.story.as_ref().unwrap().request_ahead(&e.project, &[item]).unwrap().text);
    assert_eq!(ahead, "def add(a, b):\n    return a + b\n", "what's sent ahead shows the file as if the edit were accepted");
    t.keys("tab");
    assert_eq!(t.current(), Some(1), "accepting brings the next item, asked ahead");
    t.keys("tab");
    assert!(!t.reviewing());
    assert_eq!(t.status().as_deref(), Some("applied 2 of 2 steps"));
}

#[gpui::test]
fn asking_ahead_marks_the_lines_asked_about_in_the_file_it_assumes(cx: &mut TestAppContext) {
    // Accepted, the first line grows, and where `three` starts now would be inside an arrow.
    let mut t = open("one\ntwo\nthree\n", &[&edit("doc.txt", "one", "→→→→→"), DONE], cx);
    t.keys("j j");
    t.ask("change it");
    let item = t.read(|e| e.story.as_ref().unwrap().story.as_ref().unwrap().current_item().unwrap());
    let request = t.read(|e| e.story.as_ref().unwrap().request_ahead(&e.project, &[item]).unwrap());
    assert_eq!(request.text, "→→→→→\ntwo\nthree\n");
    assert!(request.prompt().contains("→→→→→"), "it can be sent");
    assert_eq!(&request.text[request.range.clone()], "three\n");
}

#[gpui::test]
fn escape_stops_early_and_keeps_what_was_accepted(cx: &mut TestAppContext) {
    let mut t = open(ADD, &[&fix(), &hints(), DONE], cx);
    t.ask("types and body").keys("tab escape");
    assert!(t.reviewing(), "one esc doesn't end it");
    t.keys("escape");
    assert!(!t.reviewing());
    assert_eq!(t.text(), "def add(a, b):\n    return a + b\n");
}

#[gpui::test]
fn notes_are_read_one_at_a_time(cx: &mut TestAppContext) {
    let first = note("doc.txt", "def add(a, b):", "It adds two numbers.");
    let mut t = open(ADD, &[&first, &note("doc.txt", "    pass", "It doesn't do it yet."), DONE], cx);
    t.ask("what does this do");
    assert_eq!(t.current(), Some(0));
    t.keys("tab");
    assert_eq!(t.current(), Some(1));
    t.keys("tab");
    assert!(!t.reviewing(), "<done> finishes");
    assert_eq!(t.text(), ADD, "notes never edit");
}

#[gpui::test]
fn notes_jump_through_the_code(cx: &mut TestAppContext) {
    let mut text = String::from("def first():\n    return 1\n");
    text.extend((0..120).map(|i| format!("# filler {i}\n")));
    text.push_str("def second():\n    return first() + 1\n");
    let first = note("doc.txt", "def first():", "first returns one.");
    let second = note("doc.txt", "    return first() + 1", "second builds on it.");
    let mut t = open(&text, &[&first, &second, DONE], cx);
    t.ask("how do these relate");
    assert_eq!(t.cursor(), 0, "the first note is about `def first`");
    t.keys("tab");
    assert_eq!(t.cursor(), text.find("    return first() + 1").unwrap(), "Tab jumps to the second note's line");
}

#[gpui::test]
fn what_you_say_shapes_the_next_item(cx: &mut TestAppContext) {
    let sum = edit("doc.txt", "    pass", "    return sum((a, b))");
    let mut t = open(ADD, &[&fix(), &sum, DONE], cx);
    t.ask("implement it").keys("enter").input("use sum()");
    assert_eq!(t.text(), ADD, "typing the redirect doesn't edit");
    t.keys("enter").keys("tab");
    assert_eq!(t.text(), "def add(a, b):\n    return sum((a, b))\n");
    assert!(!t.reviewing());
}

#[gpui::test]
fn escape_leaves_a_redirect_but_keeps_reviewing(cx: &mut TestAppContext) {
    let mut t = open(ADD, &[&fix(), &hints(), DONE], cx);
    t.ask("implement it").keys("enter").input("never mind").keys("escape");
    assert!(t.reviewing() && t.read(|e| e.prompt.is_none()));
    t.keys("tab");
    assert_eq!(t.text(), "def add(a, b):\n    return a + b\n");
}

#[gpui::test]
fn a_story_can_rename_and_remove_files(cx: &mut TestAppContext) {
    let rename = "<rename>\nsrc/util.rs\nsrc/helpers.rs\n</rename>\n<say>\nA clearer name.\n</say>\n";
    let remove = "<remove>\nold.txt\n</remove>\n<say>\nNothing uses it.\n</say>\n";
    let mut t = open("x\n", &[rename, remove, DONE], cx);
    std::fs::create_dir_all(t.dir.join("src")).unwrap();
    std::fs::write(t.dir.join("src/util.rs"), "pub fn one() {}\n").unwrap();
    std::fs::write(t.dir.join("old.txt"), "old\n").unwrap();
    t.editor.update(t.cx, |e, _| e.project.tree.refresh());
    t.ask("tidy up the files");
    let proposal = t.read(|e| e.project.tree.proposal().cloned());
    assert_eq!(proposal, Some(Proposal::Rename(t.dir.join("src/util.rs"), "src/helpers.rs".into())), "shown in the tree first");
    assert!(t.dir.join("src/util.rs").is_file(), "nothing moves before Tab");
    t.keys("tab");
    assert!(!t.dir.join("src/util.rs").exists() && t.dir.join("src/helpers.rs").is_file(), "Tab renames it");
    assert_eq!(t.read(|e| e.project.tree.proposal().cloned()), Some(Proposal::Remove(t.dir.join("old.txt"))));
    t.keys("tab");
    assert!(!t.dir.join("old.txt").exists(), "and moves the other to the Trash");
    assert_eq!(t.status().as_deref(), Some("applied 2 of 2 steps"));
}

#[gpui::test]
fn a_story_asked_from_an_empty_tab_can_create_a_file(cx: &mut TestAppContext) {
    let create = "<create>\nposts/new.typ\n</create>\n<say>\nA new post.\n</say>\n<new>\n#set document(title: \"New\")\n\n</new>\n";
    let mut t = open("", &[create, DONE], cx);
    t.command("n");
    t.ask("make a new post");
    t.keys("tab");
    assert_eq!(t.file("posts/new.typ"), "#set document(title: \"New\")\n\n", "Tab creates it and writes its first lines");
    assert_eq!(t.status().as_deref(), Some("applied 1 of 1 steps"), "and the story goes on");
}

#[gpui::test]
fn a_story_asked_from_an_empty_tab_can_edit_another_file(cx: &mut TestAppContext) {
    let mut t = open("", &[&edit("other.txt", "old", "new"), DONE], cx);
    std::fs::write(t.dir.join("other.txt"), "old\n").unwrap();
    t.command("n");
    t.ask("change the other file");
    assert_eq!(t.active().as_deref(), Some("other.txt"), "it opens where the item is");
    t.keys("tab");
    assert_eq!(t.file("other.txt"), "new\n");
    assert_eq!(t.status().as_deref(), Some("applied 1 of 1 steps"));
}

#[gpui::test]
fn a_story_ends_if_the_tab_it_was_asked_from_closes(cx: &mut TestAppContext) {
    let mut t = open(ADD, &[&fix(), DONE], cx);
    t.ask("implement it");
    t.command("bc");
    t.keys("tab");
    assert!(!t.reviewing(), "Tab doesn't sit on an item it can't find");
}

#[gpui::test]
fn a_story_can_create_a_file(cx: &mut TestAppContext) {
    let create = "<create>\nsrc/util.rs\n</create>\n<say>\nA home for helpers.\n</say>\n<new>\npub fn one() -> u32 { 1 }\n</new>\n";
    let mut t = open("fn main() {}\n", &[create, &edit("doc.txt", "fn main() {}", "mod util;\nfn main() {}"), DONE], cx);
    t.ask("move a helper into util");
    assert!(!t.dir.join("src/util.rs").exists(), "nothing is created before Tab");
    assert!(t.read(|e| e.project.tree.new_entry().is_some()), "the new file shows in the tree");
    t.keys("tab");
    assert!(t.dir.join("src/util.rs").is_file(), "Tab creates it, folders included");
    let util = t.dir.join("src/util.rs");
    let util_text = t.read(|e| e.project.tabs.text_of(&util).map(str::to_string));
    assert_eq!(util_text.as_deref(), Some("pub fn one() -> u32 { 1 }\n"), "with its first lines");
    assert_eq!(t.active().as_deref(), Some("doc.txt"), "then to the next item's file");
    t.keys("tab");
    assert_eq!(t.text(), "mod util;\nfn main() {}\n");
    assert_eq!(t.status().as_deref(), Some("applied 2 of 2 steps"));
}

#[gpui::test]
fn a_finished_story_is_remembered(cx: &mut TestAppContext) {
    let done = "<done>\n<memory>\nThe user is learning Python.\n</memory>\n</done>\n";
    let mut t = open(ADD, &[&note("doc.txt", "def add(a, b):", "It adds."), done], cx);
    t.ask("what is this").keys("tab");
    assert!(!t.reviewing());
    assert_eq!(ghostwrite_ai::memory::notes(&t.dir), "The user is learning Python.");
    let logged = ghostwrite_ai::memory::recent(&t.dir, 5);
    assert_eq!(logged.len(), 1);
    assert_eq!(logged[0]["request"], "what is this");
    assert_eq!(logged[0]["outcome"], "answered");
    assert_eq!(logged[0]["followups"], serde_json::json!([]), "asking for the next item isn't something the user said");
}

#[gpui::test]
fn docs_notes_leave_the_cursor_alone(cx: &mut TestAppContext) {
    let mut t = open("def f():\n    return compute(1, 2)\n", &[DONE], cx);
    t.keys("j w w");
    let before = t.cursor();
    t.editor.update(t.cx, |e, cx| e.show_note("compute(a, b): adds", cx));
    t.cx.run_until_parked();
    assert!(t.reviewing(), "the docs show");
    assert_eq!(t.cursor(), before, "and the cursor stays put");
    t.keys("tab");
    assert!(!t.reviewing());
}

#[gpui::test]
fn a_tour_walks_through_other_files(cx: &mut TestAppContext) {
    let start = note("doc.txt", "fn main() { run(); }", "It starts here.");
    let run = note("src/lib.rs", "pub fn run() {", "The work happens in run.");
    let helper = note("src/lib.rs", "    helper();", "Which calls the helper.");
    let back = note("doc.txt", "fn main() { run(); }", "And that's all main does.");
    let mut t = open("fn main() { run(); }\n", &[&start, &run, &helper, &back, DONE], cx);
    std::fs::create_dir_all(t.dir.join("src")).unwrap();
    std::fs::write(t.dir.join("src/lib.rs"), "pub fn run() {\n    helper();\n}\n").unwrap();
    t.ask("give me a tour");
    assert_eq!(t.active().as_deref(), Some("doc.txt"));
    t.keys("tab");
    assert_eq!(t.active().as_deref(), Some("src/lib.rs"), "a note names its file");
    assert_eq!(t.cursor(), 0);
    t.keys("tab");
    assert_eq!(t.cursor(), "pub fn run() {\n".len());
    t.keys("tab");
    assert_eq!(t.active().as_deref(), Some("doc.txt"));
    assert_eq!(t.read(|e| e.project.tabs.len()), 1, "the tab the tour opened closes as it moves on");
    t.keys("tab");
    assert!(!t.reviewing());
}

#[gpui::test]
fn a_change_edits_other_existing_files(cx: &mut TestAppContext) {
    let lib = edit("src/lib.rs", "pub fn run() {}", "pub fn run(verbose: bool) {}");
    let main = edit("doc.txt", "fn main() { run(); }", "fn main() { run(true); }");
    let mut t = open("fn main() { run(); }\n", &[&lib, &main, DONE], cx);
    std::fs::create_dir_all(t.dir.join("src")).unwrap();
    std::fs::write(t.dir.join("src/lib.rs"), "pub fn run() {}\n").unwrap();
    // Unsaved changes elsewhere are written before the model reads the project.
    t.command("e src/lib.rs").keys("x y p").command("e doc.txt").keys("u");
    t.ask("add a verbose flag");
    assert_eq!(t.active().as_deref(), Some("src/lib.rs"), "the first edit is in another file");
    let shown = |req: ghostwrite_ai::Request| req.others.into_iter().map(|o| (o.path, o.text)).collect::<Vec<_>>();
    let ahead = t.read(|e| {
        let state = e.story.as_ref().unwrap();
        let item = state.story.as_ref().unwrap().current_item().unwrap();
        state.request_ahead(&e.project, &[item]).unwrap()
    });
    assert_eq!(
        shown(ahead),
        [("src/lib.rs".to_string(), "pub fn run(verbose: bool) {}\npub fn run() {}\n".to_string())],
        "asked ahead, Claude sees the other file as if the edit there were accepted"
    );
    t.keys("tab");
    let now = t.read(|e| e.story.as_ref().unwrap().request(&e.project).unwrap());
    assert_eq!(shown(now).len(), 1, "and goes on seeing the other files the story is in");
    assert_eq!(
        t.read(|e| e.project.tabs.text_of(&e.project.root.join("src/lib.rs")).map(str::to_string)).as_deref(),
        Some("pub fn run(verbose: bool) {}\npub fn run() {}\n")
    );
    assert_eq!(t.active().as_deref(), Some("doc.txt"), "back for the next edit");
    t.keys("tab");
    assert_eq!(t.text(), "fn main() { run(true); }\n");
    assert!(!t.reviewing());
}

#[gpui::test]
fn a_quoted_line_number_picks_the_copy_meant(cx: &mut TestAppContext) {
    let text = "if a {\n    done();\n}\nif b {\n    done();\n}\n";
    let mut t = open(text, &[&edit("doc.txt", "5│    done();", "    finish();"), DONE], cx);
    t.ask("finish b");
    t.keys("tab");
    assert_eq!(t.text(), "if a {\n    done();\n}\nif b {\n    finish();\n}\n", "not the copy nearer the cursor");
}

#[gpui::test]
fn ai_mode_reviews_and_goes_back_to_where_you_were(cx: &mut TestAppContext) {
    let mut t = open(ADD, &[&fix(), &hints(), DONE], cx);
    // From select mode: the selection is the context, and comes back after.
    t.keys("v j");
    let selected = t.read(|e| e.tab().unwrap().view.selection);
    t.keys("space a");
    assert_eq!(t.read(|e| e.modal.mode), Mode::Ai);
    t.input("implement it").keys("enter");
    // Looking around works; editing doesn't.
    t.keys("g g d x");
    assert_eq!(t.text(), ADD, "no edits in AI mode");
    t.keys("escape escape");
    assert_eq!(t.read(|e| e.modal.mode), Mode::Select, "back to select mode");
    assert_eq!(t.read(|e| e.tab().unwrap().view.selection), selected, "with the selection");
}

#[gpui::test]
fn ai_mode_from_the_tree_goes_back_to_the_tree(cx: &mut TestAppContext) {
    let mut t = open(ADD, &[&fix(), &hints(), DONE], cx);
    t.keys("space e");
    assert_eq!(t.read(|e| e.modal.mode), Mode::Tree);
    t.keys("space a");
    assert_eq!(t.read(|e| e.modal.mode), Mode::Ai);
    t.input("implement it").keys("enter").keys("tab");
    assert_eq!(t.text(), "def add(a, b):\n    return a + b\n");
    t.keys("tab");
    assert_eq!(t.read(|e| e.modal.mode), Mode::Tree);
}

#[gpui::test]
fn a_follow_up_is_answered_where_it_was_asked(cx: &mut TestAppContext) {
    let lib = edit("src/lib.rs", "pub fn run() {}", "pub fn run(verbose: bool) {}");
    let mut t = open("fn main() { run(); }\n", &[&lib, &note("src/lib.rs", "", "So callers can ask for detail."), DONE], cx);
    std::fs::create_dir_all(t.dir.join("src")).unwrap();
    std::fs::write(t.dir.join("src/lib.rs"), "\n\npub fn run() {}\n").unwrap();
    t.ask("add a verbose flag");
    assert_eq!(t.active().as_deref(), Some("src/lib.rs"));
    // Asked about the edit in lib.rs: the answer shows there, under it.
    t.keys("enter").input("why?").keys("enter");
    let at = t.read(|e| {
        let state = e.story.as_ref().unwrap();
        let story = state.story.as_ref().unwrap();
        let located = state.locate(&e.project, story.current().unwrap()).unwrap();
        (located.tab.map(|ix| e.project.tabs.get(ix).unwrap().doc.name().to_string()), located.near)
    });
    assert_eq!(at, (Some("lib.rs".into()), 2..3), "under the edit's line");
    assert_eq!(t.active().as_deref(), Some("src/lib.rs"), "and the view stays there");
}

#[gpui::test]
fn a_comment_changes_what_comes_next(cx: &mut TestAppContext) {
    let types = edit("doc.txt", "def add(a, b):", "def add(a: int, b: int):");
    let mut t = open(ADD, &[&note("doc.txt", "def add(a, b):", "It adds."), &types, DONE], cx);
    t.ask("walk me through this");
    t.keys("enter").input("I care about the types, not the body").keys("enter");
    t.keys("tab");
    assert_eq!(t.text(), "def add(a: int, b: int):\n    pass\n", "the next item followed what was said");
    assert!(!t.reviewing());
}

#[gpui::test]
fn the_tree_marks_where_the_story_is_waiting(cx: &mut TestAppContext) {
    let lib = edit("src/lib.rs", "pub fn run() {}", "pub fn run(verbose: bool) {}");
    let main = edit("doc.txt", "fn main() { run(); }", "fn main() { run(true); }");
    let mut t = open("fn main() { run(); }\n", &[&lib, &main, DONE], cx);
    std::fs::create_dir_all(t.dir.join("src")).unwrap();
    std::fs::write(t.dir.join("src/lib.rs"), "pub fn run() {}\n").unwrap();
    let waiting = |t: &mut Setup| {
        let dir = t.dir.clone();
        let mut files: Vec<(String, ghostwrite_view::story::Waiting)> = t
            .read(|e| e.story.as_ref().map(|s| s.files(&e.project)).unwrap_or_default())
            .into_iter()
            .map(|(p, w)| (p.strip_prefix(&dir).unwrap().display().to_string(), w))
            .collect();
        files.sort();
        files
    };
    t.keys("space a");
    assert_eq!(waiting(&mut t), [("doc.txt".into(), ghostwrite_view::story::Waiting::Here)], "asking about this file");
    t.input("add a verbose flag").keys("enter");
    assert_eq!(waiting(&mut t), [("src/lib.rs".into(), ghostwrite_view::story::Waiting::Here)]);
    t.keys("tab");
    assert_eq!(waiting(&mut t), [("doc.txt".into(), ghostwrite_view::story::Waiting::Here)], "on to the next item");
}

#[gpui::test]
fn claude_hears_the_whole_conversation_and_the_editor(cx: &mut TestAppContext) {
    let mut t = open(ADD, &[&fix(), &hints(), &note("doc.txt", "", "OK."), DONE], cx);
    std::fs::write(t.dir.join("other.py"), "x = 1\n").unwrap();
    t.command("e other.py").command("e doc.txt");
    t.ask("types and body").keys("tab");
    t.keys("enter").input("why int?").keys("enter");
    let req = t.read(|e| e.story.as_ref().unwrap().request(&e.project).unwrap());
    assert_eq!(req.surroundings.open_files, ["doc.txt", "other.py"], "what is open, in tab order");
    assert_eq!(req.followups[0].reply, fix(), "each reply, whole");
    assert_eq!(req.followups[0].decided.len(), 1, "what was accepted");
    assert_eq!(req.followups[0].said, "", "then the next item was asked for");
    assert_eq!(req.followups[1].said, "why int?");
}

#[gpui::test]
fn an_edit_that_doesnt_apply_is_rewritten_first(cx: &mut TestAppContext) {
    let wrong = edit("doc.txt", "    return None", "    return a + b");
    let mut t = open(ADD, &[&wrong, &fix(), DONE], cx);
    t.ask("implement it");
    let followups = t.read(|e| e.story.as_ref().unwrap().story.as_ref().unwrap().followups().to_vec());
    assert_eq!(followups.len(), 1, "the model was told, without the user");
    assert!(followups[0].correction && followups[0].said.contains("aren't in doc.txt"), "{:?}", followups[0]);
    t.keys("tab");
    assert_eq!(t.text(), "def add(a, b):\n    return a + b\n", "the rewritten edit is the one shown");
    assert!(!t.reviewing());
}

#[gpui::test]
fn a_note_whose_lines_arent_there_is_rewritten_first(cx: &mut TestAppContext) {
    let wrong = note("doc.txt", "def add(a, b): ...", "It adds.");
    let mut t = open(ADD, &[&wrong, &note("doc.txt", "def add(a, b):", "It adds."), DONE], cx);
    t.ask("what is this");
    let followups = t.read(|e| e.story.as_ref().unwrap().story.as_ref().unwrap().followups().to_vec());
    assert!(followups.len() == 1 && followups[0].correction, "{followups:?}");
    assert_eq!(t.cursor(), 0, "the corrected note is at its line");
}

#[gpui::test]
fn an_item_in_a_missing_file_is_rewritten_first(cx: &mut TestAppContext) {
    let mut t = open(ADD, &[&edit("src/doc.txt", "    pass", "    return a + b"), &fix(), DONE], cx);
    t.ask("implement it");
    let followups = t.read(|e| e.story.as_ref().unwrap().story.as_ref().unwrap().followups().to_vec());
    assert!(followups[0].correction && followups[0].said.contains("no file src/doc.txt"), "{:?}", followups[0]);
    assert!(!t.dir.join("src/doc.txt").exists(), "nothing was made for it");
    t.keys("tab");
    assert_eq!(t.text(), "def add(a, b):\n    return a + b\n");
}

#[gpui::test]
fn an_item_about_a_pdf_is_rewritten_about_another_file(cx: &mut TestAppContext) {
    let mut t = open(ADD, &[&note("paper.pdf", "", "It says hello."), &fix(), DONE], cx);
    std::fs::write(t.dir.join("paper.pdf"), "%PDF-1.4").unwrap();
    t.ask("implement it");
    let followups = t.read(|e| e.story.as_ref().unwrap().story.as_ref().unwrap().followups().to_vec());
    assert!(followups[0].correction && followups[0].said.contains("only pages"), "{:?}", followups[0]);
    t.keys("tab");
    assert_eq!(t.text(), "def add(a, b):\n    return a + b\n", "the next reply is used");
}

#[gpui::test]
fn a_reply_without_the_format_is_asked_again(cx: &mut TestAppContext) {
    let mut t = open(ADD, &["Sure, I'd return a + b.", &fix(), DONE], cx);
    t.ask("implement it");
    assert!(t.reviewing(), "the story didn't just end");
    t.keys("tab");
    assert_eq!(t.text(), "def add(a, b):\n    return a + b\n");
}

#[gpui::test]
fn a_reply_that_breaks_the_format_is_stopped_and_asked_again(cx: &mut TestAppContext) {
    let broken = "<file>\ndoc.txt\n</file>\n<say>\nNo <at>.\n</say>\n";
    let mut t = open(ADD, &[broken, &fix(), DONE], cx);
    t.ask("implement it");
    let followups = t.read(|e| e.story.as_ref().unwrap().story.as_ref().unwrap().followups().to_vec());
    assert!(followups[0].correction && followups[0].said.contains("<at>"), "{:?}", followups[0]);
    t.keys("tab");
    assert_eq!(t.text(), "def add(a, b):\n    return a + b\n");
}

#[gpui::test]
fn corrections_stop_after_a_few_tries(cx: &mut TestAppContext) {
    let wrong = edit("doc.txt", "    return None", "    return a + b");
    let mut t = open(ADD, &[&wrong, &wrong, &wrong, &wrong, DONE], cx);
    t.ask("implement it");
    let n = t.read(|e| e.story.as_ref().unwrap().story.as_ref().unwrap().followups().len());
    assert_eq!(n, ghostwrite_ai::story::MAX_REVISIONS);
    assert!(t.reviewing(), "the edit is left to redirect");
    t.keys("enter").input("never mind").keys("enter");
    assert!(!t.reviewing());
}

#[gpui::test]
fn switching_to_a_storys_file_keeps_it_centered_on_the_item(cx: &mut TestAppContext) {
    let t = open("x\n", &[], cx);
    std::fs::write(t.dir.join("other.txt"), "a\nb\nc\n").unwrap();
    let path = t.dir.join("other.txt");
    let scroll = t.editor.update(t.cx, |e, _| {
        let ix = e.project.open(&path).unwrap();
        e.project.tabs.get_mut(ix).unwrap().select(ghostwrite_core::Range::point(2), crate::editor::Autoscroll::Center(1..2));
        e.project.activate(ix);
        e.tab().unwrap().view.autoscroll.clone()
    });
    assert_eq!(scroll, Some(crate::editor::Autoscroll::Center(1..2)));
}

#[gpui::test]
fn a_whole_story_is_undone_at_once(cx: &mut TestAppContext) {
    let mut t = open(ADD, &[&fix(), &hints(), DONE], cx);
    t.ask("implement it").keys("tab tab");
    assert!(!t.reviewing());
    assert_ne!(t.text(), ADD);
    t.command("undo-story");
    assert_eq!(t.text(), ADD, "both steps, in one go");
    assert_eq!(t.status().as_deref(), Some("undid 2 steps in 1 file: not yet saved"));
    t.command("undo-story");
    assert_eq!(t.status().as_deref(), Some("no story to undo"));
}

#[gpui::test]
fn a_story_isnt_undone_over_later_edits(cx: &mut TestAppContext) {
    let mut t = open(ADD, &[&fix(), DONE], cx);
    t.ask("implement it").keys("tab");
    assert!(!t.reviewing());
    t.keys("g g i").input("# mine\n").keys("escape");
    let edited = t.text();
    t.command("undo-story");
    assert_eq!(t.text(), edited, "what was typed since is left alone");
    assert!(t.status().is_some_and(|s| s.contains("has been edited since")));
}

#[gpui::test]
fn replacing_goes_through_the_project_as_steps(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let mut t = open("old one\nkeep\nold old\n", &[], cx);
    t.cx.update(|_, _| ());
    std::fs::write(t.dir.join("other.txt"), "an old line\n").unwrap();
    t.command("replace old new");
    for _ in 0..200 {
        t.cx.run_until_parked();
        if t.reviewing() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(t.reviewing(), "each changing line is a step: {:?}", t.status());
    assert_eq!(t.text(), "old one\nkeep\nold old\n", "nothing yet");
    t.keys("tab tab tab");
    assert!(!t.reviewing());
    let doc = t.read(|e| e.project.tabs.iter().find(|tab| tab.doc.name().as_ref() as &str == "doc.txt").unwrap().doc.buffer.text().to_string());
    let other = t.read(|e| e.project.tabs.iter().find(|tab| tab.doc.name().as_ref() as &str == "other.txt").map(|tab| tab.doc.buffer.text().to_string()));
    assert_eq!(doc, "new one\nkeep\nnew new\n");
    assert_eq!(other.unwrap_or_else(|| std::fs::read_to_string(t.dir.join("other.txt")).unwrap()), "an new line\n");
}

#[gpui::test]
fn replacing_with_a_bang_takes_them_all(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let mut t = open("old one\nold two\n", &[], cx);
    t.command("replace! old new");
    for _ in 0..200 {
        t.cx.run_until_parked();
        if t.text() != "old one\nold two\n" {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(t.text(), "new one\nnew two\n");
    assert!(!t.reviewing());
    t.command("undo-story");
    assert_eq!(t.text(), "old one\nold two\n", "and they're taken back the same way");
}

/// Draw, and draw again: what a frame shows of the story is where the next one's light goes.
fn draw(t: &mut Setup) {
    for _ in 0..2 {
        t.cx.update(|window, _| window.refresh());
        t.cx.run_until_parked();
    }
}

/// How many times the light has jumped, and where to, last.
fn jumped(t: &mut Setup) -> (u64, Option<gpui::Bounds<gpui::Pixels>>) {
    t.read(|e| (e.attention.jumps, e.attention.jump.map(|jump| jump.to)))
}

#[gpui::test]
fn the_eye_is_led_to_each_step_wherever_it_is(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let run = "<run>\necho hi\n</run>\n<say>\nCheck.\n</say>\n";
    let create = "<create>\nnew.txt\n</create>\n<say>\nA new file.\n</say>\n<new>\nhello\n</new>\n";
    let other = edit("other.txt", "old", "new");
    let mut t = open(ADD, &[&fix(), &other, create, run, DONE], cx);
    t.cx.simulate_resize(gpui::size(gpui::px(1200.), gpui::px(900.)));
    std::fs::write(t.dir.join("other.txt"), "old\n").unwrap();
    t.keys("space a");
    draw(&mut t);
    t.input("go on").keys("enter");
    draw(&mut t);
    let (first, _) = jumped(&mut t);
    assert!(first >= 1, "from the prompt to the first step");

    let inside = |t: &mut Setup, name: &'static str| {
        let (_, to) = jumped(t);
        let to = to.expect("it's on its way");
        let area = t.cx.debug_bounds(name).unwrap_or_else(|| panic!("{name} isn't drawn"));
        assert!(area.contains(&to.center()), "to {name}: {to:?} in {area:?}");
    };
    t.keys("tab");
    draw(&mut t);
    assert!(jumped(&mut t).0 > first, "to the next step, in another file");
    inside(&mut t, "code-view");

    let before = jumped(&mut t).0;
    t.keys("tab");
    draw(&mut t);
    assert!(jumped(&mut t).0 > before, "a new file is proposed in the tree");
    inside(&mut t, "tree-pane");
    assert_eq!(t.read(|e| e.modal.mode), Mode::Ai, "the keys stay with the story");

    let before = jumped(&mut t).0;
    t.keys("tab");
    draw(&mut t);
    assert!(jumped(&mut t).0 > before, "the command waits in the terminal");
    inside(&mut t, "terminal-screen");
}

#[gpui::test]
fn thinking_isnt_on_a_line_but_in_the_corner(cx: &mut TestAppContext) {
    let mut t = open(ADD, &[&fix(), &hints(), DONE], cx);
    t.cx.simulate_resize(gpui::size(gpui::px(1200.), gpui::px(900.)));
    t.ask("implement it");
    draw(&mut t);
    assert_eq!(t.read(|e| e.story_mark()), Some(crate::editor::attention::Mark::Item(0)), "a step's up: nothing's thinking");
    let before = jumped(&mut t).0;
    // Said something: the model's writing the next step, and nothing's shown of it yet.
    t.editor.update(t.cx, |e, _| e.story.as_mut().unwrap().story.as_mut().unwrap().follow_up("more".into()));
    draw(&mut t);
    let corner = t.cx.debug_bounds("story-thinking").expect("thinking, in the corner");
    let code = t.cx.debug_bounds("code-view").unwrap();
    assert!(corner.right() > code.center().x && corner.top() < code.top() + gpui::px(40.), "top right: {corner:?} of {code:?}");
    assert_eq!(jumped(&mut t).0, before, "it isn't somewhere to look: no light goes there");
    let editor = t.editor.clone();
    let block = t.cx.update(|_, cx| {
        let theme = crate::theme::theme(cx).clone();
        let e = editor.read(cx);
        e.story.as_ref().unwrap().overlay(&e.project, e.tab().unwrap(), None, &theme).1.is_some()
    });
    assert!(!block, "and not under any line");
}

#[gpui::test]
fn the_next_step_in_the_same_file_is_jumped_to_as_well(cx: &mut TestAppContext) {
    let mut t = open(ADD, &[&fix(), &hints(), DONE], cx);
    t.ask("implement it");
    draw(&mut t);
    let before = jumped(&mut t).0;
    t.keys("tab");
    draw(&mut t);
    assert_eq!(t.current(), Some(1));
    assert!(jumped(&mut t).0 > before, "another step, even in the same file");
}

#[gpui::test]
fn looking_around_while_it_thinks_isnt_interrupted_and_tab_comes_back(cx: &mut TestAppContext) {
    use crate::editor::attention::Mark;
    let other = edit("other.txt", "old", "new");
    let mut t = open(ADD, &[&fix(), &other, DONE], cx);
    std::fs::write(t.dir.join("other.txt"), "old\n").unwrap();
    t.ask("go on");
    assert_eq!(t.current(), Some(0));
    // The model's writing the next step, and meanwhile you look around.
    t.editor.update(t.cx, |e, _| e.story.as_mut().unwrap().story.as_mut().unwrap().follow_up("and the other file".into()));
    t.keys("k");
    let looking = (t.active(), t.cursor());
    // It arrives, in another file.
    let editor = t.editor.clone();
    t.cx.update(|_, cx| {
        editor.update(cx, |e, cx| {
            let story = e.story.as_mut().unwrap().story.as_mut().unwrap();
            story.push_text(&other);
            story.finished_writing();
            e.settle_story(cx);
        })
    });
    t.cx.run_until_parked();
    assert_eq!((t.active(), t.cursor()), looking, "you stay where you were");
    assert!(matches!(t.read(|e| e.story_mark()), Some(Mark::Ready(_))), "it waits, in the corner");

    t.keys("tab");
    assert_eq!(t.active().as_deref(), Some("other.txt"), "the first Tab takes you to it");
    assert_eq!(t.file("other.txt"), "old\n", "and accepts nothing");
    t.keys("tab");
    assert_eq!(t.file("other.txt"), "new\n", "the next one accepts it");
}

#[gpui::test]
fn having_looked_away_from_a_step_tab_comes_back_to_it_first(cx: &mut TestAppContext) {
    let mut t = open(ADD, &[&fix(), DONE], cx);
    t.ask("implement it");
    let at = t.cursor();
    t.keys("k");
    assert_ne!(t.cursor(), at, "free to move");
    t.keys("tab");
    assert_eq!(t.text(), ADD, "the first Tab doesn't accept");
    assert_eq!(t.cursor(), at, "it comes back to the step");
    t.keys("tab");
    assert_eq!(t.text(), "def add(a, b):\n    return a + b\n", "the next one does");
}

#[gpui::test]
fn files_can_be_opened_while_a_story_is_up(cx: &mut TestAppContext) {
    let mut t = open(ADD, &[&fix(), DONE], cx);
    t.ask("implement it");
    t.keys("space f");
    assert!(t.read(|e| e.picker.is_some()), "space f opens the file picker in AI mode too");
}

#[gpui::test]
fn the_tree_can_be_used_while_a_story_is_up(cx: &mut TestAppContext) {
    let mut t = open(ADD, &[&fix(), DONE], cx);
    std::fs::write(t.dir.join("notes.txt"), "elsewhere\n").unwrap();
    t.editor.update(t.cx, |e, _| e.project.tree.refresh());
    t.ask("implement it");
    let at = t.cursor();
    t.keys("space e");
    assert_eq!(t.read(|e| e.modal.mode), Mode::Tree, "space e goes to the tree");
    let row = t.read(|e| e.project.tree.rows().iter().position(|r| r.name == "notes.txt")).expect("in the tree");
    t.editor.update(t.cx, |e, _| e.project.tree.move_to(row));
    t.keys("enter");
    assert_eq!(t.active().as_deref(), Some("notes.txt"), "and opens a file from there");
    assert_eq!(t.read(|e| e.modal.mode), Mode::Ai, "back in the story");
    t.keys("tab");
    assert_eq!((t.active().as_deref(), t.cursor()), (Some("doc.txt"), at), "Tab comes back to the step");
    assert_eq!(t.text(), ADD);
    t.keys("tab");
    assert_eq!(t.text(), "def add(a, b):\n    return a + b\n");
}

#[gpui::test]
fn esc_out_of_the_tree_goes_back_to_the_story(cx: &mut TestAppContext) {
    let mut t = open(ADD, &[&fix(), DONE], cx);
    t.ask("implement it");
    t.keys("space e escape");
    assert_eq!(t.read(|e| e.modal.mode), Mode::Ai);
    assert!(t.reviewing());
}

#[gpui::test]
fn with_nothing_open_space_a_still_asks(cx: &mut TestAppContext) {
    let create = "<create>\nnotes.md\n</create>\n<say>\nA place for them.\n</say>\n<new>\n# Notes\n</new>\n";
    let mut t = open("", &[create, DONE], cx);
    t.command("bc");
    assert!(t.read(|e| e.tab().is_none()), "nothing's open");
    t.ask("start a notes file");
    assert!(t.reviewing(), "it's asked");
    t.keys("tab");
    assert_eq!(t.file("notes.md"), "# Notes\n");
}

#[gpui::test]
fn waiting_for_the_model_is_timed(cx: &mut TestAppContext) {
    let mut t = open(ADD, &[&fix(), &hints(), DONE], cx);
    t.ask("implement it");
    assert!(t.read(|e| e.story.as_ref().unwrap().timer.is_none()), "the first step shown ends the wait");
    t.keys("tab");
    assert!(t.read(|e| e.story.as_ref().unwrap().timer.is_none()), "and the next one, asked ahead");
    t.command("timings");
    let said = t.status().unwrap_or_default();
    assert!(said.contains("ask (") && said.contains("next step ("), "{said}");
}

#[gpui::test]
fn an_accepted_step_lights_up_what_it_changed(cx: &mut TestAppContext) {
    let mut t = open(ADD, &[&fix(), DONE], cx);
    t.ask("implement it").keys("tab");
    let flash = t.read(|e| e.tab().unwrap().view.motion.flash.clone().map(|(range, _)| range));
    let text = t.text();
    assert_eq!(flash.map(|range| text[range].to_string()).as_deref(), Some("    return a + b\n"));
}

#[gpui::test]
fn reading_outside_the_project_waits_for_tab_or_enter(cx: &mut TestAppContext) {
    let mut t = open(ADD, &[&fix(), &hints(), DONE], cx);
    t.ask("implement it");
    let ask = |t: &mut Setup| {
        let (tx, rx) = std::sync::mpsc::channel();
        t.editor.update(t.cx, |e, _| e.story.as_mut().unwrap().may_read("/etc/hosts".into(), tx));
        rx
    };
    let answered = ask(&mut t);
    t.keys("tab");
    assert_eq!(answered.try_recv(), Ok(true), "Tab lets it");
    assert_eq!(t.text(), ADD, "and accepts nothing");
    let answered = ask(&mut t);
    t.keys("enter");
    assert_eq!(answered.try_recv(), Ok(false), "Enter doesn't");
    assert!(!t.read(|e| e.asking()), "and opens no prompt");
    let answered = ask(&mut t);
    t.keys("escape escape");
    assert!(!t.reviewing());
    assert_eq!(answered.try_recv(), Err(std::sync::mpsc::TryRecvError::Disconnected), "ending the story is no answer: no");
}

#[gpui::test]
fn clear_starts_over_with_nothing_remembered(cx: &mut TestAppContext) {
    let done = "<done>\n<memory>\nThe user is learning Python.\n</memory>\n</done>\n";
    let mut t = open(ADD, &[&note("doc.txt", "def add(a, b):", "It adds."), done, &fix()], cx);
    t.ask("what is this").keys("tab");
    assert!(!ghostwrite_ai::memory::notes(&t.dir).is_empty() && !ghostwrite_ai::memory::recent(&t.dir, 5).is_empty());
    t.ask("and now");
    t.command("clear");
    assert!(!t.reviewing(), "the story up ends");
    assert_eq!(ghostwrite_ai::memory::notes(&t.dir), "", "Claude's notes are gone");
    assert!(ghostwrite_ai::memory::recent(&t.dir, 5).is_empty(), "and the earlier requests");
    assert!(t.status().is_some_and(|s| s.starts_with("starting over")), "{:?}", t.status());
    t.ask("again");
    let prompt = t.read(|e| e.story.as_ref().unwrap().request(&e.project).unwrap().prompt());
    assert!(!prompt.contains("what is this") && !prompt.contains("learning Python"), "the next story is told nothing of them");
}
