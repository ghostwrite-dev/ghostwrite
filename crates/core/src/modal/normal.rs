//! Normal and select mode: what each command does (the moves, selections
//! and edits), and keys looked up in the keymap to run them. AI mode uses
//! the same commands to look around, but every edit is refused, and those
//! that would start something else (most under `space`) wait for the
//! review: see [`Command::waits_for_review`].

use super::command::Command;
use super::keymap::{Bound, Found};
use super::{Effect, Key, Modal, Mode, Outcome, Pending, Place, Registers, Review, Target};
use crate::buffer::Buffer;
use crate::indent::{self, unindent};
use crate::language::Language;
use crate::movement::{self, Word, line_last_char};
use crate::nodes;
use crate::selection::Range;
use crate::textobject;

/// A motion `alt-.` does again, as helix keeps it.
#[derive(Debug, Clone, Copy)]
pub(super) enum Motion {
    /// `f` `t` `F` `T` and a char, or Enter (`None`): a line's end.
    Find { ch: Option<char>, forward: bool, till: bool, count: usize, extend: bool },
    /// `mi`, `ma`.
    Object { ch: char, around: bool, count: usize },
    /// `]p`, `[p`.
    Paragraph { forward: bool, count: usize },
    /// By the syntax tree (`alt-o`, `]f`, ...): see [`nodes`].
    Node(NodeMove),
}

/// A move by the syntax tree.
#[derive(Debug, Clone, Copy)]
pub(super) enum NodeMove {
    /// `alt-o`, `alt-i`.
    Expand,
    Shrink,
    /// `alt-n`, `alt-p`.
    Sibling {
        next: bool,
    },
    /// `alt-e`, `alt-b`.
    End {
        to_end: bool,
    },
    /// `]f`, `[a` and the like: to the `count`th object `name`.
    Object {
        name: &'static str,
        next: bool,
        count: usize,
    },
}

/// The syntax object a letter names after `mi`, `ma`, `]` or `[`, as helix has them.
pub(super) fn syntax_object(ch: char) -> Option<&'static str> {
    Some(match ch {
        'f' => "function",
        't' => "class",
        'a' => "parameter",
        'c' => "comment",
        'T' => "test",
        'e' => "entry",
        'x' => "xml-element",
        _ => return None,
    })
}

impl Motion {
    /// Where it takes `r` (`extend`: in select mode), in a file of `language`.
    fn from(self, b: &Buffer, r: Range, extend: bool, language: Option<&'static Language>) -> Option<Range> {
        let text = b.text();
        match self {
            Motion::Object { ch, around, .. } if let Some(name) = syntax_object(ch) => nodes::object(language?, text, r, name, around),
            Motion::Node(NodeMove::Expand) => nodes::expand(language?, text, r),
            Motion::Node(NodeMove::Shrink) => nodes::shrink(language?, text, r),
            Motion::Node(NodeMove::Sibling { next }) => nodes::sibling(language?, text, r, next),
            Motion::Node(NodeMove::End { to_end }) => nodes::node_end(language?, text, r, to_end, extend),
            Motion::Node(NodeMove::Object { name, next, count }) => {
                let to = nodes::goto_object(language?, text, r, name, next, count)?;
                // In select mode it stretches the selection there, as helix does.
                Some(if extend { Range::new(r.anchor, if to.head < r.anchor { to.anchor } else { to.head }) } else { to })
            }
            Motion::Find { ch: Some(ch), forward, till, count, extend } => movement::find_char(b, r, ch, forward, till, count, extend),
            Motion::Find { ch: None, forward, till, count, extend } => movement::find_line_end(b, r, forward, till, count, extend),
            Motion::Object { ch, around, count } => textobject::text_object(b, r, ch, around, count),
            Motion::Paragraph { forward: true, count } => Some(movement::next_paragraph(b, r, count, extend)),
            Motion::Paragraph { forward: false, count } => Some(movement::prev_paragraph(b, r, count, extend)),
        }
    }
}

impl Modal {
    /// A key in normal, select or AI mode: a digit of a count, a key on the
    /// way to a binding (`g`, `space`), the char a command waits for, or the
    /// last key of a binding, which runs its command.
    pub(super) fn normal_key(&mut self, key: Key, regs: &mut dyn Registers, t: &mut Target) -> Outcome {
        let keys = match self.pending.take() {
            Some(Pending::Char { command, keys, first }) => return self.char_key(command, keys, first, key, t),
            Some(Pending::Keys(keys)) => keys,
            _ => vec![],
        };
        if keys.is_empty() && self.count_digit(key) {
            return Outcome::Handled;
        }
        self.bound_key(keys, key, regs, t)
    }

