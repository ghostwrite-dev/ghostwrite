//! Helix-style modal editing: the stateful half. Keystrokes are intercepted
//! before the editor sees them; in normal and select mode every key without
//! cmd is consumed, so stray letters never reach the buffer.

use std::ops::Range as Bytes;

use gpui_kit::component::input::{self, EditorState};
use gpui_kit::{Action as _, App, ClipboardItem, Entity, Keystroke, Window};

use crate::helix::{self, Doc, Mode, Range, Word};

/// What the workspace should do after a key the modal layer handled.
pub enum Effect {
    None,
    NextBuffer,
    PrevBuffer,
    FocusTree,
    CommandLine,
    /// `space i`: ask Claude about the selection or current line, or to change it.
    Assist,
    /// `?`: show every key.
    Cheatsheet,
    /// `gd`, `space k`, `space d`: the language server's definition, docs, diagnostics.
    GotoDefinition,
    Hover,
    Diagnostics,
    /// `space /`: search the project.
    Search,
}

#[derive(Clone, Copy)]
enum Pending {
    Goto,
    Space,
    Find { forward: bool, till: bool },
    Replace,
    /// `m`, then: `m` `s` `r` `d` `a` `i`.
    Match,
    SurroundAdd,
    SurroundDelete,
    SurroundReplace,
    SurroundReplaceWith(char),
    Object { around: bool },
}

pub struct Modal {
    pub mode: Mode,
    /// One level of indent in the current buffer, for `>` and `<`.
    pub indent: String,
    pending: Option<Pending>,
    count: Option<usize>,
    sticky_col: Option<usize>,
    register: String,
    /// The selection we last showed, the byte range the editor shows for it,
    /// and the byte its block cursor sits on; lets a backward or empty
    /// selection survive the round trip.
    last: Option<(Range, Bytes<usize>, usize)>,
}

enum Key<'a> {
    Char(char),
    Named(&'a str),
    Ctrl(&'a str),
    Alt(&'a str),
}

fn key_of(ks: &Keystroke) -> Key<'_> {
    let m = &ks.modifiers;
    if m.control {
        return Key::Ctrl(&ks.key);
    }
    if m.alt {
        return Key::Alt(&ks.key);
    }
    // What was typed; else a one-character key name (some sources send no `key_char`).
    let typed = ks.key_char.as_deref().unwrap_or(if ks.key.chars().count() == 1 { &ks.key } else { "" });
    let mut chars = typed.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) if !c.is_control() => Key::Char(c),
        _ => Key::Named(&ks.key),
    }
}

impl Modal {
    pub fn new() -> Self {
        Self { mode: Mode::Normal, indent: "    ".into(), pending: None, count: None, sticky_col: None, register: String::new(), last: None }
    }

    /// Keys typed so far toward a command, for the status bar.
    pub fn pending_label(&self) -> String {
        let count = self.count.map(|c| c.to_string()).unwrap_or_default();
        let pending = match self.pending {
            Some(Pending::Goto) => "g",
            Some(Pending::Space) => "space",
            Some(Pending::Find { forward: true, till: false }) => "f",
            Some(Pending::Find { forward: true, till: true }) => "t",
            Some(Pending::Find { forward: false, till: false }) => "F",
            Some(Pending::Find { forward: false, till: true }) => "T",
            Some(Pending::Replace) => "r",
            Some(Pending::Match) => "m",
            Some(Pending::SurroundAdd) => "ms",
            Some(Pending::SurroundDelete) => "md",
            Some(Pending::SurroundReplace) => "mr",
            Some(Pending::SurroundReplaceWith(_)) => "mr…",
            Some(Pending::Object { around: true }) => "ma",
            Some(Pending::Object { around: false }) => "mi",
            None => "",
        };
        format!("{count}{pending}")
    }

    fn selection(&self, doc: &Doc, st: &EditorState) -> Range {
        let shown = st.selected_range();
        if let Some((sel, bytes, _)) = &self.last
            && *bytes == shown
        {
            return *sel;
        }
        let (from, to) = (doc.char_of_byte(shown.start), doc.char_of_byte(shown.end));
        if from == to {
            Range::point(from)
        } else if st.cursor() == shown.start {
            Range::new(to, from)
        } else {
            Range::new(from, to)
        }
    }

    fn show(&mut self, doc: &Doc, sel: Range, st: &mut EditorState, cx: &mut gpui_kit::Context<EditorState>) {
        let sel = Range::new(sel.anchor.min(doc.len()), sel.head.min(doc.len()));
        let shown = if self.mode == Mode::Insert { sel } else { sel.min_width_1(doc) };
        let bytes = doc.byte(shown.from())..doc.byte(shown.to());
        st.set_selected_range(bytes.clone(), cx);
        self.last = Some((sel, bytes, doc.byte(sel.cursor())));
    }

