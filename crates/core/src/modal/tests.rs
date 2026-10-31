use super::*;

#[derive(Default)]
struct Regs {
    yanked: String,
    named: std::collections::HashMap<char, String>,
    clipboard: Option<String>,
}

impl Registers for Regs {
    fn yanked(&self, register: Option<char>) -> String {
        register.and_then(|r| self.named.get(&r).cloned()).unwrap_or_else(|| self.yanked.clone())
    }
    fn yank(&mut self, register: Option<char>, text: String) {
        match register {
            Some(r) => {
                self.named.insert(r, text);
            }
            None => self.yanked = text,
        }
    }
    fn clipboard(&self) -> Option<String> {
        self.clipboard.clone()
    }
    fn set_clipboard(&mut self, text: String) {
        self.clipboard = Some(text);
    }
}

/// A buffer driven by typed keys, as in the editor.
struct Session {
    modal: Modal,
    buffer: Buffer,
    selection: Range,
    regs: Regs,
}

impl Session {
    fn new(text: &str) -> Self {
        Self { modal: Modal::default(), buffer: Buffer::new(text), selection: Range::point(0), regs: Regs::default() }
    }

    /// Space-separated keys; `esc` and `ret` are named, anything else
    /// longer than one char is typed text.
    fn keys(&mut self, keys: &str) -> &mut Self {
        let indent = IndentUnit::default();
        for word in keys.split(' ') {
            let key = match word {
                "esc" => Some(Key::Escape),
                "ret" => Some(Key::Enter),
                "space" => Some(Key::Char(' ')),
                w if w.starts_with("ctrl-") => w.chars().last().map(Key::Ctrl),
                w if w.chars().count() == 1 && self.modal.mode != Mode::Insert => Some(Key::Char(w.chars().next().unwrap())),
                _ => None,
            };
            let mut t = Target { buffer: &mut self.buffer, selection: &mut self.selection, indent: &indent, page_rows: 20, language: None };
            match key {
                Some(key) => {
                    self.modal.key(key, &mut self.regs, &mut t);
                }
                None => {
                    self.modal.insert(word, &mut t);
                }
            }
        }
        self
    }

    fn text(&self) -> &str {
        self.buffer.text()
    }
}

#[test]
fn word_then_delete() {
    let mut s = Session::new("foo bar baz\n");
    assert_eq!(s.keys("w d").text(), "bar baz\n");
    // A count repeats the motion; only the last word stays selected.
    assert_eq!(s.keys("2 w d").text(), "bar \n");
    assert_eq!(s.keys("u").text(), "bar baz\n");
}

#[test]
fn lines_yank_paste_and_open() {
    let mut s = Session::new("one\ntwo\n");
    assert_eq!(s.keys("x y j p").text(), "one\ntwo\none\n");
    assert_eq!(s.keys("g g x d").text(), "two\none\n");
    assert_eq!(s.keys("o new esc").text(), "two\nnew\none\n");
    assert_eq!(s.keys("A ! esc g e d").text(), "two\nnew!\nne\n");
}

#[test]
fn an_insert_session_is_one_undo_step() {
    let mut s = Session::new("x\n");
    s.keys("o one ret two esc");
    assert_eq!(s.text(), "x\none\ntwo\n");
    assert_eq!(s.keys("u").text(), "x\n");
}

#[test]
fn find_and_change() {
    let mut s = Session::new("let x = foo(bar);\n");
    assert_eq!(s.keys("g l F ( c ; esc").text(), "let x = foo;\n");
    assert_eq!(s.keys("u g h f = d").text(), " foo(bar);\n", "f selects forward to the char, as in helix");
    assert_eq!(s.keys("u g h t ( d").text(), "(bar);\n", "t stops before it");
}

#[test]
fn a_appends_after_the_selection() {
    let mut s = Session::new("one two\n");
    s.keys("w a");
    assert_eq!(s.modal.mode, Mode::Insert);
    assert_eq!(s.selection, Range::point("one ".len()), "after the selection: the word and its space");
}

#[test]
fn match_mode_selects_and_surrounds() {
    let mut s = Session::new("call(a, b)\n");
    assert_eq!(s.keys("l l l l l m i ( d").text(), "call()\n");
    s.keys("u");
    assert_eq!(s.keys("g h m i w m s '").text(), "'call'(a, b)\n");
    assert_eq!(s.keys("g h l m d '").text(), "call(a, b)\n");
    assert_eq!(s.keys("g l h m r ( [").text(), "call[a, b]\n");
    s.keys("g l m m");
    assert_eq!(s.selection.head, 4);
}