    /// `key` after `keys`, looked up in the keymap.
    fn bound_key(&mut self, mut keys: Vec<Key>, key: Key, regs: &mut dyn Registers, t: &mut Target) -> Outcome {
        let at_rest = keys.is_empty();
        keys.push(key);
        let keymap = self.keymap;
        match keymap.find(&keys) {
            // A count before them is for the key after (`5gg`, `2fx`).
            Found::Keys => {
                self.sticky_col = None;
                self.pending = Some(Pending::Keys(keys));
                Outcome::Handled
            }
            Found::Bound(Bound::Command(command)) => self.command(*command, keys, regs, t),
            // Yours wait while a story's reviewed, as the keys under `space` do.
            Found::Bound(Bound::Typed(line)) if !self.mode.reviewing() => {
                self.count = None;
                Outcome::Effect(Effect::Run(line))
            }
            _ => {
                self.count = None;
                if !at_rest {
                    return Outcome::Handled;
                }
                self.sticky_col = None;
                // Other ctrl keys aren't modal editing's: the app may have them.
                if matches!(key, Key::Ctrl(_)) { Outcome::Ignored } else { Outcome::Handled }
            }
        }
    }

    /// Run `command`, bound to `keys`, or wait for the char it takes.
    fn command(&mut self, command: Command, keys: Vec<Key>, regs: &mut dyn Registers, t: &mut Target) -> Outcome {
        if self.mode.reviewing() && command.waits_for_review() {
            (self.count, self.sticky_col) = (None, None);
            return Outcome::Handled;
        }
        if command.takes_char() {
            self.sticky_col = None;
            self.pending = Some(Pending::Char { command, keys, first: None });
            return Outcome::Handled;
        }
        if !command.keeps_column() {
            self.sticky_col = None;
        }
        let count = self.count.take();
        self.run(command, count, regs, t)
    }

    /// A command, with the count typed before it, if any: first the moves.
    fn run(&mut self, command: Command, explicit_count: Option<usize>, regs: &mut dyn Registers, t: &mut Target) -> Outcome {
        use Command::*;
        let count = explicit_count.unwrap_or(1);
        let sel = *t.selection;
        let extend = self.mode == Mode::Select;
        let b = &*t.buffer;
        let text = b.text();
        let cursor = sel.cursor(text);
        let put = |at: usize| sel.put_cursor(text, at, extend);
        let next = matches!(
            command,
            GotoNextParagraph | GotoNextFunction | GotoNextClass | GotoNextParameter | GotoNextComment | GotoNextTest | GotoNextEntry | GotoNextXmlElement
        );
        let to = match command {
            MoveCharLeft | MoveCharRight => Some(movement::move_horizontally(b, sel, command == MoveCharRight, count, extend)),
            MoveLineDown | MoveLineUp | PageCursorHalfDown | PageCursorHalfUp | PageCursorDown | PageCursorUp => {
                let rows = match command {
                    MoveLineDown | MoveLineUp => count,
                    PageCursorHalfDown | PageCursorHalfUp => (t.page_rows.max(2) / 2).saturating_mul(count),
                    _ => t.page_rows.max(2).saturating_mul(count),
                };
                let down = matches!(command, MoveLineDown | PageCursorHalfDown | PageCursorDown);
                let (r, col) = movement::move_vertically(b, sel, down, rows, extend, self.sticky_col, t.indent.width);
                self.sticky_col = Some(col);
                Some(r)
            }
            MoveNextWordStart | MovePrevWordStart | MoveNextWordEnd | MoveNextLongWordStart | MovePrevLongWordStart | MoveNextLongWordEnd => {
                let target = match command {
                    MoveNextWordStart => Word::NextStart,
                    MovePrevWordStart => Word::PrevStart,
                    MoveNextWordEnd => Word::NextEnd,
                    MoveNextLongWordStart => Word::NextLongStart,
                    MovePrevLongWordStart => Word::PrevLongStart,
                    _ => Word::NextLongEnd,
                };
                let r = movement::word_move(b, sel, count, target);
                Some(if extend { sel.put_cursor(text, r.cursor(text), true) } else { r })
            }
            // `G` goes to the line counted (`10G`); without a count, nowhere, as in helix.
            GotoLine => explicit_count.map(|n| put(b.line_start((n - 1).min(b.last_line())))),
            GotoFileStart => Some(put(b.line_start((count - 1).min(b.last_line())))),
            GotoLastLine => Some(put(b.line_start(b.last_line()))),
            GotoLineStart => Some(put(b.line_start_at(cursor))),
            GotoLineEnd => Some(put(line_last_char(b, cursor))),
            GotoFirstNonwhitespace => Some(put(b.first_non_blank_at(cursor))),
            GotoLastModification => b.last_edit().map(put),
            MatchBrackets => textobject::matching_bracket(b, cursor).map(put),
            ExtendLineBelow => Some(movement::extend_line_below(b, sel, count)),
            // `X`: out to whole lines, not on to the next, facing as it did.
            ExtendToLineBounds => {
                let lines = b.lines_range(b.line_of(sel.from())..b.line_of(b.prev(sel.to()).max(sel.from())) + 1);
                Some(facing(sel, lines.start, lines.end))
            }
            ShrinkToLineBounds => Some(shrink_to_lines(b, sel)),
            SelectAll => Some(Range::new(0, b.len())),
            CollapseSelection => Some(Range::point(cursor)),
            FlipSelections => Some(sel.flip()),
            TrimSelections => Some(trimmed(text, sel)),
            SplitSelectionOnNewline => {
                let mut lines = split_lines(text, sel).into_iter();
                let first = lines.next();
                self.added = lines.collect();
                first
            }
            RepeatLastMotion => self.last_motion.map(|motion| (0..count).fold(sel, |r, _| motion.from(b, r, extend, t.language).unwrap_or(r))),
            ExpandSelection | ShrinkSelection | SelectNextSibling | SelectPrevSibling | MoveParentNodeEnd | MoveParentNodeStart => {
                let node = match command {
                    ExpandSelection => NodeMove::Expand,
                    ShrinkSelection => NodeMove::Shrink,
                    SelectNextSibling | SelectPrevSibling => NodeMove::Sibling { next: command == SelectNextSibling },
                    _ => NodeMove::End { to_end: command == MoveParentNodeEnd },
                };
                self.motion(Motion::Node(node), t);
                return Outcome::Handled;
            }
            // `alt-a`: each node beside the selection's; `alt-I`: each inside it.
            SelectAllSiblings | SelectAllChildren => {
                let spread = t.language.and_then(|language| nodes::spread(language, text, sel, command == SelectAllSiblings));
                let mut each = spread.unwrap_or_default().into_iter();
                let first = each.next();
                self.added = each.collect();
                first
            }
            GotoNextParagraph | GotoPrevParagraph => {
                self.motion(Motion::Paragraph { forward: next, count }, t);
                return Outcome::Handled;
            }
            GotoNextFunction | GotoPrevFunction | GotoNextClass | GotoPrevClass | GotoNextParameter | GotoPrevParameter | GotoNextComment | GotoPrevComment
            | GotoNextTest | GotoPrevTest | GotoNextEntry | GotoPrevEntry | GotoNextXmlElement | GotoPrevXmlElement => {
                let name = match command {
                    GotoNextFunction | GotoPrevFunction => "function",
                    GotoNextClass | GotoPrevClass => "class",
                    GotoNextParameter | GotoPrevParameter => "parameter",
                    GotoNextComment | GotoPrevComment => "comment",
                    GotoNextTest | GotoPrevTest => "test",
                    GotoNextEntry | GotoPrevEntry => "entry",
                    _ => "xml-element",
                };
                self.motion(Motion::Node(NodeMove::Object { name, next, count }), t);
                return Outcome::Handled;
            }
            _ => return self.act(command, count, regs, t),
        };
        if let Some(r) = to {
            *t.selection = r;
        }
        Outcome::Handled
    }