    /// What `space i` works on, in bytes: a selection wider than the cursor,
    /// else the cursor's line without its line break.
    pub fn assist_target(&self, st: &EditorState) -> Bytes<usize> {
        let doc = Doc::new(&st.text().to_string());
        let sel = self.selection(&doc, st);
        if sel.to() - sel.from() > 1 {
            return doc.byte(sel.from())..doc.byte(sel.to());
        }
        let cursor = sel.cursor();
        doc.byte(doc.line_start(cursor))..doc.byte(doc.line_end(cursor))
    }

    /// Select `bytes`, as helix does with pasted text, back in normal mode.
    pub fn select_bytes(&mut self, bytes: Bytes<usize>, st: &mut EditorState, cx: &mut gpui_kit::Context<EditorState>) {
        let doc = Doc::new(&st.text().to_string());
        self.mode = Mode::Normal;
        let r = Range::new(doc.char_of_byte(bytes.start), doc.char_of_byte(bytes.end));
        self.show(&doc, r, st, cx);
    }

    /// `:<line>`: put the cursor at the start of the one-based `line`.
    pub fn goto_line(&mut self, line: usize, st: &mut EditorState, cx: &mut gpui_kit::Context<EditorState>) {
        let doc = Doc::new(&st.text().to_string());
        let at = doc.nth_line_start(line.saturating_sub(1).min(doc.last_line()));
        self.show(&doc, Range::point(at), st, cx);
    }

    /// The byte the block cursor sits on, wherever the selection came from.
    pub fn block_at(&self, st: &EditorState) -> usize {
        let shown = st.selected_range();
        if let Some((_, bytes, at)) = &self.last
            && *bytes == shown
        {
            return *at;
        }
        if shown.is_empty() || st.cursor() == shown.start {
            return shown.start;
        }
        shown.end - st.text().chars_at(shown.end).reversed().next().map_or(0, char::len_utf8)
    }

    fn enter_insert(&mut self, at: usize, doc: &Doc, st: &mut EditorState, cx: &mut gpui_kit::Context<EditorState>) {
        self.mode = Mode::Insert;
        self.show(doc, Range::point(at), st, cx);
    }

    /// Handles `ks` if modal editing claims it. `None` lets it through.
    pub fn key(&mut self, ks: &Keystroke, state: &Entity<EditorState>, window: &mut Window, cx: &mut App) -> Option<Effect> {
        if ks.modifiers.platform {
            return None;
        }
        let key = key_of(ks);
        if self.mode == Mode::Insert {
            if matches!(key, Key::Named("escape")) {
                state.update(cx, |st, cx| {
                    let doc = Doc::new(&st.text().to_string());
                    self.mode = Mode::Normal;
                    let at = doc.char_of_byte(st.cursor());
                    self.show(&doc, Range::point(at), st, cx);
                });
                return Some(Effect::None);
            }
            return None;
        }
        if let Key::Ctrl(k) = key
            && !matches!(k, "d" | "u" | "b" | "f")
        {
            return None;
        }
        Some(state.update(cx, |st, cx| self.normal_key(key, st, window, cx)))
    }

