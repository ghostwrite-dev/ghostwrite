use super::*;

#[derive(Default)]
struct Regs {
    yanked: String,
    clipboard: Option<String>,
}

impl Registers for Regs {
    fn yanked(&self) -> String {
        self.yanked.clone()
    }
    fn yank(&mut self, text: String) {
        self.yanked = text;
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
                w if w.chars().count() == 1 && self.modal.mode != Mode::Insert => Some(Key::Char(w.chars().next().unwrap())),
                _ => None,
            };
            let mut t = Target { buffer: &mut self.buffer, selection: &mut self.selection, indent: &indent, page_rows: 20 };
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
    assert_eq!(s.keys("t ( c call esc").text(), "call(bar);\n");
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
    let mut t = Target { buffer: &mut s.buffer, selection: &mut s.selection, indent: &indent, page_rows: 20 };
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
    assert_eq!(m.tree_key(Key::Char('f')), Outcome::Tree(TreeCommand::Leave), "f goes back too");
    assert_eq!(m.mode, Mode::Normal);
}

#[test]
fn review_modes_take_review_keys_and_go_back_where_they_came_from() {
    let mut s = Session::new("one two\n");
    s.keys("v w");
    s.modal.enter(Mode::Ai);
    assert_eq!(s.modal.entered_from(), Some(Mode::Select));
    let indent = IndentUnit::default();
    let mut t = Target { buffer: &mut s.buffer, selection: &mut s.selection, indent: &indent, page_rows: 20 };
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
fn f_enters_tree_mode() {
    let mut s = Session::new("abc\n");
    let indent = IndentUnit::default();
    let mut t = Target { buffer: &mut s.buffer, selection: &mut s.selection, indent: &indent, page_rows: 20 };
    assert_eq!(s.modal.key(Key::Char('f'), &mut s.regs, &mut t), Outcome::Effect(Effect::FocusTree));
    assert_eq!(s.modal.mode, Mode::Tree);
}

#[test]
fn effects_come_back() {
    let mut s = Session::new("abc\n");
    let indent = IndentUnit::default();
    let mut t = Target { buffer: &mut s.buffer, selection: &mut s.selection, indent: &indent, page_rows: 20 };
    assert_eq!(s.modal.key(Key::Char('a'), &mut s.regs, &mut t), Outcome::Effect(Effect::Assist));
    assert_eq!(s.modal.key(Key::Char(':'), &mut s.regs, &mut t), Outcome::Effect(Effect::CommandLine));
}