    /// A command that reaches beyond the buffer: an effect for the editor.
    fn act(&mut self, command: Command, count: usize, regs: &mut dyn Registers, t: &mut Target) -> Outcome {
        use Command::*;
        let effect = match command {
            CommandMode => Effect::CommandLine,
            Search | Rsearch => Effect::Find { backward: command == Rsearch },
            SearchNext | SearchPrev => Effect::FindNext { backward: command == SearchPrev, count },
            SearchSelectionDetectWordBounds => Effect::FindSelection { word_bounds: true },
            SearchSelection => Effect::FindSelection { word_bounds: false },
            JumpBackward => Effect::JumpBack(count),
            JumpForward => Effect::JumpForward(count),
            RepeatLastChange => Effect::Repeat,
            RecordMacro => Effect::RecordMacro,
            ReplayMacro => Effect::PlayMacro,
            GotoNextBuffer => Effect::NextBuffer(count),
            GotoPreviousBuffer => Effect::PrevBuffer(count),
            MoveBufferNext => Effect::MoveBufferNext(count),
            MoveBufferPrevious => Effect::MoveBufferPrev(count),
            GotoDefinition => Effect::GotoDefinition,
            GotoFile => Effect::GotoFile,
            GotoReference => Effect::References,
            GotoWindowTop => Effect::ScreenLine(Place::Top),
            GotoWindowCenter => Effect::ScreenLine(Place::Middle),
            GotoWindowBottom => Effect::ScreenLine(Place::Bottom),
            AlignViewCenter => Effect::View(Place::Middle),
            AlignViewTop => Effect::View(Place::Top),
            AlignViewBottom => Effect::View(Place::Bottom),
            GotoNextDiag | GotoPrevDiag => Effect::Diagnostic { forward: command == GotoNextDiag },
            Vsplit => Effect::SplitRight,
            RotateView => Effect::OtherPane,
            Wonly => Effect::CloseSplit,
            // While reviewing, `space a` asks about what's under review.
            Ask if self.mode.reviewing() => return Outcome::Review(Review::Ask),
            Ask => Effect::Assist,
            Terminal => Effect::Terminal,
            GotoNextProblem => Effect::NextProblem,
            FileExplorer => {
                self.to_tree();
                Effect::FocusTree
            }
            Hover => Effect::Hover,
            FileDiagnostics => Effect::Diagnostics,
            GlobalSearch => Effect::Search,
            Cheatsheet => Effect::Cheatsheet,
            FilePicker => Effect::PickFile,
            BufferPicker => Effect::PickBuffer,
            RenameSymbol => Effect::Rename,
            CodeAction => Effect::CodeActions,
            SymbolPicker => Effect::Symbols,
            CopySelectionOnNextLine | CopySelectionOnPrevLine => Effect::CopySelections { below: command == CopySelectionOnNextLine, count },
            KeepPrimarySelection => Effect::KeepPrimarySelection,
            RemovePrimarySelection => Effect::RemovePrimarySelection,
            RotateSelectionsBackward | RotateSelectionsForward => Effect::RotateSelections { forward: command == RotateSelectionsForward, count },
            MergeSelections | MergeConsecutiveSelections => Effect::MergeSelections { touching: command == MergeConsecutiveSelections },
            AlignSelections => Effect::AlignSelections,
            SelectRegex => Effect::SelectRegex,
            _ => {
                self.change(command, count, regs, t);
                return Outcome::Handled;
            }
        };
        Outcome::Effect(effect)
    }