    fn normal_key(&mut self, key: Key, st: &mut EditorState, window: &mut Window, cx: &mut gpui_kit::Context<EditorState>) -> Effect {
        let doc = Doc::new(&st.text().to_string());
        let sel = self.selection(&doc, st);
        let extend = self.mode == Mode::Select;

        if let Some(pending) = self.pending.take() {
            let count = self.count.take().unwrap_or(1);
            return self.pending_key(pending, key, count, &doc, sel, extend, st, window, cx);
        }
        if let Key::Char(c @ '0'..='9') = key
            && (c != '0' || self.count.is_some())
        {
            self.count = Some(self.count.unwrap_or(0) * 10 + c.to_digit(10).unwrap() as usize);
            return Effect::None;
        }
        let explicit_count = self.count.take();
        let count = explicit_count.unwrap_or(1);
        let mut sticky = None;

        let moved = match key {
            Key::Char('h') | Key::Named("left") => Some(helix::move_horizontally(&doc, sel, false, count, extend)),
            Key::Char('l') | Key::Named("right") => Some(helix::move_horizontally(&doc, sel, true, count, extend)),
            Key::Char('j' | 'k') | Key::Named("down" | "up") => {
                let down = matches!(key, Key::Char('j') | Key::Named("down"));
                let (r, col) = helix::move_vertically(&doc, sel, down, count, extend, self.sticky_col);
                sticky = Some(col);
                Some(r)
            }
            Key::Ctrl(k @ ("d" | "u" | "f" | "b")) | Key::Named(k @ ("pagedown" | "pageup")) => {
                let rows = st.visible_row_range().map_or(20, |r| r.len().max(2));
                let rows = if matches!(k, "d" | "u") { rows / 2 } else { rows };
                let down = matches!(k, "d" | "f" | "pagedown");
                let (r, col) = helix::move_vertically(&doc, sel, down, rows * count, extend, self.sticky_col);
                sticky = Some(col);
                Some(r)
            }
            Key::Char(c @ ('w' | 'b' | 'e' | 'W' | 'B' | 'E')) => {
                let target = match c {
                    'w' => Word::NextStart,
                    'b' => Word::PrevStart,
                    'e' => Word::NextEnd,
                    'W' => Word::NextLongStart,
                    'B' => Word::PrevLongStart,
                    _ => Word::NextLongEnd,
                };
                let r = helix::word_move(&doc, sel, count, target);
                Some(if extend { sel.put_cursor(&doc, r.cursor(), true) } else { r })
            }
            Key::Char('G') => {
                let line = explicit_count.map_or(doc.last_line(), |n| n.saturating_sub(1));
                Some(sel.put_cursor(&doc, doc.nth_line_start(line), extend))
            }
            Key::Named("home") => Some(sel.put_cursor(&doc, doc.line_start(sel.cursor()), extend)),
            Key::Named("end") => Some(sel.put_cursor(&doc, line_last_char(&doc, sel.cursor()), extend)),
            Key::Char('x') => Some(helix::extend_line_below(&doc, sel, count)),
            Key::Char('%') => Some(Range::new(0, doc.len())),
            Key::Char(';') => Some(Range::point(sel.cursor())),
            Key::Alt(";") => Some(sel.flip()),
            _ => None,
        };
        if let Some(r) = moved {
            self.sticky_col = sticky;
            self.show(&doc, r, st, cx);
            return Effect::None;
        }
        self.sticky_col = None;

        let target = sel.min_width_1(&doc);
        match key {
            Key::Char(':') => return Effect::CommandLine,
            Key::Char('?') => return Effect::Cheatsheet,
            Key::Char('g') => self.pending = Some(Pending::Goto),
            Key::Char(' ') => self.pending = Some(Pending::Space),
            Key::Char('r') => self.pending = Some(Pending::Replace),
            Key::Char('m') => self.pending = Some(Pending::Match),
            Key::Char(c @ ('f' | 't' | 'F' | 'T')) => {
                self.pending = Some(Pending::Find { forward: c.is_lowercase(), till: c.eq_ignore_ascii_case(&'t') });
            }
            Key::Char('v') => {
                self.mode = if self.mode == Mode::Select { Mode::Normal } else { Mode::Select };
            }
            Key::Named("escape") => {
                if self.mode == Mode::Select {
                    self.mode = Mode::Normal;
                }
            }
            Key::Char('i') => self.enter_insert(target.from(), &doc, st, cx),
            Key::Char('a') => self.enter_insert(target.to(), &doc, st, cx),
            Key::Char('I') => self.enter_insert(doc.first_non_whitespace(sel.from()), &doc, st, cx),
            Key::Char('A') => self.enter_insert(doc.line_end(target.to().saturating_sub(1).max(target.from())), &doc, st, cx),
            Key::Char('o') => {
                let at = doc.line_end(target.to().saturating_sub(1));
                let text = format!("\n{}", doc.indent(at));
                let caret = at + text.chars().count();
                self.edit(&doc, at..at, &text, Range::point(caret), Mode::Insert, st, window, cx);
            }
            Key::Char('O') => {
                let at = doc.line_start(target.from());
                let indent = doc.indent(at);
                let caret = at + indent.chars().count();
                self.edit(&doc, at..at, &format!("{indent}\n"), Range::point(caret), Mode::Insert, st, window, cx);
            }
            Key::Char(c @ ('d' | 'c')) => {
                self.register = doc.slice(target.from(), target.to());
                let mode = if c == 'c' { Mode::Insert } else { Mode::Normal };
                self.edit(&doc, target.from()..target.to(), "", Range::point(target.from()), mode, st, window, cx);
            }
            Key::Char('y') => {
                self.register = doc.slice(target.from(), target.to());
                self.mode = Mode::Normal;
            }
            Key::Char(c @ ('p' | 'P')) => {
                let text = self.register.clone();
                self.paste(&doc, target, &text, c == 'p', count, st, window, cx);
            }
            Key::Char('u') => {
                self.last = None;
                window.dispatch_action(input::Undo.boxed_clone(), cx);
            }
            Key::Char('U') => {
                self.last = None;
                window.dispatch_action(input::Redo.boxed_clone(), cx);
            }
            Key::Char(c @ ('>' | '<')) => {
                let start = doc.line_start(target.from());
                let end = doc.line_end(target.to().saturating_sub(1).max(target.from()));
                let block = doc.slice(start, end);
                let shifted: Vec<String> = block
                    .split('\n')
                    .map(|line| match c {
                        '>' if !line.is_empty() => format!("{}{line}", self.indent.repeat(count)),
                        '>' => line.to_string(),
                        _ => unindent(line, count, self.indent.len().max(1)),
                    })
                    .collect();
                let shifted = shifted.join("\n");
                let r = Range::new(start, start + shifted.chars().count());
                self.edit(&doc, start..end, &shifted, r, Mode::Normal, st, window, cx);
            }
            Key::Char('~') => {
                let swapped: String = doc
                    .slice(target.from(), target.to())
                    .chars()
                    .flat_map(|ch| -> Vec<char> {
                        if ch.is_uppercase() { ch.to_lowercase().collect() } else { ch.to_uppercase().collect() }
                    })
                    .collect();
                self.edit(&doc, target.from()..target.to(), &swapped, sel, self.mode, st, window, cx);
            }
            _ => {}
        }
        Effect::None
    }