#[test]
fn normal_mode_swallows_letters() {
    let mut s = Session::new("abc\n");
    let indent = IndentUnit::default();
    let mut t = Target { buffer: &mut s.buffer, selection: &mut s.selection, indent: &indent, page_rows: 20, language: None };
    assert!(!s.modal.insert("m", &mut t));
    assert_eq!(s.modal.key(Key::Char('z'), &mut s.regs, &mut t), Outcome::Handled);
    assert_eq!(s.text(), "abc\n");
}

#[test]
fn tree_mode_turns_keys_into_tree_commands() {
    let mut m = Modal::default();
    m.to_tree();
    assert_eq!(m.tree_key(Key::Char('3')), Outcome::Handled);
    assert_eq!(m.tree_key(Key::Char('j')), Outcome::Tree(TreeCommand::Down(3)));
    assert_eq!(m.tree_key(Key::Char('g')), Outcome::Handled);
    assert_eq!(m.tree_key(Key::Char('g')), Outcome::Tree(TreeCommand::First));
    assert_eq!(m.tree_key(Key::Char('x')), Outcome::Handled, "other letters are swallowed");
    assert_eq!(m.tree_key(Key::Char(':')), Outcome::Effect(Effect::CommandLine));
    assert_eq!(m.tree_key(Key::Escape), Outcome::Tree(TreeCommand::Leave));
    assert_eq!(m.mode, Mode::Normal);
    m.to_tree();
    assert_eq!(m.tree_key(Key::Char(' ')), Outcome::Handled);
    assert_eq!(m.tree_key(Key::Char('a')), Outcome::Effect(Effect::Assist), "space a asks, as everywhere");
    assert_eq!(m.tree_key(Key::Char(' ')), Outcome::Handled);
    assert_eq!(m.tree_key(Key::Char('e')), Outcome::Tree(TreeCommand::Leave), "space e goes back too");
    assert_eq!(m.mode, Mode::Normal);
}

#[test]
fn review_modes_take_review_keys_and_go_back_where_they_came_from() {
    let mut s = Session::new("one two\n");
    s.keys("v w");
    s.modal.enter(Mode::Ai);
    assert_eq!(s.modal.entered_from(), Some(Mode::Select));
    let indent = IndentUnit::default();
    let mut t = Target { buffer: &mut s.buffer, selection: &mut s.selection, indent: &indent, page_rows: 20, language: None };
    assert_eq!(s.modal.key(Key::Tab, &mut s.regs, &mut t), Outcome::Review(Review::Next));
    assert_ne!(s.modal.key(Key::BackTab, &mut s.regs, &mut t), Outcome::Review(Review::Next), "a story only goes forward");
    s.modal.leave(Mode::Ai);
    assert_eq!(s.modal.mode, Mode::Select, "back to where it started");
    assert_eq!(s.modal.menu_key(Key::Ctrl('n')), Some(MenuKey::Next));
}

#[test]
fn reviewing_moves_but_never_edits() {
    let mut s = Session::new("one two\nthree\n");
    s.modal.enter(Mode::Ai);
    s.keys("w j d c x p u ~ > r z m s ( i o");
    assert_eq!(s.text(), "one two\nthree\n", "nothing changes");
    assert_eq!(s.modal.mode, Mode::Ai, "and the mode stays");
    s.keys("a v");
    assert_eq!(s.modal.mode, Mode::Ai, "nothing new starts mid-review");
}

#[test]
fn leaving_a_mode_below_the_current_one_drops_it() {
    let mut m = Modal::default();
    m.enter(Mode::Tree);
    m.enter(Mode::Ai);
    m.leave(Mode::Tree);
    assert_eq!(m.mode, Mode::Ai);
    m.leave(Mode::Ai);
    assert_eq!(m.mode, Mode::Normal);
}

#[test]
fn space_e_enters_tree_mode() {
    let mut s = Session::new("abc\n");
    let indent = IndentUnit::default();
    let mut t = Target { buffer: &mut s.buffer, selection: &mut s.selection, indent: &indent, page_rows: 20, language: None };
    s.modal.key(Key::Char(' '), &mut s.regs, &mut t);
    assert_eq!(s.modal.key(Key::Char('e'), &mut s.regs, &mut t), Outcome::Effect(Effect::FocusTree));
    assert_eq!(s.modal.mode, Mode::Tree);
}