    /// A command that changes the mode or the text.
    fn change(&mut self, command: Command, count: usize, regs: &mut dyn Registers, t: &mut Target) {
        use Command::*;
        let sel = *t.selection;
        let extend = self.mode == Mode::Select;
        let b = &*t.buffer;
        let text = b.text();
        let target = sel.min_width_1(text);
        match command {
            SelectMode if !self.mode.reviewing() => self.mode = if self.mode == Mode::Select { Mode::Normal } else { Mode::Select },
            NormalMode => self.mode = Mode::Normal,
            // `i` types before the selection, `a` after it; either way it
            // stays selected, as in helix (a single char is just the cursor).
            InsertMode if sel.is_wide(text) => self.enter_insert(Range::new(target.to(), target.from()), t),
            InsertMode => self.enter_insert(Range::point(target.from()), t),
            AppendMode => self.enter_insert(Range::new(target.from(), target.to()), t),
            InsertAtLineStart | InsertAtLineEnd => self.insert_on_line(t, sel, command == InsertAtLineEnd, extend),
            OpenBelow | OpenAbove => self.open(t, target, command == OpenBelow, count, true),
            // Under review nothing's deleted, so nothing's yanked either.
            DeleteSelection | ChangeSelection | DeleteSelectionNoyank | ChangeSelectionNoyank if self.mode.reviewing() => self.refused = true,
            // `alt-d`, `alt-c`: the same, but nothing's yanked.
            DeleteSelection | ChangeSelection | DeleteSelectionNoyank | ChangeSelectionNoyank => {
                let register = self.register.take();
                if matches!(command, DeleteSelection | ChangeSelection) {
                    regs.yank(register, b.slice(target.bytes()).to_string());
                }
                let change = matches!(command, ChangeSelection | ChangeSelectionNoyank);
                let whole_lines = change && whole_lines(b, target);
                let mode = if change && !whole_lines { Mode::Insert } else { Mode::Normal };
                self.edit(t, target.bytes(), "", Range::point(target.from()), mode);
                // Lines changed: typing goes on a line of its own in their place.
                if whole_lines {
                    let at = Range::point(target.from());
                    self.open(t, at.min_width_1(t.buffer.text()), false, 1, false);
                }
            }
            Yank => {
                regs.yank(self.register.take(), b.slice(target.bytes()).to_string());
                if !self.mode.reviewing() {
                    self.mode = Mode::Normal;
                }
            }
            PasteAfter | PasteBefore => {
                let text = regs.yanked(self.register.take());
                self.paste(t, target, &text, command == PasteAfter, count);
            }
            // `R`: the selection replaced with what was yanked (if anything was).
            ReplaceWithYanked => {
                let text = regs.yanked(self.register.take()).repeat(count.min(1000));
                if !text.is_empty() {
                    let after = Range::new(target.from(), target.from() + text.len());
                    self.edit(t, target.bytes(), &text, after, Mode::Normal);
                }
            }
            // `J`: the selection's lines (or this and the next) joined; `alt-J` selects the spaces put between.
            JoinSelections | JoinSelectionsSpace => self.join(t, sel, command == JoinSelectionsSpace),
            // `ctrl-a`, `ctrl-x`: the number at or after the cursor on its line, up or down by the count.
            Increment | Decrement => {
                let cursor = sel.cursor(text);
                let (start, end) = (b.line_start_at(cursor), b.line_end_at(cursor));
                if let Some((at, len, n)) = number_at(&text[start..end], cursor - start) {
                    let by = count as i128 * if command == Increment { 1 } else { -1 };
                    let new = (n + by).to_string();
                    let at = start + at;
                    self.edit(t, at..at + len, &new, Range::new(at, at + new.len()), self.mode);
                }
            }
            // `ctrl-c`: the selection's lines commented out, or back in.
            ToggleComments => {
                if let Some(token) = t.language.and_then(|l| l.comment) {
                    let first = b.line_of(sel.from());
                    let last = b.line_of(b.prev(sel.to()).max(sel.from()));
                    let (start, end) = (b.line_start(first), b.line_end(last));
                    let toggled = toggle_comment(b.slice(start..end), token);
                    let after = Range::new(start, start + toggled.len());
                    self.edit(t, start..end, &toggled, after, self.mode);
                }
            }
            Undo | Redo if !self.mode.reviewing() => {
                for _ in 0..count {
                    let Some(r) = (if command == Undo { t.buffer.undo() } else { t.buffer.redo() }) else { break };
                    *t.selection = r.clamp(t.buffer.len());
                }
            }
            Indent | Unindent => {
                let start = b.line_start_at(target.from());
                let end = b.line_end_at(b.prev(target.to()).max(target.from()));
                let (unit, width) = (t.indent.as_str().repeat(count.min(100)), t.indent.width.max(1));
                let shifted: Vec<String> = b
                    .slice(start..end)
                    .split('\n')
                    .map(|line| match command {
                        // Blank lines stay so; others go in to the next indent stop, as in helix.
                        Indent if line.trim().is_empty() => line.to_string(),
                        Indent if t.indent.hard_tabs => format!("{unit}{line}"),
                        Indent => {
                            let indent = line.len() - line.trim_start_matches([' ', '\t']).len();
                            format!("{}{line}", &unit[indent % width..])
                        }
                        _ => unindent(line, count, width).to_string(),
                    })
                    .collect();
                let shifted = shifted.join("\n");
                // Whole lines (`x`) stay whole: their line break stays selected.
                let line_break = target.to().saturating_sub(end).min(1);
                let after = Range::new(start, start + shifted.len() + line_break);
                self.edit(t, start..end, &shifted, after, Mode::Normal);
            }
            // `~` switches case; `` ` `` is lower case, `` alt-` `` upper.
            SwitchCase => self.change_case(t, sel, |ch| if ch.is_uppercase() { ch.to_lowercase().collect() } else { ch.to_uppercase().collect() }),
            SwitchToLowercase => self.change_case(t, sel, |ch| ch.to_lowercase().collect()),
            SwitchToUppercase => self.change_case(t, sel, |ch| ch.to_uppercase().collect()),
            // `] space`, `[ space`: blank lines below the selection, or above.
            AddNewlineBelow | AddNewlineAbove => {
                let below = command == AddNewlineBelow;
                let (first, last) = (b.line_of(sel.from()), b.line_of(b.prev(sel.to()).max(sel.from())));
                let at = movement::start_of(b, if below { last + 1 } else { first });
                let moved = |p: usize| if p > at || (p == at && !below) { p + count } else { p };
                let after = Range::new(moved(sel.anchor), moved(sel.head));
                self.edit(t, at..at, &"\n".repeat(count), after, self.mode);
            }
            YankToClipboard => regs.set_clipboard(b.slice(target.bytes()).to_string()),
            PasteClipboardAfter | PasteClipboardBefore => {
                let pasted = regs.clipboard().unwrap_or_default();
                self.paste(t, target, &pasted, command == PasteClipboardAfter, 1);
            }
            _ => {}
        }
    }