    #[allow(clippy::too_many_arguments)]
    fn pending_key(
        &mut self,
        pending: Pending,
        key: Key,
        count: usize,
        doc: &Doc,
        sel: Range,
        extend: bool,
        st: &mut EditorState,
        window: &mut Window,
        cx: &mut gpui_kit::Context<EditorState>,
    ) -> Effect {
        let Key::Char(c) = key else { return Effect::None };
        let cursor = sel.cursor();
        let to = match (pending, c) {
            (Pending::Goto, 'g') => Some(doc.nth_line_start(count - 1)),
            (Pending::Goto, 'e') => Some(doc.nth_line_start(doc.last_line())),
            (Pending::Goto, 'h') => Some(doc.line_start(cursor)),
            (Pending::Goto, 'l') => Some(line_last_char(doc, cursor)),
            (Pending::Goto, 's') => Some(doc.first_non_whitespace(cursor)),
            (Pending::Goto, 'n') => return Effect::NextBuffer,
            (Pending::Goto, 'd') => return Effect::GotoDefinition,
            (Pending::Goto, 'p') => return Effect::PrevBuffer,
            (Pending::Find { forward, till }, ch) => {
                if let Some(r) = helix::find_char(doc, sel, ch, forward, till, count, extend) {
                    self.show(doc, r, st, cx);
                }
                return Effect::None;
            }
            (Pending::Match, 'm') => {
                if let Some(at) = helix::matching_bracket(doc, cursor) {
                    self.show(doc, sel.put_cursor(doc, at, extend), st, cx);
                }
                return Effect::None;
            }
            (Pending::Match, next @ ('s' | 'd' | 'r' | 'a' | 'i')) => {
                self.pending = Some(match next {
                    's' => Pending::SurroundAdd,
                    'd' => Pending::SurroundDelete,
                    'r' => Pending::SurroundReplace,
                    'a' => Pending::Object { around: true },
                    _ => Pending::Object { around: false },
                });
                return Effect::None;
            }
            (Pending::SurroundAdd, ch) => {
                let (open, close) = helix::pair_of(ch);
                let target = sel.min_width_1(doc);
                let text = format!("{open}{}{close}", doc.slice(target.from(), target.to()));
                let after = Range::new(target.from(), target.to() + 2);
                self.edit(doc, target.from()..target.to(), &text, after, Mode::Normal, st, window, cx);
                return Effect::None;
            }
            (Pending::SurroundReplace, from) => {
                self.pending = Some(Pending::SurroundReplaceWith(from));
                return Effect::None;
            }
            (pending @ (Pending::SurroundDelete | Pending::SurroundReplaceWith(_)), ch) => {
                let target = sel.min_width_1(doc);
                let which = match pending {
                    Pending::SurroundReplaceWith(from) => from,
                    _ => ch,
                };
                let found = if which == 'm' {
                    helix::nearest_pair(doc, target.from(), target.to())
                } else {
                    helix::surrounding(doc, target.from(), target.to(), helix::pair_of(which))
                };
                let Some((l, r)) = found else { return Effect::None };
                let inner = doc.slice(l + 1, r);
                let (text, after) = match pending {
                    Pending::SurroundReplaceWith(_) => {
                        let (open, close) = helix::pair_of(ch);
                        (format!("{open}{inner}{close}"), sel)
                    }
                    _ => (inner, Range::new(target.from().saturating_sub(1).max(l), target.to().saturating_sub(1).max(l))),
                };
                self.edit(doc, l..r + 1, &text, after, self.mode, st, window, cx);
                return Effect::None;
            }
            (Pending::Object { around }, ch) => {
                if let Some(r) = helix::text_object(doc, sel, ch, around) {
                    self.show(doc, r, st, cx);
                }
                return Effect::None;
            }
            (Pending::Replace, ch) => {
                let target = sel.min_width_1(doc);
                let text: String =
                    doc.slice(target.from(), target.to()).chars().map(|o| if o == '\n' { o } else { ch }).collect();
                self.edit(doc, target.from()..target.to(), &text, sel, self.mode, st, window, cx);
                return Effect::None;
            }
            (Pending::Space, 'e') => return Effect::FocusTree,
            (Pending::Space, 'i') => return Effect::Assist,
            (Pending::Space, 'k') => return Effect::Hover,
            (Pending::Space, 'd') => return Effect::Diagnostics,
            (Pending::Space, '/') => return Effect::Search,
            (Pending::Space, 'y') => {
                let target = sel.min_width_1(doc);
                cx.write_to_clipboard(ClipboardItem::new_string(doc.slice(target.from(), target.to())));
                return Effect::None;
            }
            (Pending::Space, p @ ('p' | 'P')) => {
                let text = cx.read_from_clipboard().and_then(|c| c.text()).unwrap_or_default();
                self.paste(doc, sel.min_width_1(doc), &text, p == 'p', 1, st, window, cx);
                return Effect::None;
            }
            _ => None,
        };
        if let Some(at) = to {
            self.show(doc, sel.put_cursor(doc, at, extend), st, cx);
        }
        Effect::None
    }