#[test]
fn effects_come_back() {
    let mut s = Session::new("abc\n");
    let indent = IndentUnit::default();
    let mut t = Target { buffer: &mut s.buffer, selection: &mut s.selection, indent: &indent, page_rows: 20, language: None };
    let mut space = |key: char, t: &mut Target| {
        s.modal.key(Key::Char(' '), &mut s.regs, t);
        s.modal.key(Key::Char(key), &mut s.regs, t)
    };
    assert_eq!(space('a', &mut t), Outcome::Effect(Effect::Assist));
    assert_eq!(space('t', &mut t), Outcome::Effect(Effect::Terminal));
    assert_eq!(space('j', &mut t), Outcome::Effect(Effect::NextProblem));
    assert_eq!(s.modal.key(Key::Char(':'), &mut s.regs, &mut t), Outcome::Effect(Effect::CommandLine));
}

#[test]
fn terminal_mode_leaves_the_keys_alone_and_goes_back_where_it_came_from() {
    let mut s = Session::new("abc\n");
    s.modal.enter(Mode::Ai);
    s.modal.enter(Mode::Terminal);
    s.keys("d d x");
    assert_eq!((s.text(), s.modal.mode), ("abc\n", Mode::Terminal), "the editor sends them to the shell");
    s.modal.leave(Mode::Terminal);
    assert_eq!(s.modal.mode, Mode::Ai, "back to the review");
}

#[test]
fn brackets_and_quotes_close_themselves() {
    let typed = |text: &str, keys: &str| {
        let mut s = Session::new(text);
        s.keys(keys);
        (s.text().to_string(), s.selection.head)
    };
    assert_eq!(typed("\n", "i f ( x"), ("f(x)\n".into(), 3), "the pair goes in, the cursor between");
    assert_eq!(typed("\n", "i ( ) x"), ("()x\n".into(), 3), "typing the closing one steps over it");
    assert_eq!(typed("ab\n", "i ("), ("(ab\n".into(), 1), "not before a word");
    assert_eq!(typed("\n", "i don ' t"), ("don't\n".into(), 5), "an apostrophe is just one");
    assert_eq!(typed("\n", "i x= \" hi \""), ("x=\"hi\"\n".into(), 6), "a quote pairs, and its close steps over");

    let mut s = Session::new("\n");
    s.keys("i [");
    let indent = IndentUnit::default();
    let mut t = Target { buffer: &mut s.buffer, selection: &mut s.selection, indent: &indent, page_rows: 20, language: None };
    s.modal.key(Key::Backspace, &mut s.regs, &mut t);
    assert_eq!(s.text(), "\n", "Backspace in an empty pair takes both");

    let mut s = Session::new("\n");
    s.keys("i fn ( ) { ret x");
    assert_eq!(s.text(), "fn(){\n    x\n}\n", "Enter between braces opens an indented line");
}

/// Lean's unicode input, typed a char at a time as the editor types.
#[test]
fn backslash_abbreviations_become_symbols() {
    let typed = |keys: &str| {
        let mut s = Session::new("");
        let indent = IndentUnit::default();
        s.keys("i");
        for c in keys.chars() {
            let mut t = Target {
                buffer: &mut s.buffer,
                selection: &mut s.selection,
                indent: &indent,
                page_rows: 20,
                language: crate::Language::for_path(std::path::Path::new("a.lean")),
            };
            match c {
                '⇥' => s.modal.key(Key::Tab, &mut s.regs, &mut t),
                '⌫' => s.modal.key(Key::Backspace, &mut s.regs, &mut t),
                '⎋' => s.modal.key(Key::Escape, &mut s.regs, &mut t),
                c => {
                    s.modal.insert(c.encode_utf8(&mut [0; 4]), &mut t);
                    Outcome::Handled
                }
            };
        }
        (s.buffer.text().to_string(), s.selection.head)
    };
    assert_eq!(typed("\\alpha").0, "α", "as soon as no longer one starts with it");
    assert_eq!(typed("\\a ").0, "α ", "finished by a space: the shortest it starts");
    assert_eq!(typed("\\N⇥").0, "ℕ", "Tab only finishes it");
    assert_eq!(typed("x \\to y").0, "x → y");
    assert_eq!(typed("\\\\").0, "\\", "two backslashes are one");
    assert_eq!(typed("\\alx").0, "∀x", "a char that can't go on finishes it");
    assert_eq!(typed("\\a\\b ").0, "αβ ", "a backslash finishes one and starts another");
    assert_eq!(typed("\\N⎋").0, "ℕ", "leaving insert finishes it");
    assert_eq!(typed("\\alp⌫⌫⌫ ").0, "\\ ", "backspacing past it leaves a backslash");
    assert_eq!(typed("\\<>x"), ("⟨x⟩".to_string(), "⟨x".len()), "the cursor goes where the symbol says");
    assert_eq!(typed("(\\zz").0, "(ζz)", "brackets close around it, and z alone is ζ");
    assert_eq!(typed("`Nat").0, "`Nat", "a backtick is a name's, in Lean");
}