    /// The char a command waited for (`f`, `r`, `mi`). Enter and Tab are a
    /// line break and a tab for some (see [`Command::takes_enter`]); any
    /// other key that isn't a char gives up on it.
    fn char_key(&mut self, command: Command, keys: Vec<Key>, first: Option<char>, key: Key, t: &mut Target) -> Outcome {
        use Command::*;
        let explicit_count = self.count.take();
        let count = explicit_count.unwrap_or(1);
        let ch = match key {
            Key::Char(c) => c,
            Key::Enter if command.takes_enter() => '\n',
            Key::Tab if command.takes_enter() => '\t',
            _ => return Outcome::Handled,
        };
        let sel = *t.selection;
        let extend = self.mode == Mode::Select;
        let b = &*t.buffer;
        let text = b.text();
        let target = sel.min_width_1(text);
        match command {
            FindNextChar | FindTillChar | FindPrevChar | TillPrevChar => {
                let forward = matches!(command, FindNextChar | FindTillChar);
                let till = matches!(command, FindTillChar | TillPrevChar);
                // Enter: the line's end.
                let ch = Some(ch).filter(|&ch| ch != '\n');
                self.motion(Motion::Find { ch, forward, till, count, extend }, t);
            }
            Replace => {
                let replaced: String = b.slice(target.bytes()).chars().map(|o| if o == '\n' { o } else { ch }).collect();
                let after = same_shape(sel, target.from(), replaced.len());
                self.edit(t, target.bytes(), &replaced, after, Mode::Normal);
            }
            SelectRegister => self.register = Some(ch),
            SurroundAdd => {
                let (open, close) = textobject::pair_of(ch);
                let wrapped = format!("{open}{}{close}", b.slice(target.bytes()));
                let after = Range::new(target.from(), target.from() + wrapped.len());
                self.edit(t, target.bytes(), &wrapped, after, Mode::Normal);
            }
            // `mr` waits for the pair there is, then the one it becomes.
            SurroundReplace if first.is_none() => {
                self.pending = Some(Pending::Char { command, keys, first: Some(ch) });
                self.count = explicit_count;
            }
            // The count is for which pair (`2md(`: the second out).
            SurroundDelete | SurroundReplace => {
                let which = first.unwrap_or(ch);
                let Some((left, right)) = textobject::pair_around(b, target.from(), target.to(), which, count) else { return Outcome::Handled };
                let inner = b.slice(b.next(left)..right).to_string();
                let (open_len, close_len) = (b.next(left) - left, b.next(right) - right);
                let (new_open, new_close) = match command {
                    SurroundReplace => {
                        let (open, close) = textobject::pair_of(ch);
                        (open.to_string(), close.to_string())
                    }
                    _ => (String::new(), String::new()),
                };
                // The selection moves with the text: inside the pair by the
                // change in the opening, after it by both.
                let map = |at: usize| {
                    if at <= left {
                        at
                    } else if at <= right {
                        at - open_len + new_open.len()
                    } else {
                        at.max(right + close_len) - open_len - close_len + new_open.len() + new_close.len()
                    }
                };
                let after = Range::new(map(sel.anchor), map(sel.head));
                self.edit(t, left..b.next(right), &format!("{new_open}{inner}{new_close}"), after, Mode::Normal);
            }
            SelectTextobjectAround | SelectTextobjectInner => self.motion(Motion::Object { ch, around: command == SelectTextobjectAround, count }, t),
            _ => {}
        }
        Outcome::Handled
    }

