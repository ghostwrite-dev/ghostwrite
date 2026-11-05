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
    language: Option<&'static crate::Language>,
}

impl Session {
    fn new(text: &str) -> Self {
        Self { modal: Modal::default(), buffer: Buffer::new(text), selection: Range::point(0), regs: Regs::default(), language: None }
    }

    /// The same, in the language of files named like `path`.
    fn in_file(text: &str, path: &str) -> Self {
        crate::language::testing::install();
        Self { language: crate::Language::for_path(std::path::Path::new(path)), ..Self::new(text) }
    }

    /// Space-separated keys; `esc`, `ret`, `tab`, `bs`, `del` are named, and
    /// `ctrl-x`, `alt-x`; `'x` is the key x even in insert mode; anything
    /// else longer than one char is typed text.
    fn keys(&mut self, keys: &str) -> &mut Self {
        let indent = IndentUnit::default();
        for word in keys.split(' ') {
            let key = match word {
                "esc" => Some(Key::Escape),
                "ret" => Some(Key::Enter),
                "tab" => Some(Key::Tab),
                "bs" => Some(Key::Backspace),
                "del" => Some(Key::Delete),
                "space" => Some(Key::Char(' ')),
                w if w.starts_with("ctrl-") => w.chars().last().map(Key::Ctrl),
                w if w.starts_with("alt-") => w.chars().last().map(Key::Alt),
                w if w.starts_with('\'') && w.len() > 1 => w.chars().last().map(Key::Char),
                w if w.chars().count() == 1 && self.modal.mode != Mode::Insert => Some(Key::Char(w.chars().next().unwrap())),
                _ => None,
            };
            let mut t = Target { buffer: &mut self.buffer, selection: &mut self.selection, indent: &indent, page_rows: 20, language: self.language };
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

    fn text(&self) -> String {
        self.buffer.to_string()
    }

    /// What's selected.
    fn selected(&self) -> String {
        self.buffer.slice(self.selection.bytes()).into_owned()
    }

    /// What the last key gave back, for a key that's an effect.
    fn outcome(&mut self, keys: &str) -> Outcome {
        let (last, first) = keys.rsplit_once(' ').map_or((keys, ""), |(first, last)| (last, first));
        if !first.is_empty() {
            self.keys(first);
        }
        let indent = IndentUnit::default();
        let mut t = Target { buffer: &mut self.buffer, selection: &mut self.selection, indent: &indent, page_rows: 20, language: self.language };
        let key = if let Some(c) = last.strip_prefix("ctrl-") { Key::Ctrl(c.chars().next().unwrap()) } else { Key::Char(last.chars().next().unwrap()) };
        self.modal.key(key, &mut self.regs, &mut t)
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
    assert_eq!(s.selection, Range::new(0, "one ".len()), "after the selection (the word and its space), which stays");
}

#[test]
fn i_and_a_keep_the_selection_as_helix_does() {
    let mut s = Session::new("one two\n");
    s.keys("e i X esc");
    assert_eq!(s.text(), "Xone two\n");
    assert_eq!((s.selected().as_str(), s.selection.head), ("one", 1), "still the word, the cursor at its start");
    let mut s = Session::new("one two\n");
    s.keys("e a XY esc");
    assert_eq!(s.text(), "oneXY two\n");
    assert_eq!(s.selected(), "oneXY", "after a, what was typed is selected too");
    assert_eq!(s.selection.cursor(&s.buffer), 4, "with the cursor on the last char typed");
    let mut s = Session::new("ab\n");
    s.keys("i X esc");
    assert_eq!((s.text().as_str(), s.selection), ("Xab\n", Range::point(1)), "a cursor alone stays one");
    let mut s = Session::new("one two\n");
    s.keys("e i bs");
    assert_eq!(s.text(), "one two\n", "Backspace in insert mode takes the char before, never the selection");
}

#[test]
fn whole_lines_changed_are_typed_on_a_line_of_their_own() {
    let mut s = Session::new("{\n    b\n}\n");
    s.keys("j x c X esc");
    assert_eq!(s.text(), "{\n    X\n}\n", "in their place, indented, not joined to the next");
    let mut s = Session::new("ab cd\n");
    s.keys("e c X esc");
    assert_eq!(s.text(), "X cd\n", "part of a line: changed where it was");
}

#[test]
fn alt_d_and_alt_c_dont_yank() {
    let mut s = Session::new("one two\n");
    s.keys("e y");
    s.selection = Range::new(4, 7);
    s.keys("alt-d");
    assert_eq!(s.text(), "one \n");
    s.selection = Range::new(0, 3);
    s.keys("alt-c X esc");
    assert_eq!(s.text(), "X \n");
    assert_eq!(s.regs.yanked, "one", "what was yanked before is still there");
}

#[test]
fn o_opens_lines_as_enter_would() {
    let mut s = Session::new("x\n");
    s.keys("3 o");
    assert_eq!(s.text(), "x\n\n\n\n");
    assert_eq!((s.selection, s.modal.take_added()), (Range::point(2), vec![Range::point(3), Range::point(4)]), "three lines, each with a cursor");
    let mut s = Session::in_file("fn f() {\n}\n", "a.rs");
    s.keys("o");
    assert_eq!((s.text().as_str(), s.selection.head), ("fn f() {\n    \n}\n", 13), "in a level after an open bracket");
    let mut s = Session::in_file("fn f() {\n}\n", "a.rs");
    s.keys("j O");
    assert_eq!(s.text(), "fn f() {\n    \n}\n", "O: as if Enter at the end of the line above");
    let mut s = Session::in_file("    // a\n", "a.rs");
    s.keys("o b esc");
    assert_eq!(s.text(), "    // a\n    // b\n", "a line comment goes on");
    let mut s = Session::in_file("/// a\n", "a.rs");
    s.keys("O");
    assert_eq!(s.text(), "/// \n/// a\n", "as it's written there");
}

#[test]
fn enter_goes_on_with_a_line_comment() {
    let mut s = Session::in_file("", "a.py");
    s.keys("i #a ret b");
    assert_eq!(s.text(), "#a\n# b");
    let mut s = Session::in_file("", "a.py");
    s.keys("i x#a ret b");
    assert_eq!(s.text(), "x#a\nb", "only a line that starts with one");
}

#[test]
fn insert_keys_as_in_helix() {
    let typed = |text: &str, keys: &str| {
        let mut s = Session::new(text);
        s.keys(keys);
        (s.text().to_string(), s.selection.head)
    };
    assert_eq!(typed("foo.bar baz\n", "g l a ctrl-w"), ("foo.bar \n".into(), 8), "ctrl-w: back to the word's start");
    assert_eq!(typed("foo.bar\n", "g l a ctrl-w ctrl-w"), ("foo\n".into(), 3), "punctuation is a word of its own");
    assert_eq!(typed("foo bar\n", "i alt-d"), (" bar\n".into(), 0), "alt-d: on to the word's end");
    assert_eq!(typed("    foo bar\n", "g l a ctrl-u"), ("    \n".into(), 4), "ctrl-u: back to the first non-blank");
    assert_eq!(typed("    foo\n", "g s i ctrl-u"), ("foo\n".into(), 0), "and from there, the line's start");
    assert_eq!(typed("a\nb\n", "j i ctrl-u"), ("ab\n".into(), 1), "at the start, onto the line before");
    assert_eq!(typed("foo bar\n", "l l i ctrl-k"), ("fo\n".into(), 2), "ctrl-k: on to the line's end");
    assert_eq!(typed("ab\ncd\n", "g l a ctrl-k"), ("abcd\n".into(), 2), "at the end, its line break");
    assert_eq!(typed("abc\n", "l i ctrl-h ctrl-d"), ("c\n".into(), 0), "ctrl-h, ctrl-d: a char back, on");
    assert_eq!(typed("ab\n", "l i ctrl-j"), ("a\nb\n".into(), 2), "ctrl-j: a new line");
    assert_eq!(typed("ab\n", "l i tab"), ("a   b\n".into(), 4), "tab: to the next indent stop");
    assert_eq!(typed("ab\n", "y i ctrl-r '\""), ("aab\n".into(), 1), "ctrl-r: a register, typed out");
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
fn in_files_the_trees_keys_are_a_layer_of_their_own() {
    let mut m = Modal { view: ViewKind::Files, ..Modal::default() };
    assert_eq!(m.view_key(Key::Char('3')), Outcome::Handled);
    assert_eq!(m.view_key(Key::Char('j')), Outcome::Tree(TreeCommand::Down(3)));
    assert_eq!(m.view_key(Key::Char('g')), Outcome::Handled);
    assert_eq!(m.view_key(Key::Char('g')), Outcome::Tree(TreeCommand::First));
    assert_eq!(m.view_key(Key::Char('x')), Outcome::Handled, "other letters are swallowed");
    assert_eq!(m.view_key(Key::Char(':')), Outcome::Effect(Effect::CommandLine));
    assert_eq!(m.view_key(Key::Escape), Outcome::Effect(Effect::FocusCode));
    assert_eq!(m.mode, Mode::Normal);
    assert_eq!(m.view_key(Key::Char(' ')), Outcome::Handled);
    assert_eq!(m.view_key(Key::Char('a')), Outcome::Effect(Effect::Assist), "space a asks, as everywhere");
    assert_eq!(m.view_key(Key::Char(' ')), Outcome::Handled);
    assert_eq!(m.view_key(Key::Char('e')), Outcome::Effect(Effect::Files), "space e goes back too");
    m.view_key(Key::Char(' '));
    assert_eq!(m.view_key(Key::Char('v')), Outcome::Effect(Effect::Pages), "space v: the pages");
    m.view_key(Key::Char(' '));
    assert_eq!(m.view_key(Key::Char('g')), Outcome::Effect(Effect::Aside), "space g: the side view");
    assert_eq!(m.view_key(Key::Ctrl('w')), Outcome::Ignored, "ctrl-w goes nowhere");
    assert_eq!(m.pending_keys(), "", "and waits for nothing after it");
    m.enter(Mode::Ai);
    assert_eq!(m.view_key(Key::Tab), Outcome::Review(Review::Next), "Tab accepts a story's step from here too");
    assert_eq!(m.view_key(Key::Enter), Outcome::Tree(TreeCommand::Open), "Enter is the tree's");
}

#[test]
fn on_the_terminal_the_keys_that_type_type_in_its_shell() {
    let mut s = Session::new("ab\n");
    s.modal.view = ViewKind::Terminal;
    s.keys("w");
    assert_eq!(s.selection, Range::new(0, 2), "the code's motions move through it");
    let indent = IndentUnit::default();
    let mut t = Target { buffer: &mut s.buffer, selection: &mut s.selection, indent: &indent, page_rows: 20, language: None };
    assert_eq!(s.modal.key(Key::Char('i'), &mut s.regs, &mut t), Outcome::Effect(Effect::TypeInTerminal));
    assert_eq!(s.modal.key(Key::Char('c'), &mut s.regs, &mut t), Outcome::Effect(Effect::TypeInTerminal));
    assert_eq!(s.modal.key(Key::Enter, &mut s.regs, &mut t), Outcome::Effect(Effect::GotoFile), "Enter opens the file a line names");
    assert_eq!(s.modal.key(Key::Char('v'), &mut s.regs, &mut t), Outcome::Handled);
    assert_eq!(s.modal.key(Key::Escape, &mut s.regs, &mut t), Outcome::Handled, "Esc ends select mode first");
    assert_eq!(s.modal.mode, Mode::Normal);
    assert_eq!(s.modal.key(Key::Escape, &mut s.regs, &mut t), Outcome::Effect(Effect::FocusCode), "then goes back to the code");
    assert_eq!(s.text(), "ab\n");
}

#[test]
fn the_mode_line_names_the_view_unless_its_the_code() {
    let mut m = Modal::default();
    assert_eq!(m.label(None), "NOR");
    m.view = ViewKind::Terminal;
    assert_eq!(m.label(None), "NOR · terminal");
    m.enter(Mode::Insert);
    assert_eq!(m.label(None), "INS · terminal");
    m.leave(Mode::Insert);
    m.view = ViewKind::Files;
    assert_eq!(m.label(None), "NOR · files");
    m.view = ViewKind::Aside;
    assert_eq!(m.label(Some("Goals")), "NOR · goals", "a plugin's view, by its title");
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
    let mut m = Modal { view: ViewKind::Terminal, ..Modal::default() };
    m.enter(Mode::Ai);
    m.enter(Mode::Insert);
    m.leave(Mode::Ai);
    assert_eq!(m.mode, Mode::Insert);
    m.leave(Mode::Insert);
    assert_eq!(m.mode, Mode::Normal);
}

#[test]
fn space_e_goes_to_files() {
    let mut s = Session::new("abc\n");
    let indent = IndentUnit::default();
    let mut t = Target { buffer: &mut s.buffer, selection: &mut s.selection, indent: &indent, page_rows: 20, language: None };
    s.modal.key(Key::Char(' '), &mut s.regs, &mut t);
    assert_eq!(s.modal.key(Key::Char('e'), &mut s.regs, &mut t), Outcome::Effect(Effect::Files));
    assert_eq!(s.modal.mode, Mode::Normal, "the mode goes with the keys");
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
    assert_eq!(space('v', &mut t), Outcome::Effect(Effect::Pages));
    assert_eq!(space('g', &mut t), Outcome::Effect(Effect::Aside));
    assert_eq!(space('j', &mut t), Outcome::Effect(Effect::NextProblem));
    assert_eq!(s.modal.key(Key::Char(':'), &mut s.regs, &mut t), Outcome::Effect(Effect::CommandLine));
}

#[test]
fn typing_on_the_terminal_leaves_the_keys_alone_and_goes_back_where_it_came_from() {
    let mut s = Session::new("abc\n");
    s.modal.enter(Mode::Ai);
    s.modal.view = ViewKind::Terminal;
    s.modal.enter(Mode::Insert);
    s.keys("d d x ret");
    assert_eq!((s.text().as_str(), s.modal.mode), ("abc\n", Mode::Insert), "the editor sends them to the shell");
    s.modal.leave(Mode::Insert);
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

static MATHS: crate::Language = crate::Language { name: "maths", pairs: &[('$', '$')], unpaired: &['\''], ..crate::Language::DEFAULT };

#[test]
fn a_language_has_pairs_of_its_own() {
    let mut s = Session::new("\n");
    let typed = |s: &mut Session, text: &str| {
        let indent = IndentUnit::default();
        let mut t = Target { buffer: &mut s.buffer, selection: &mut s.selection, indent: &indent, page_rows: 20, language: Some(&MATHS) };
        for c in text.chars() {
            s.modal.insert(c.encode_utf8(&mut [0; 4]), &mut t);
        }
    };
    s.keys("i");
    typed(&mut s, "$x");
    assert_eq!((s.text().as_str(), s.selection.head), ("$x$\n", 2), "`$` closes itself");
    typed(&mut s, "$ '");
    assert_eq!(s.text(), "$x$ '\n", "its close steps over, and `'` is just one here");
    let mut plain = Session::new("\n");
    plain.keys("i $");
    assert_eq!(plain.text(), "$\n", "elsewhere `$` is just one");
}

/// A language that types symbols with backslashes, with a real table of them.
static TABLE: crate::abbreviation::Abbreviations = crate::abbreviation::Abbreviations::new(include_str!("../testdata/abbreviations.tsv"));
static ABBREVIATING: crate::Language = crate::Language { name: "abbreviating", abbreviations: Some(&TABLE), unpaired: &['`'], ..crate::Language::DEFAULT };

/// Unicode input, typed a char at a time as the editor types.
#[test]
fn backslash_abbreviations_become_symbols() {
    let typed = |keys: &str| {
        let mut s = Session::new("");
        let indent = IndentUnit::default();
        s.keys("i");
        for c in keys.chars() {
            let mut t = Target { buffer: &mut s.buffer, selection: &mut s.selection, indent: &indent, page_rows: 20, language: Some(&ABBREVIATING) };
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
        (s.buffer.to_string().to_string(), s.selection.head)
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
    assert_eq!(s.buffer.to_string(), "aé");
    assert_eq!(s.selection, Range::new(0, 1), "over the a, not inside é");
    s.keys("l d");
    assert_eq!(s.buffer.to_string(), "a");
    let mut s = Session::new("x[ab]y");
    s.selection = Range::new(2, 4);
    s.keys("m r [ (");
    assert_eq!((s.buffer.to_string().as_str(), s.selection), ("x(ab)y", Range::new(2, 4)));
}

#[test]
fn counts_reach_the_key_after_g_and_f() {
    let mut s = Session::new("1\n2\n3\n4\n5\n6\n");
    s.keys("5 g g");
    assert_eq!(s.buffer.line_of(s.selection.head), 4, "5gg is line 5");
    let mut s = Session::new("axbxcx");
    s.keys("2 f x");
    assert_eq!(s.selection.cursor(&s.buffer), 3, "2fx is the second x");
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
    assert_eq!(&s.buffer.slice(s.selection.bytes()), "b", "the inner pair, from its opening bracket");
    let mut s = Session::new("f(a(b)c)");
    s.selection = Range::new(5, 6);
    s.keys("m d (");
    assert_eq!(s.buffer.to_string(), "f(abc)", "from its closing one");
}

#[test]
fn moves_are_by_characters_as_seen_and_columns_as_shown() {
    let mut s = Session::new("e\u{301}x");
    s.keys("d");
    assert_eq!(s.buffer.to_string(), "x", "the accent goes with its letter");
    let mut s = Session::new("\u{1f1f8}\u{1f1ea}x");
    s.keys("d");
    assert_eq!(s.buffer.to_string(), "x", "a flag is one character");
    let mut s = Session::new("\tabc\n        xyz\n");
    s.selection = Range::new(2, 3);
    s.keys("j");
    assert_eq!(s.selection.cursor(&s.buffer), "\tabc\n".len() + 5, "b is in column 5, on screen");
}

#[test]
fn enter_trims_the_line_left_and_indents_after_an_opener() {
    let mut s = Session::new("");
    s.keys("i if_x:   ret");
    assert_eq!(s.buffer.to_string(), "if_x:\n", "trimmed; no deeper outside Python");
    let mut s = Session::new("");
    s.keys("i call( ret");
    assert_eq!(s.buffer.to_string(), "call(\n    ", "in a level after an open bracket");
}

#[test]
fn indenting_lines_keeps_them_selected_whole() {
    let mut s = Session::new("a\nb\n");
    s.keys("x >");
    assert_eq!(s.buffer.to_string(), "    a\nb\n");
    assert_eq!(&s.buffer.slice(s.selection.bytes()), "    a\n");
    s.keys("x");
    assert_eq!(&s.buffer.slice(s.selection.bytes()), "    a\nb\n", "so x takes in the next line");
}

#[test]
fn a_word_around_at_the_end_takes_the_space_before() {
    let mut s = Session::new("foo bar");
    s.selection = Range::new(5, 6);
    s.keys("m a w d");
    assert_eq!(s.buffer.to_string(), "foo");
}

#[test]
fn join_increment_comment_and_registers() {
    let mut s = Session::new("a\n   b\nc\n");
    s.keys("J");
    assert_eq!(s.buffer.to_string(), "a b\nc\n", "J joins the next line, its indent gone");
    let mut s = Session::new("x = 41;\n");
    s.keys("ctrl-a");
    assert_eq!(s.buffer.to_string(), "x = 42;\n", "the number after the cursor");
    s.keys("3 ctrl-x");
    assert_eq!(s.buffer.to_string(), "x = 39;\n");
    let mut s = Session::new("n -1\n");
    s.keys("ctrl-a ctrl-a");
    assert_eq!(s.buffer.to_string(), "n 1\n", "negative numbers too");
    let mut s = Session::new("ab cd\n");
    s.keys("e \" a y \" a P");
    assert_eq!(s.buffer.to_string(), "abab cd\n", "a named register");
}

#[test]
fn the_mode_stays_as_the_keys_go_from_view_to_view() {
    let mut m = Modal::default();
    m.enter(Mode::Ai);
    m.view = ViewKind::Files;
    assert_eq!(m.mode, Mode::Ai, "a story's still up");
    m.view = ViewKind::Code;
    m.leave(Mode::Ai);
    assert_eq!(m.mode, Mode::Normal, "it ended meanwhile");
}

#[test]
fn a_views_layer_goes_over_the_codes_and_yours_over_both() {
    let (keymap, wrong) = Keymap::default()
        .with_yours_in(ViewKind::Files, &[("r".into(), "delete_file".into()), ("x".into(), "rename_file".into()), ("d".into(), "no_op".into())]);
    let keymap: &'static Keymap = Box::leak(Box::new(keymap));
    assert!(wrong.is_empty(), "{wrong:?}");
    let mut m = Modal { view: ViewKind::Files, ..Modal::default() };
    m.set_keymap(keymap);
    assert_eq!(m.view_key(Key::Char('x')), Outcome::Tree(TreeCommand::Rename));
    assert_eq!(m.view_key(Key::Char('r')), Outcome::Tree(TreeCommand::Delete));
    assert_eq!(m.view_key(Key::Char('d')), Outcome::Handled, "no_op hides it");
    assert_eq!(m.view_key(Key::Char('a')), Outcome::Tree(TreeCommand::Add), "the rest as they were");
    m.view = ViewKind::Code;
    assert_eq!(m.command_for(Key::Char('x')), Some(Command::ExtendLineBelow), "the code's keys are its own");
    m.view = ViewKind::Pages;
    assert_eq!(m.command_for(Key::Char('j')), Some(Command::ScrollDown), "pages scroll");
    assert_eq!(m.command_for(Key::Char('w')), Some(Command::MoveNextWordStart), "and have the code's keys under theirs");
}

#[test]
fn counts_go_where_helix_takes_them() {
    let mut s = Session::new("1\n2\n3\n");
    s.keys("j j G");
    assert_eq!(s.selection, Range::point(4), "G without a count goes nowhere");
    s.keys("9 g g");
    assert_eq!(s.selection, Range::point(4), "a count past the end is the last line, not the empty one after it");
    s.keys("g g x d x d u u");
    assert_eq!(s.text(), "1\n2\n3\n", "u takes a step at a time");
    s.keys("2 U");
    assert_eq!(s.text(), "3\n", "2U: two");
    let mut s = Session::new("abc\n");
    assert_eq!(s.outcome("3 n"), Outcome::Effect(Effect::FindNext { backward: false, count: 3 }));
    assert_eq!(s.outcome("N"), Outcome::Effect(Effect::FindNext { backward: true, count: 1 }));
    assert_eq!(s.outcome("*"), Outcome::Effect(Effect::FindSelection { word_bounds: true }));
    assert_eq!(s.outcome("2 ctrl-o"), Outcome::Effect(Effect::JumpBack(2)));
    assert_eq!(s.outcome("space w v"), Outcome::Effect(Effect::SplitRight));
    assert_eq!(s.outcome("space w w"), Outcome::Effect(Effect::OtherPane));
    assert_eq!(s.outcome("space w q"), Outcome::Effect(Effect::CloseSplit));
    assert_eq!(s.outcome("space w o"), Outcome::Effect(Effect::CloseSplit));
}

#[test]
fn select_mode_ends_after_r_and_case_changes() {
    let mut s = Session::new("ab cd\n");
    s.keys("v l r x");
    assert_eq!((s.text().as_str(), s.modal.mode), ("xx cd\n", Mode::Normal));
    s.keys("v l ~");
    assert_eq!((s.text().as_str(), s.modal.mode), ("XX cd\n", Mode::Normal));
    s.keys("` w alt-`");
    assert_eq!(s.text(), "xx CD\n", "` lower case, alt-` upper");
    s.keys("v g h R");
    assert_eq!((s.text().as_str(), s.modal.mode), ("xx CD\n", Mode::Select), "R with nothing yanked does nothing");
    s.keys("y l R");
    assert_eq!(s.modal.mode, Mode::Normal, "with something, it ends select mode");
}

#[test]
fn x_out_to_whole_lines_faces_as_it_did_and_alt_x_shrinks_in() {
    let mut s = Session::new("ab\ncd\nef\n");
    s.selection = Range::new(4, 1);
    s.keys("X");
    assert_eq!(s.selection, Range::new(6, 0), "still facing back");
    s.selection = Range::new(1, 7);
    s.keys("alt-x");
    assert_eq!(s.selected(), "cd\n", "only the whole line in it");
}

#[test]
fn join_takes_off_comment_leaders_and_alt_j_selects_the_spaces() {
    let mut s = Session::in_file("// a\n   // b\nc\n", "a.rs");
    s.keys("x x J");
    assert_eq!(s.text(), "// a b\nc\n");
    let mut s = Session::new("a  \nb\n\nc\n");
    s.keys("x x x alt-J");
    assert_eq!(s.text(), "a   b\nc\n", "the spaces at a line's end stay, as in helix; a blank line adds none");
    assert_eq!((s.selection, s.modal.take_added()), (Range::point(3), vec![]), "the space put in is selected");
}

#[test]
fn indenting_goes_to_the_next_stop_and_leaves_blank_lines() {
    let mut s = Session::new("  a\n\n b\n");
    s.keys("% >");
    assert_eq!(s.text(), "    a\n\n    b\n");
}

#[test]
fn insert_at_line_start_and_end_go_by_the_cursor_and_indent_an_empty_line() {
    let mut s = Session::new("    x {\n\n");
    s.keys("j A y esc");
    assert_eq!(s.text(), "    x {\n        y\n", "an empty line indented as Enter after the line above would");
    let mut s = Session::new("ab cd\n");
    s.keys("v A");
    assert_eq!((s.selection, s.modal.mode), (Range::new(0, 5), Mode::Insert), "in select mode the selection stretches there");
}

#[test]
fn finds_cross_lines_and_alt_dot_finds_again() {
    let mut s = Session::new("ab\ncd\tx\nab\n");
    s.keys("f d");
    assert_eq!(s.selected(), "ab\ncd");
    s.keys("f tab");
    assert_eq!(s.selection.cursor(&s.buffer), 5, "Tab finds a tab");
    s.keys("g g f ret");
    assert_eq!(s.selected(), "ab\n", "Enter: the line's end");
    s.keys("g g f b alt-.");
    assert_eq!(s.selection.cursor(&s.buffer), 9, "alt-. finds the next b");
}

#[test]
fn match_mode_counts_and_letters() {
    let mut s = Session::new("f(a(b)c)\n");
    s.selection = Range::point(4);
    s.keys("2 m d (");
    assert_eq!(s.text(), "fa(b)c\n", "2md( deletes the second pair out");
    let mut s = Session::new("q(a)q\n");
    s.selection = Range::point(2);
    s.keys("m i q");
    assert_eq!(s.selection, Range::point(2), "mi and a letter: no pair of q's");
}

#[test]
fn blank_lines_paragraphs_and_the_last_change() {
    let mut s = Session::new("a\nb\n");
    s.keys("2 ] space");
    assert_eq!((s.text().as_str(), s.selection), ("a\n\n\nb\n", Range::point(0)), "blank lines below, the cursor where it was");
    s.keys("[ space");
    assert_eq!((s.text().as_str(), s.selection), ("\na\n\n\nb\n", Range::point(1)), "and above");
    s.keys("g g ] p");
    assert_eq!(s.selected(), "a\n\n\n", "]p: on to the next paragraph");
    s.keys("g e g .");
    assert_eq!(s.selection, Range::point(1), "g.: where the last change ended");
}

#[test]
fn trimming_splitting_and_shrinking_selections() {
    let mut s = Session::new("  ab  \ncd\n");
    s.keys("x _");
    assert_eq!(s.selected(), "ab", "_ trims the ends");
    let mut s = Session::new("ab\ncd\nef");
    s.keys("%");
    s.keys("alt-s");
    let added = s.modal.take_added();
    assert_eq!((s.selected().as_str(), added), ("ab", vec![Range::new(3, 5), Range::new(6, 8)]), "alt-s: a selection a line");
}

#[test]
fn reviewing_refuses_the_new_edits_too() {
    let mut s = Session::new("one\ntwo\n");
    s.modal.enter(Mode::Ai);
    s.keys("alt-d alt-c alt-J ` alt-` ] space 3 o");
    assert_eq!(s.text(), "one\ntwo\n", "nothing changes");
    assert!(s.modal.take_added().is_empty() && s.modal.take_refused());
}

#[test]
fn the_syntax_tree_selects_by_its_nodes() {
    let code = "fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n\nfn two() -> i32 {\n    add(1, 1)\n}\n";
    let shown = |s: &Session| s.buffer.to_string()[s.selection.bytes()].to_string();
    let mut s = Session::in_file(code, "a.rs");
    s.selection = Range::point(code.find("b: i32").unwrap());
    s.keys("alt-o");
    assert_eq!(shown(&s), "b: i32", "alt-o: out to the node around it");
    s.keys("alt-p");
    assert_eq!(shown(&s), "a: i32", "alt-p: the one before");
    s.keys("alt-n");
    assert_eq!(shown(&s), "b: i32", "alt-n: and after");
    s.keys("alt-o alt-i");
    assert_eq!(shown(&s), "a: i32", "alt-i: back in");
    s.keys("alt-a");
    assert_eq!((shown(&s), s.modal.take_added().len()), ("a: i32".to_string(), 1), "alt-a: each beside it");

    // Text objects and going to them.
    s.selection = Range::point(code.find("add(1, 1)").unwrap());
    s.keys("m a f");
    assert_eq!(shown(&s), "fn two() -> i32 {\n    add(1, 1)\n}", "maf");
    s.selection = Range::point(code.find("1, 1").unwrap());
    s.keys("m i a");
    assert_eq!(shown(&s), "1", "mia: the argument");
    s.selection = Range::point(0);
    s.keys("] f");
    assert!(shown(&s).starts_with("fn two"), "]f");
    s.keys("[ f");
    assert!(shown(&s).starts_with("fn add"), "[f");
    s.selection = Range::point(0);
    s.keys("] f alt-.");
    assert!(shown(&s).starts_with("fn two"), "alt-. goes on from there (there's no third, so it stays)");
}

#[test]
fn without_a_grammar_syntax_keys_do_nothing() {
    let mut s = Session::new("plain text\n");
    s.selection = Range::point(2);
    s.keys("alt-o m a f ] f");
    assert_eq!(s.selection, Range::point(2));
}