#[test]
fn surround_delete_keeps_the_selection_on_the_text() {
    // The selection takes in the closing bracket, and a wide char follows.
    let mut s = Session::new("(a)é");
    s.selection = Range::new(1, 3);
    s.keys("m d (");
    assert_eq!(s.buffer.text(), "aé");
    assert_eq!(s.selection, Range::new(0, 1), "over the a, not inside é");
    s.keys("l d");
    assert_eq!(s.buffer.text(), "a");
    let mut s = Session::new("x[ab]y");
    s.selection = Range::new(2, 4);
    s.keys("m r [ (");
    assert_eq!((s.buffer.text(), s.selection), ("x(ab)y", Range::new(2, 4)));
}

#[test]
fn counts_reach_the_key_after_g_and_f() {
    let mut s = Session::new("1\n2\n3\n4\n5\n6\n");
    s.keys("5 g g");
    assert_eq!(s.buffer.line_of(s.selection.head), 4, "5gg is line 5");
    let mut s = Session::new("axbxcx");
    s.keys("2 f x");
    assert_eq!(s.selection.cursor(s.buffer.text()), 3, "2fx is the second x");
    let mut s = Session::new("ab");
    s.keys("y 9 9 9 9 9 9 9 9 9 9 9 9 9 9 p");
    assert!(s.buffer.len() < 1 << 20, "a huge count is capped");
    s.keys("9 9 9 9 9 9 9 9 9 9 l");
    assert!(s.selection.head <= s.buffer.len());
}

#[test]
fn on_a_bracket_its_own_pair_is_the_one() {
    let mut s = Session::new("f(a(b)c)");
    s.selection = Range::new(3, 4);
    s.keys("m i (");
    assert_eq!(&s.buffer.text()[s.selection.bytes()], "b", "the inner pair, from its opening bracket");
    let mut s = Session::new("f(a(b)c)");
    s.selection = Range::new(5, 6);
    s.keys("m d (");
    assert_eq!(s.buffer.text(), "f(abc)", "from its closing one");
}

#[test]
fn moves_are_by_characters_as_seen_and_columns_as_shown() {
    let mut s = Session::new("e\u{301}x");
    s.keys("d");
    assert_eq!(s.buffer.text(), "x", "the accent goes with its letter");
    let mut s = Session::new("\u{1f1f8}\u{1f1ea}x");
    s.keys("d");
    assert_eq!(s.buffer.text(), "x", "a flag is one character");
    let mut s = Session::new("\tabc\n        xyz\n");
    s.selection = Range::new(2, 3);
    s.keys("j");
    assert_eq!(s.selection.cursor(s.buffer.text()), "\tabc\n".len() + 5, "b is in column 5, on screen");
}

#[test]
fn enter_trims_the_line_left_and_indents_after_an_opener() {
    let mut s = Session::new("");
    s.keys("i if_x:   ret");
    assert_eq!(s.buffer.text(), "if_x:\n", "trimmed; no deeper outside Python");
    let mut s = Session::new("");
    s.keys("i call( ret");
    assert_eq!(s.buffer.text(), "call(\n    ", "in a level after an open bracket");
}

#[test]
fn indenting_lines_keeps_them_selected_whole() {
    let mut s = Session::new("a\nb\n");
    s.keys("x >");
    assert_eq!(s.buffer.text(), "    a\nb\n");
    assert_eq!(&s.buffer.text()[s.selection.bytes()], "    a\n");
    s.keys("x");
    assert_eq!(&s.buffer.text()[s.selection.bytes()], "    a\nb\n", "so x takes in the next line");
}

#[test]
fn a_word_around_at_the_end_takes_the_space_before() {
    let mut s = Session::new("foo bar");
    s.selection = Range::new(5, 6);
    s.keys("m a w d");
    assert_eq!(s.buffer.text(), "foo");
}

#[test]
fn join_increment_comment_and_registers() {
    let mut s = Session::new("a\n   b\nc\n");
    s.keys("J");
    assert_eq!(s.buffer.text(), "a b\nc\n", "J joins the next line, its indent gone");
    let mut s = Session::new("x = 41;\n");
    s.keys("ctrl-a");
    assert_eq!(s.buffer.text(), "x = 42;\n", "the number after the cursor");
    s.keys("3 ctrl-x");
    assert_eq!(s.buffer.text(), "x = 39;\n");
    let mut s = Session::new("n -1\n");
    s.keys("ctrl-a ctrl-a");
    assert_eq!(s.buffer.text(), "n 1\n", "negative numbers too");
    let mut s = Session::new("ab cd\n");
    s.keys("e \" a y \" a P");
    assert_eq!(s.buffer.text(), "abab cd\n", "a named register");
}