    /// Carry out a motion, and keep it for `alt-.`.
    fn motion(&mut self, motion: Motion, t: &mut Target) {
        self.last_motion = Some(motion);
        if let Some(r) = motion.from(t.buffer, *t.selection, self.mode == Mode::Select, t.language) {
            *t.selection = r;
        }
    }

    /// Into insert mode, with `selection`: its head is where typing goes.
    fn enter_insert(&mut self, selection: Range, t: &mut Target) {
        if self.mode.reviewing() {
            self.refused = true;
            return;
        }
        self.mode = Mode::Insert;
        *t.selection = selection;
    }

    /// `I`, `A` (`end`): typing at the cursor's line's first non-blank, or
    /// its end; in select mode the selection stretches there. An empty line
    /// is indented first, as a line opened after the one above would be.
    fn insert_on_line(&mut self, t: &mut Target, sel: Range, end: bool, extend: bool) {
        let b = &*t.buffer;
        let text = b.text();
        let line = b.line_of(sel.cursor(text));
        let start = b.line_start(line);
        if b.line(line).is_empty() {
            let opens = t.language.unwrap_or(&Language::DEFAULT).indent_after;
            let indent = line.checked_sub(1).map_or(String::new(), |above| indent::for_newline(b.line(above), opens, t.indent));
            let after = Range::point(start + indent.len());
            return self.edit(t, start..start, &indent, after, Mode::Insert);
        }
        let at = if end { b.line_end(line) } else { b.first_non_blank_at(start) };
        let r = sel.put_cursor(text, at, extend);
        self.enter_insert(if extend { Range::new(r.anchor, at) } else { r }, t);
    }

    /// `o`, `O` (and `c` on whole lines): `count` lines opened below the
    /// selection (or above), each with a cursor, indented as Enter would,
    /// and going on with a line comment the line has (if `comments`).
    fn open(&mut self, t: &mut Target, target: Range, below: bool, count: usize, comments: bool) {
        if self.mode.reviewing() {
            self.refused = true;
            return;
        }
        let b = &*t.buffer;
        let line = b.line_of(if below { b.prev(target.to()) } else { target.from() });
        let language = t.language.unwrap_or(&Language::DEFAULT);
        let leader = language.comment_leader(b.line(line)).filter(|_| comments);
        let indent = match (leader, below, line.checked_sub(1)) {
            (Some(_), ..) => b.indent_at(b.line_start(line)).to_string(),
            (None, true, _) => indent::for_newline(b.line(line), language.indent_after, t.indent),
            (None, false, Some(above)) => indent::for_newline(b.line(above), language.indent_after, t.indent),
            (None, false, None) => b.indent_at(0).to_string(),
        };
        let one = format!("{indent}{}", leader.map(|l| format!("{l} ")).unwrap_or_default());
        let step = one.len() + 1;
        let (at, opened, first) = match (below, line.checked_sub(1)) {
            (true, _) => (b.line_end(line), format!("\n{one}").repeat(count), b.line_end(line) + step),
            (false, Some(above)) => (b.line_end(above), format!("\n{one}").repeat(count), b.line_end(above) + step),
            (false, None) => (0, format!("{one}\n").repeat(count), one.len()),
        };
        self.edit(t, at..at, &opened, Range::point(first), Mode::Insert);
        self.added = (1..count).map(|i| Range::point(first + i * step)).collect();
    }