    /// Paste after (or before) the selection; text ending in a newline goes
    /// on its own line, as yanked with `x`. The pasted text ends up selected.
    #[allow(clippy::too_many_arguments)]
    fn paste(
        &mut self,
        doc: &Doc,
        target: Range,
        text: &str,
        after: bool,
        count: usize,
        st: &mut EditorState,
        window: &mut Window,
        cx: &mut gpui_kit::Context<EditorState>,
    ) {
        if text.is_empty() {
            return;
        }
        let mut text = text.repeat(count);
        let linewise = text.ends_with('\n');
        let at = match (linewise, after) {
            (true, true) => (doc.line_end(target.to().saturating_sub(1).max(target.from())) + 1).min(doc.len()),
            (true, false) => doc.line_start(target.from()),
            (false, true) => target.to(),
            (false, false) => target.from(),
        };
        if linewise && at == doc.len() && doc.len() > 0 && doc.line_start(at) != at {
            text = format!("\n{}", text.trim_end_matches('\n'));
        }
        let r = Range::new(at, at + text.chars().count());
        self.edit(doc, at..at, &text, r, Mode::Normal, st, window, cx);
    }

    /// Replace chars `range` with `text`, then show `after` in `mode`.
    #[allow(clippy::too_many_arguments)]
    fn edit(
        &mut self,
        doc: &Doc,
        range: std::ops::Range<usize>,
        text: &str,
        after: Range,
        mode: Mode,
        st: &mut EditorState,
        window: &mut Window,
        cx: &mut gpui_kit::Context<EditorState>,
    ) {
        st.set_selected_range(doc.byte(range.start)..doc.byte(range.end), cx);
        st.replace(text.to_string(), window, cx);
        self.mode = mode;
        let doc = Doc::new(&st.text().to_string());
        self.show(&doc, after, st, cx);
    }
}

/// The last char before the line break; the line start on an empty line.
fn line_last_char(doc: &Doc, ci: usize) -> usize {
    let (start, end) = (doc.line_start(ci), doc.line_end(ci));
    if end > start { end - 1 } else { start }
}

fn unindent(line: &str, count: usize, width: usize) -> String {
    let mut rest = line;
    for _ in 0..count {
        rest = match rest.strip_prefix('\t') {
            Some(r) => r,
            None => {
                let spaces = rest.len() - rest.trim_start_matches(' ').len();
                &rest[spaces.min(width)..]
            }
        };
    }
    rest.to_string()
}