    /// `J`: the selection's lines joined (or this one and the next), as
    /// helix joins them: each line's indent goes, and the comment leader it
    /// shares with the line before; a space goes between, unless what's
    /// joined is blank. With `select_spaces` (`alt-J`), the spaces are selected.
    fn join(&mut self, t: &mut Target, sel: Range, select_spaces: bool) {
        let b = &*t.buffer;
        let text = b.text();
        let first = b.line_of(sel.from());
        let mut last = b.line_of(b.prev(sel.to()).max(sel.from()));
        if first == last {
            last = (last + 1).min(b.line_count() - 1);
        }
        let language = t.language.unwrap_or(&Language::DEFAULT);
        let mut leader = language.comment_leader(b.line(first));
        // Each join: from the line's end to what's kept of the next, and whether a space goes there.
        let mut joins = vec![];
        for line in first..last {
            let next = b.line(line + 1);
            let mut end = b.line_start(line + 1) + next.len() - next.trim_start_matches([' ', '\t']).len();
            match language.comment_leader(next) {
                Some(next_leader) if Some(next_leader) == leader => {
                    end += next_leader.len();
                    end += text[end..].len() - text[end..].trim_start_matches([' ', '\t']).len();
                }
                Some(next_leader) => leader = Some(next_leader),
                None => {}
            }
            joins.push((b.line_end(line), end, end != b.line_end(line + 1)));
        }
        let (Some(&(from, ..)), Some(&(.., to, _))) = (joins.first(), joins.last()) else { return };
        let mut joined = String::new();
        let mut kept = from;
        for &(start, end, space) in &joins {
            joined.push_str(&text[kept..start]);
            joined.push_str(if space { " " } else { "" });
            kept = end;
        }
        // Where a place in the text is after the joins.
        let moved = |at: usize| {
            let mut by = 0isize;
            for &(start, end, space) in &joins {
                if at >= end {
                    by += space as isize - (end - start) as isize;
                } else if at > start {
                    return start.saturating_add_signed(by);
                } else {
                    break;
                }
            }
            at.saturating_add_signed(by)
        };
        let mut spaces = joins.iter().filter(|(.., space)| *space).map(|&(start, ..)| Range::point(moved(start)));
        let after = match spaces.next().filter(|_| select_spaces) {
            Some(space) => space,
            None => Range::new(moved(sel.anchor), moved(sel.head)),
        };
        let added: Vec<Range> = if select_spaces { spaces.collect() } else { vec![] };
        self.edit(t, from..to, &joined, after, self.mode);
        if !self.mode.reviewing() {
            self.added = added;
        }
    }

    /// The selection's text with each char changed by `change`; select mode ends.
    fn change_case(&mut self, t: &mut Target, sel: Range, change: impl Fn(char) -> Vec<char>) {
        let target = sel.min_width_1(t.buffer.text());
        let changed: String = t.buffer.slice(target.bytes()).chars().flat_map(change).collect();
        let after = same_shape(sel, target.from(), changed.len());
        self.edit(t, target.bytes(), &changed, after, Mode::Normal);
    }

    /// Paste after (or before) the selection; text ending in a newline goes
    /// on its own line, as yanked with `x`. The pasted text ends up selected.
    fn paste(&mut self, t: &mut Target, target: Range, text: &str, after: bool, count: usize) {
        if text.is_empty() {
            return;
        }
        let b = &*t.buffer;
        // A count can't paste more than 64 MB.
        let mut text = text.repeat(count.min((64 << 20) / text.len().max(1)).max(1));
        let linewise = text.ends_with('\n');
        let at = match (linewise, after) {
            (true, true) => (b.line_end_at(b.prev(target.to()).max(target.from())) + 1).min(b.len()),
            (true, false) => b.line_start_at(target.from()),
            (false, true) => target.to(),
            (false, false) => target.from(),
        };
        if linewise && at == b.len() && !b.is_empty() && b.line_start_at(at) != at {
            text = format!("\n{}", text.trim_end_matches('\n'));
        }
        let selected = Range::new(at, at + text.len());
        self.edit(t, at..at, &text, selected, Mode::Normal);
    }

    /// Replace `range` with `text`, then select `after` in `mode`. Nothing
    /// is edited while reviewing.
    fn edit(&mut self, t: &mut Target, range: std::ops::Range<usize>, text: &str, after: Range, mode: Mode) {
        if self.mode.reviewing() {
            self.refused = true;
            return;
        }
        t.buffer.edit(range, text);
        self.mode = mode;
        *t.selection = after.within(t.buffer.text());
    }
}

/// `from..to`, facing the way `sel` does.
fn facing(sel: Range, from: usize, to: usize) -> Range {
    if sel.head < sel.anchor { Range::new(to, from) } else { Range::new(from, to) }
}

/// Whether `r` is whole lines, line breaks and all (`x`), as helix's
/// `selection_is_linewise` asks before `c` opens a line in their place.
fn whole_lines(b: &Buffer, r: Range) -> bool {
    let text = b.text();
    let (from, to) = (r.from(), r.to());
    text[from..to].contains('\n') && b.line_start_at(from) == from && (to == b.len() || text[..to].ends_with('\n'))
}

/// `alt-x`: in to the whole lines in the selection, as helix's
/// `shrink_to_line_bounds`; on one line it stays as it is.
fn shrink_to_lines(b: &Buffer, r: Range) -> Range {
    let (first, last) = (b.line_of(r.from()), b.line_of(b.prev(r.to()).max(r.from())));
    if first == last {
        return r;
    }
    let start = if b.line_start(first) == r.from() { r.from() } else { movement::start_of(b, first + 1) };
    let end = if movement::start_of(b, last + 1) == r.to() { r.to() } else { b.line_start(last) };
    facing(r, start, end)
}

/// `_`: the selection without the whitespace at its ends; nothing but
/// whitespace, and it's the cursor alone.
fn trimmed(text: &str, r: Range) -> Range {
    let selected = &text[r.bytes()];
    if selected.trim().is_empty() {
        return Range::point(r.cursor(text));
    }
    let start = r.from() + selected.len() - selected.trim_start().len();
    facing(r, start, start + selected.trim().len())
}

/// `alt-s`: the selection split at its line breaks, as helix's
/// `split_on_newline` (the line breaks themselves left out).
fn split_lines(text: &str, r: Range) -> Vec<Range> {
    if r.is_empty() {
        return vec![r];
    }
    let mut lines = vec![];
    let mut start = r.from();
    for (i, _) in text[r.bytes()].match_indices('\n') {
        lines.push(Range::new(start, r.from() + i));
        start = r.from() + i + 1;
    }
    if start < r.to() {
        lines.push(Range::new(start, r.to()));
    }
    lines
}

/// `sel` re-made over a replacement `len` bytes long starting at `from`, in
/// its direction (a point stays a point).
fn same_shape(sel: Range, from: usize, len: usize) -> Range {
    if sel.is_empty() {
        Range::point(from)
    } else if sel.head < sel.anchor {
        Range::new(from + len, from)
    } else {
        Range::new(from, from + len)
    }
}

/// The number in `line` at `at` or after it: where, how long, and its value.
fn number_at(line: &str, at: usize) -> Option<(usize, usize, i128)> {
    let bytes = line.as_bytes();
    // Back to the start of a number the cursor is in.
    let mut start = at.min(line.len());
    while start > 0 && bytes[start - 1].is_ascii_digit() {
        start -= 1;
    }
    let start = start + line[start..].find(|c: char| c.is_ascii_digit())?;
    let negative = start > 0 && bytes[start - 1] == b'-';
    let from = if negative { start - 1 } else { start };
    let end = start + line[start..].find(|c: char| !c.is_ascii_digit()).unwrap_or(line.len() - start);
    Some((from, end - from, line[from..end].parse().ok()?))
}

/// `lines` commented out with `token` (at the least indent), or, if they
/// all are, back in. Blank lines are left be.
fn toggle_comment(lines: &str, token: &str) -> String {
    // Indentation is spaces and tabs (one byte each), so it's sliced on char boundaries.
    let lead = |l: &str| l.len() - l.trim_start_matches([' ', '\t']).len();
    let filled = || lines.split('\n').filter(|l| !l.trim().is_empty());
    let commented = filled().all(|l| l[lead(l)..].starts_with(token));
    let indent = filled().map(lead).min().unwrap_or(0);
    lines
        .split('\n')
        .map(|line| match () {
            _ if line.trim().is_empty() => line.to_string(),
            _ if commented => {
                let at = lead(line);
                let rest = &line[at + token.len()..];
                format!("{}{}", &line[..at], rest.strip_prefix(' ').unwrap_or(rest))
            }
            _ => format!("{}{token} {}", &line[..indent], &line[indent..]),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {

    #[test]
    fn commenting_lines_indented_with_odd_spaces_doesnt_split_a_char() {
        // A non-breaking space isn't indentation: it stays in the text.
        let out = super::toggle_comment(" a\n\u{a0}b", "//");
        assert_eq!(out, "//  a\n// \u{a0}b");
        assert_eq!(super::toggle_comment(&out, "//"), " a\n\u{a0}b", "and back");
    }
    #[test]
    fn comments_go_in_at_the_least_indent_and_come_back_out() {
        let code = "fn f() {\n    x();\n\n    y();\n}";
        let commented = super::toggle_comment(code, "//");
        assert_eq!(commented, "// fn f() {\n//     x();\n\n//     y();\n// }");
        assert_eq!(super::toggle_comment(&commented, "//"), code);
        assert_eq!(super::toggle_comment("    a\n    b", "#"), "    # a\n    # b");
        assert_eq!(super::number_at("v1.2 = 30", 5), Some((7, 2, 30)));
    }
}
