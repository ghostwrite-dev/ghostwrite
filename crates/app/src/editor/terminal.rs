//! The terminal under the code: the user's own shell. Terminal mode (`space t`,
//! `:term` or `cmd-j`) gives it the keyboard; Esc Esc goes to reading it with the
//! editor's keys, in the mode it was entered from (see [`super::reading`]; not
//! while a full-screen program like vim has it: Esc is its; a click on the code leaves then),
//! and `cmd-j` hides it. Dragging the mouse selects (a double click a word,
//! a triple click a line), and `cmd-c` copies it.
//! Claude is shown what's on it with every request (and, in a follow-up,
//! again when it's changed); what it types there is only a command the
//! user accepted ([`Editor::run_in_terminal`]). A command waiting to be accepted
//! shows here as a step does in a file: its note above the prompt, the command in
//! grey at the cursor, and the keys under it. It may go in a new tab, or be a Ctrl-C.
//! Tab sets it going and the story goes straight on: nothing waits for it to end. The shell
//! keeps running while it's hidden; when it exits, its tab closes, and with the
//! last the pane. Its top edge drags to resize it, and the height is kept for
//! next time.
//!
//! It has tabs of its own, each a shell of its own: in terminal mode, where
//! only ⌘ keys are the app's, the keys for tabs mean these. The tabs show once
//! there are two. Claude reads the one in front; a story's Ctrl-C goes to the one
//! its command ran in, wherever you've been since.
//! [`super::terminal_view`] draws it.

use std::path::{Path, PathBuf};

use futures::StreamExt as _;
use ghostwrite_ai::protocol::{Run, RunIn};
use ghostwrite_core::Mode;
use ghostwrite_terminal::{Button, Held, Modes, Press, Terminal};
use gpui::{
    AnyElement, Context, Keystroke, Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, ScrollWheelEvent, Task, Window, div,
    prelude::*,
};

use super::Editor;
use super::element::Metrics;
use super::terminal_view::TerminalView;
use crate::theme::{pt, theme};
use crate::ui::pane::{Edge, Resizable};

/// Bytes of the terminal Claude is told: the end.
const MAX_CONTEXT: usize = 8_000;
/// The pane's height to start with, and how small and how close to the
/// top of the window it can be dragged (in points at 100%, like [`pt`]).
const HEIGHT: f32 = 290.;
const MIN_HEIGHT: f32 = 60.;
const ROOM_ABOVE: f32 = 120.;
/// How long an Esc in the terminal waits for a second, which would leave, before it goes to the shell.
const ESCAPE_WAIT: std::time::Duration = std::time::Duration::from_millis(250);

type Point = gpui::Point<Pixels>;

pub(super) struct TerminalPane {
    pub(super) shown: bool,
    pub(super) height: Resizable,
    /// Its tabs, in order.
    pub(super) shells: Vec<Shell>,
    /// Which is in front.
    pub(super) active: usize,
    /// The number of the shell the story's command last ran in.
    ran_in: Option<usize>,
    /// An Esc that hasn't gone to the shell yet: another straight after leaves
    /// terminal mode, and the shell never sees either (zsh would take the next key
    /// as Alt and it); anything else, or a moment, and it goes on its way.
    held_escape: Option<(Vec<u8>, Task<()>)>,
    /// Scrolling that hasn't added up to a line yet.
    scrolled: f32,
    /// Where its cells are on screen, as last drawn: the top left one, and a cell's size.
    pub(super) cells: Option<(Point, Pixels, Pixels)>,
    /// The mouse is selecting.
    selecting: bool,
    /// A button down that the program in it is being told of (it asked for the
    /// mouse), and the cell it was last told the mouse is at.
    reporting: Option<(Button, (usize, usize))>,
    /// Its text, while the keys are reading it (see [`super::reading`]).
    pub(super) reading: Option<super::reading::Reading>,
    /// The link under the mouse while ⌘ is held: underlined, and ⌘-click opens it.
    pub(super) link: Option<Link>,
}

/// A link on the screen: on which row, across which columns, and to what.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Link {
    pub row: usize,
    pub cols: std::ops::Range<usize>,
    pub to: LinkTo,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) enum LinkTo {
    /// A web address, for the browser.
    Web(String),
    /// A file, at a line and column (from zero), for the editor.
    File(PathBuf, usize, usize),
}

/// The link at column `col` of a row whose cells are `row` (column, char): a web
/// address, or a file that's there (`src/main.rs:12:5`, as compilers print them).
pub(super) fn link_in(row: &[(usize, char)], col: usize, root: &Path) -> Option<(std::ops::Range<usize>, LinkTo)> {
    let width = row.iter().map(|(col, _)| col + 1).max()?;
    let mut chars = vec![' '; width];
    for &(at, c) in row {
        chars[at] = c;
    }
    if chars.get(col).is_none_or(|c| c.is_whitespace()) {
        return None;
    }
    // The word under the mouse: what's between spaces.
    let start = chars[..col].iter().rposition(|c| c.is_whitespace()).map_or(0, |at| at + 1);
    let end = chars[col..].iter().position(|c| c.is_whitespace()).map_or(width, |at| col + at);
    let word: String = chars[start..end].iter().collect();
    if let Some(from) = ["https://", "http://", "file://"].iter().filter_map(|scheme| word.find(scheme)).min() {
        let address = trim_address(&word[from..]);
        let (first, last) = (start + word[..from].chars().count(), start + word[..from].chars().count() + address.chars().count());
        return (first..last).contains(&col).then(|| (first..last, LinkTo::Web(address.to_string())));
    }
    // A path: the chars paths are made of, around the mouse.
    let first = chars[..=col].iter().rposition(|c| !super::reading::in_path(*c)).map_or(0, |at| at + 1);
    let last = chars[col..].iter().position(|c| !super::reading::in_path(*c)).map_or(width, |at| col + at);
    if first > col {
        return None;
    }
    let named: String = chars[first..last].iter().collect();
    let (path, line, column) = super::reading::located(&named, root, None)?;
    // What's underlined is the path and its numbers, not the punctuation after.
    let shown = named.trim_end_matches([':', '.']).chars().count();
    Some((first..first + shown, LinkTo::File(path, line, column)))
}

/// A web address as printed in text: punctuation after it isn't part of it, nor is a
/// closing bracket it doesn't open (`(see https://a.b/c)`).
fn trim_address(address: &str) -> &str {
    let mut address = address.trim_end_matches(['.', ',', ';', ':', '!', '?', '\'', '"', '>']);
    for (open, close) in [('(', ')'), ('[', ']')] {
        while address.ends_with(close) && address.matches(close).count() > address.matches(open).count() {
            address = &address[..address.len() - 1];
        }
    }
    address.trim_end_matches(['.', ',', ';', ':', '!', '?', '\'', '"'])
}

/// A tab of the terminal: a shell, and what listens to it.
pub(super) struct Shell {
    /// What its tab says: it keeps it when tabs move or others close.
    pub(super) number: usize,
    pub(super) terminal: Terminal,
    _task: Task<()>,
}

/// A terminal tab being dragged, and how it looks on the way.
#[derive(Clone)]
struct DraggedShell {
    ix: usize,
    number: usize,
}

impl gpui::Render for DraggedShell {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme(cx);
        div()
            .px_3()
            .rounded_sm()
            .border_1()
            .border_color(theme.border)
            .bg(theme.tab_active)
            .text_color(theme.tab_active_foreground)
            .text_xs()
            .child(format!("shell {}", self.number))
    }
}

impl Default for TerminalPane {
    fn default() -> Self {
        let height = Resizable::new("terminal-height", Edge::Top, Some(HEIGHT));
        Self {
            shown: false,
            height,
            shells: vec![],
            active: 0,
            ran_in: None,
            held_escape: None,
            scrolled: 0.,
            cells: None,
            selecting: false,
            reporting: None,
            reading: None,
            link: None,
        }
    }
}

impl TerminalPane {
    /// The shell in front.
    pub(super) fn current(&self) -> Option<&Terminal> {
        self.shells.get(self.active).map(|shell| &shell.terminal)
    }

    /// Another shell is in front: what was going on in the last one stops here.
    fn switch_to(&mut self, ix: usize) {
        self.active = ix.min(self.shells.len().saturating_sub(1));
        self.held_escape = None;
        self.selecting = false;
        self.scrolled = 0.;
    }

    /// How tall it can be dragged, in a window `room` points tall.
    pub(super) fn heights(room: f32) -> std::ops::RangeInclusive<f32> {
        MIN_HEIGHT..=(room - ROOM_ABOVE).max(MIN_HEIGHT)
    }
}

impl Editor {
    /// `space t`, `:term`, `cmd-j`, a click on the pane: into terminal mode,
    /// starting a shell if there's none.
    pub(super) fn enter_terminal(&mut self, cx: &mut Context<Self>) {
        if self.terminal.shells.is_empty()
            && let Err(e) = self.start_shell(cx)
        {
            self.project.say(e);
            return cx.notify();
        }
        self.leave_insert();
        self.stop_selecting();
        self.stop_reading();
        self.terminal.shown = true;
        self.terminal.held_escape = None;
        self.modal.enter(Mode::Terminal);
        cx.notify();
    }

    /// `cmd-j`: into terminal mode, or from it (or reading it), hide the terminal.
    pub(super) fn toggle_terminal(&mut self, cx: &mut Context<Self>) {
        if !self.in_terminal() {
            return self.enter_terminal(cx);
        }
        self.terminal.shown = false;
        self.stop_reading();
        self.modal.leave(Mode::Terminal);
        cx.notify();
    }

    /// Do what the story's accepted `run` says in the terminal, as if typed there, and
    /// show it: the command (what's on the command line is cleared first), in a new tab
    /// if it says so, or a Ctrl-C. A shell is started if there's none. A command can't be
    /// typed while a full-screen program has the terminal; a Ctrl-C can, it's the program's.
    pub(super) fn run_in_terminal(&mut self, run: &Run, cx: &mut Context<Self>) -> Result<(), String> {
        if self.terminal.shells.is_empty() || run.how == RunIn::NewTab {
            self.start_shell(cx)?;
        }
        let Some(terminal) = self.terminal.current() else { return Err("the terminal has no shell".into()) };
        if run.how == RunIn::Interrupt {
            terminal.input(b"\x03".to_vec());
        } else {
            if terminal.modes().full_screen {
                return Err("a program has the terminal: quit it, then ⇥ again".into());
            }
            // The line typed so far goes, so the command runs on its own.
            terminal.input(format!("{}{}\r", ghostwrite_terminal::CLEAR_LINE, run.command).into_bytes());
        }
        self.terminal.ran_in = self.terminal.shells.get(self.terminal.active).map(|shell| shell.number);
        self.terminal.shown = true;
        cx.notify();
        Ok(())
    }

    /// The story is at a command: the terminal shows, at its prompt, starting a
    /// shell if there's none.
    pub(super) fn show_terminal_for_command(&mut self, cx: &mut Context<Self>) {
        if self.terminal.shells.is_empty()
            && let Err(e) = self.start_shell(cx)
        {
            return self.project.say(e);
        }
        self.terminal.shown = true;
        // A Ctrl-C is for what the story set going, wherever you've been since.
        let interrupt = self.story.as_ref().and_then(|state| state.command_to_run()).is_some_and(|run| run.how == RunIn::Interrupt);
        if interrupt {
            self.back_to_the_command();
        }
        if let Some(terminal) = self.terminal.current() {
            terminal.scroll_to_bottom();
            if !interrupt && terminal.modes().full_screen {
                self.project.say("a program has the terminal: quit it to see the command");
            }
        }
    }

    /// Whether the terminal is showing a command for the story to run. It can't
    /// while it's hidden, or a full-screen program has it.
    pub(super) fn command_proposed(&self) -> bool {
        self.terminal.shown
            && self.terminal.current().is_some_and(|terminal| !terminal.modes().full_screen)
            && self.story.as_ref().is_some_and(|state| state.command_to_run().is_some())
    }

    /// The story's command is done: the shell it ran in comes to the front,
    /// so what it printed is what Claude reads.
    pub(super) fn back_to_the_command(&mut self) {
        let ran = self.terminal.shells.iter().position(|shell| Some(shell.number) == self.terminal.ran_in);
        if let Some(ix) = ran.filter(|ix| *ix != self.terminal.active) {
            self.terminal.switch_to(ix);
        }
    }

    /// A new shell in a tab right of the one in front, and in front.
    fn start_shell(&mut self, cx: &mut Context<Self>) -> Result<(), String> {
        let (tx, mut rx) = futures::channel::mpsc::unbounded();
        // Sized for real when it's first drawn.
        let terminal = Terminal::start(&self.project.root, 80, 24, move || drop(tx.unbounded_send(())))?;
        let number = (1..).find(|n| self.terminal.shells.iter().all(|shell| shell.number != *n)).unwrap_or(1);
        let task = cx.spawn(async move |this, cx| {
            while rx.next().await.is_some() {
                // One redraw for everything that's come in.
                while rx.try_recv().is_ok() {}
                let alive = this.update(cx, |this, cx| this.shell_changed(number, cx)).unwrap_or(false);
                if !alive {
                    break;
                }
            }
        });
        let ix = if self.terminal.shells.is_empty() { 0 } else { self.terminal.active + 1 };
        self.terminal.shells.insert(ix, Shell { number, terminal, _task: task });
        self.terminal.switch_to(ix);
        Ok(())
    }

    /// ⌘T in terminal mode: another shell.
    pub(super) fn new_shell(&mut self, cx: &mut Context<Self>) {
        if let Err(e) = self.start_shell(cx) {
            self.project.say(e);
        }
        self.read_the_shell_in_front(cx);
        cx.notify();
    }

    /// ⌘W in terminal mode, or a tab's ×: that shell goes, with whatever is running in it.
    pub(super) fn close_shell(&mut self, ix: usize, cx: &mut Context<Self>) {
        if ix >= self.terminal.shells.len() {
            return;
        }
        self.terminal.shells.remove(ix);
        let active = self.terminal.active;
        self.terminal.switch_to(if ix < active { active - 1 } else { active });
        if self.terminal.shells.is_empty() {
            self.terminal.shown = false;
            self.stop_reading();
            self.modal.leave(Mode::Terminal);
        }
        self.read_the_shell_in_front(cx);
        cx.notify();
    }

    /// ⌘⇧] and ⌘⇧[ in terminal mode: the next or previous shell, around the end.
    pub(super) fn cycle_shell(&mut self, forward: bool, cx: &mut Context<Self>) {
        let count = self.terminal.shells.len();
        if count > 1 {
            self.terminal.switch_to((self.terminal.active + if forward { 1 } else { count - 1 }) % count);
            self.read_the_shell_in_front(cx);
            cx.notify();
        }
    }

    /// ⌘⌥→ and ⌘⌥← in terminal mode: the shell in front moves over a place.
    pub(super) fn nudge_shell(&mut self, right: bool, cx: &mut Context<Self>) {
        let active = self.terminal.active;
        let to = if right { active + 1 } else { active.wrapping_sub(1) };
        self.move_shell(active, to, cx);
    }

    /// The tab at `from` goes to `to`; the same shell stays in front.
    fn move_shell(&mut self, from: usize, to: usize, cx: &mut Context<Self>) {
        let count = self.terminal.shells.len();
        if from == to || from >= count || to >= count {
            return;
        }
        let front = self.terminal.shells[self.terminal.active].number;
        let shell = self.terminal.shells.remove(from);
        self.terminal.shells.insert(to, shell);
        self.terminal.active = self.terminal.shells.iter().position(|shell| shell.number == front).unwrap_or(0);
        cx.notify();
    }

    /// There's something new on shell `number`, or it exited (then its tab
    /// closes, and `false`: there's nothing more to come).
    fn shell_changed(&mut self, number: usize, cx: &mut Context<Self>) -> bool {
        // Hidden, there's nothing to draw (Claude reads it when it asks).
        if self.terminal.shown {
            cx.notify();
        }
        let Some(ix) = self.terminal.shells.iter().position(|shell| shell.number == number) else { return false };
        if !self.terminal.shells[ix].terminal.exited() {
            self.read_again(number);
            return true;
        }
        self.close_shell(ix, cx);
        if self.terminal.shells.is_empty() {
            self.project.say("the terminal's shell exited");
        }
        false
    }

    /// What Claude is told: the end of what's on the terminal, if there's a shell.
    pub(super) fn terminal_context(&self) -> Option<String> {
        self.terminal.current().map(|t| t.text(MAX_CONTEXT)).filter(|text| !text.trim().is_empty())
    }

    /// A key in terminal mode: to the shell, unless it's the app's (cmd), a
    /// second Esc in a row, which leaves, or Tab with a story's command up.
    pub(super) fn terminal_key(&mut self, keystroke: &Keystroke, cx: &mut Context<Self>) -> bool {
        let Some(terminal) = self.terminal.current() else { return false };
        // In a full-screen program Esc is the program's, however often.
        let modes = terminal.modes();
        let escape = keystroke.key == "escape" && keystroke.modifiers == Modifiers::none() && !modes.full_screen;
        if escape {
            if self.terminal.held_escape.take().is_some() {
                self.modal.leave(Mode::Terminal);
                self.start_reading(false, cx);
                return true;
            }
            let Some(bytes) = key_bytes(keystroke, &modes) else { return false };
            let wait = cx.spawn(async move |this, cx| {
                cx.background_executor().timer(ESCAPE_WAIT).await;
                this.update(cx, |this, _| this.let_go_of_escape()).ok();
            });
            self.terminal.held_escape = Some((bytes, wait));
            return true;
        }
        if keystroke.key == "tab" && keystroke.modifiers == Modifiers::none() && !modes.full_screen && self.command_proposed() {
            self.terminal.held_escape = None;
            self.run_from_the_terminal(cx);
            return true;
        }
        let Some(bytes) = key_bytes(keystroke, &modes) else { return false };
        self.let_go_of_escape();
        if let Some(terminal) = self.terminal.current() {
            terminal.input(bytes);
        }
        true
    }

    /// An Esc held back goes to the shell after all.
    fn let_go_of_escape(&mut self) {
        if let Some((bytes, wait)) = self.terminal.held_escape.take() {
            wait.detach();
            if let Some(terminal) = self.terminal.current() {
                terminal.input(bytes);
            }
        }
    }

    /// `cmd-c` in terminal mode: what's selected.
    pub(super) fn terminal_copy(&mut self, cx: &mut Context<Self>) {
        if let Some(text) = self.terminal.current().and_then(Terminal::selected) {
            cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
        }
    }

    /// The cell under `position`: its row from the top of the view, and column.
    fn terminal_cell(&self, position: Point) -> Option<(usize, usize)> {
        let (origin, cw, lh) = self.terminal.cells?;
        let (x, y) = (f32::from(position.x - origin.x).max(0.), f32::from(position.y - origin.y).max(0.));
        Some(((y / f32::from(lh)) as usize, (x / f32::from(cw)).round() as usize))
    }

    /// The link at a point on the screen, if ⌘ is held: what's underlined.
    fn link_at(&self, position: Point, command: bool) -> Option<Link> {
        if !command {
            return None;
        }
        let (row, col) = self.terminal_cell_under(position)?;
        let screen = self.terminal.current()?.screen();
        let cells: Vec<(usize, char)> = screen.cells.iter().filter(|cell| cell.line == row).map(|cell| (cell.col, cell.c)).collect();
        let (cols, to) = link_in(&cells, col, &self.project.root)?;
        Some(Link { row, cols, to })
    }

    /// The mouse moved, or ⌘ went down or up: what's underlined follows.
    pub(super) fn hover_link(&mut self, position: Point, command: bool, cx: &mut Context<Self>) {
        let link = self.link_at(position, command);
        if link != self.terminal.link {
            self.terminal.link = link;
            cx.notify();
        }
    }

    /// The cell under a point (the one it's in, not the nearest boundary).
    fn terminal_cell_under(&self, position: Point) -> Option<(usize, usize)> {
        let (origin, cw, lh) = self.terminal.cells?;
        if position.x < origin.x || position.y < origin.y {
            return None;
        }
        let (x, y) = (f32::from(position.x - origin.x), f32::from(position.y - origin.y));
        Some(((y / f32::from(lh)) as usize, (x / f32::from(cw)) as usize))
    }

    fn terminal_mouse_down(&mut self, event: &MouseDownEvent, cx: &mut Context<Self>) {
        // ⌘-click on a link: the browser, or the file in the editor (before any program has the mouse).
        if event.button == MouseButton::Left
            && let Some(link) = self.link_at(event.position, event.modifiers.platform)
        {
            self.terminal.link = None;
            match link.to {
                LinkTo::Web(address) => cx.open_url(&address),
                LinkTo::File(path, line, column) => {
                    let from = self.before_jump();
                    self.modal.leave(Mode::Terminal);
                    self.stop_reading();
                    self.open_at(&path, line, column, cx);
                    self.note_jump(from);
                }
            }
            return cx.notify();
        }
        self.enter_terminal(cx);
        let Some((row, col)) = self.terminal_cell(event.position) else { return };
        // A program that asked for the mouse has it; shift-click still selects, as in
        // other terminals.
        if let Some(terminal) = self.terminal.current().filter(|t| t.modes().mouse && !event.modifiers.shift) {
            let button = button(event.button);
            terminal.mouse(button, Press::Down, row, col, held(&event.modifiers));
            self.terminal.reporting = Some((button, (row, col)));
            return;
        }
        if event.button != MouseButton::Left {
            return;
        }
        if let Some(terminal) = self.terminal.current() {
            terminal.select_from(row, col, event.click_count == 2, event.click_count >= 3);
            self.terminal.selecting = true;
            cx.notify();
        }
    }

    fn terminal_mouse_up(&mut self, event: &MouseUpEvent) {
        let Some((button, _)) = self.terminal.reporting.take() else { return };
        let Some((row, col)) = self.terminal_cell(event.position) else { return };
        if let Some(terminal) = self.terminal.current().filter(|t| t.modes().mouse) {
            terminal.mouse(button, Press::Up, row, col, held(&event.modifiers));
        }
    }

    fn terminal_mouse_move(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        self.hover_link(event.position, event.modifiers.platform, cx);
        // Moves, to a program that asked to be told them: dragging, or any.
        if let (Some((row, col)), Some(terminal)) = (self.terminal_cell(event.position), self.terminal.current()) {
            let modes = terminal.modes();
            let down = self.terminal.reporting.map(|(button, _)| button);
            let told = match down {
                Some(button) if modes.mouse_drag || modes.mouse_motion => Some(button),
                None if modes.mouse_motion => Some(Button::None),
                _ => None,
            };
            let last = self.terminal.reporting.map(|(_, at)| at);
            if let Some(button) = told
                && last != Some((row, col))
            {
                terminal.mouse(button, Press::Moved, row, col, held(&event.modifiers));
                if let Some((_, at)) = self.terminal.reporting.as_mut() {
                    *at = (row, col);
                }
                return;
            }
            if down.is_some() {
                return;
            }
        }
        if !self.terminal.selecting {
            return;
        }
        if event.pressed_button != Some(MouseButton::Left) {
            self.terminal.selecting = false;
            return;
        }
        if let (Some((row, col)), Some(terminal)) = (self.terminal_cell(event.position), self.terminal.current()) {
            terminal.select_to(row, col);
            cx.notify();
        }
    }

    /// `cmd-v` in terminal mode.
    pub(super) fn terminal_paste(&mut self, cx: &mut Context<Self>) {
        let Some(terminal) = self.terminal.current() else { return };
        let Some(text) = cx.read_from_clipboard().and_then(|c| c.text()) else { return };
        let text = if terminal.modes().bracketed_paste {
            format!("\x1b[200~{}\x1b[201~", text.replace('\x1b', ""))
        } else {
            text.replace("\r\n", "\r").replace('\n', "\r")
        };
        self.let_go_of_escape();
        if let Some(terminal) = self.terminal.current() {
            terminal.input(text.into_bytes());
        }
    }

    /// The wheel: back through the scrollback, or (in a full-screen program
    /// that asks for it) arrow keys.
    fn terminal_scroll(&mut self, event: &ScrollWheelEvent, window: &Window, cx: &mut Context<Self>) {
        let line_height = Metrics::measure(theme(cx), window).line_height;
        self.terminal.scrolled += f32::from(event.delta.pixel_delta(line_height).y) / f32::from(line_height);
        let lines = self.terminal.scrolled.trunc() as i32;
        if lines == 0 {
            return;
        }
        self.terminal.scrolled -= lines as f32;
        let cell = self.terminal_cell(event.position).unwrap_or((0, 0));
        let Some(terminal) = self.terminal.current() else { return };
        let modes = terminal.modes();
        // A program that asked for the mouse scrolls itself; one that only has the whole
        // screen gets arrow keys; otherwise it's the scrollback that moves.
        if modes.mouse {
            terminal.wheel(lines, cell.0, cell.1, held(&event.modifiers));
        } else if modes.alternate_scroll {
            let key = if lines > 0 { 'A' } else { 'B' };
            let arrow = if modes.app_cursor { format!("\x1bO{key}") } else { format!("\x1b[{key}") };
            terminal.input(arrow.repeat(lines.unsigned_abs() as usize).into_bytes());
        } else {
            terminal.scroll(lines);
        }
        cx.notify();
    }

    /// The terminal's tabs, once there are two.
    fn render_shell_tabs(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.terminal.shells.len() < 2 {
            return None;
        }
        let theme = theme(cx).clone();
        let tabs = self.terminal.shells.iter().enumerate().map(|(ix, shell)| {
            let number = shell.number;
            div()
                .id(("shell-tab", ix))
                .flex()
                .items_center()
                .gap_2()
                .px_3()
                .border_r_1()
                .border_color(theme.border)
                .cursor_pointer()
                .when(ix == self.terminal.active, |d| d.bg(theme.tab_active).text_color(theme.tab_active_foreground))
                .when(ix != self.terminal.active, |d| d.text_color(theme.tab_foreground))
                .debug_selector(move || format!("shell-tab-{ix}"))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.terminal.switch_to(ix);
                    this.enter_terminal(cx);
                }))
                .on_drag(DraggedShell { ix, number }, |shell, _, _, cx| cx.new(|_| shell.clone()))
                .drag_over::<DraggedShell>({
                    let accent = theme.accent;
                    move |style, _, _, _| style.border_l_2().border_color(accent)
                })
                .on_drop(cx.listener(move |this, dragged: &DraggedShell, _, cx| this.move_shell(dragged.ix, ix, cx)))
                .child(format!("shell {number}"))
                .child(
                    div()
                        .id(("shell-close", ix))
                        .px_1()
                        .rounded_sm()
                        .text_color(theme.muted_foreground)
                        .hover(|s| s.bg(theme.muted).text_color(theme.foreground))
                        .child("×")
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.close_shell(ix, cx);
                        })),
                )
        });
        Some(
            div()
                .id("shell-tabs")
                .flex_none()
                .flex()
                .h(pt(24.))
                .mb_1()
                .overflow_x_scroll()
                .bg(theme.tab_bar)
                .border_b_1()
                .border_color(theme.border)
                .text_xs()
                // A click here is about the tabs, not a selection on the screen.
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .children(tabs)
                .child(
                    div()
                        .id("shell-new")
                        .flex()
                        .items_center()
                        .px_3()
                        .cursor_pointer()
                        .text_color(theme.muted_foreground)
                        .hover(|s| s.text_color(theme.foreground))
                        .child("+")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.new_shell(cx);
                            this.enter_terminal(cx);
                        })),
                )
                .into_any_element(),
        )
    }

    pub(super) fn render_terminal(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.terminal.shown {
            return None;
        }
        let theme = theme(cx).clone();
        let active = self.in_terminal();
        Some(
            div()
                .id("terminal")
                .relative()
                .flex_none()
                .flex()
                .flex_col()
                .h(pt(self.terminal.height.size.unwrap_or(HEIGHT)))
                .px_2()
                .py_1()
                .border_t_1()
                .border_color(if active { theme.success } else { theme.border })
                .bg(theme.background)
                .on_mouse_down(MouseButton::Left, cx.listener(|this, event: &MouseDownEvent, _, cx| this.terminal_mouse_down(event, cx)))
                .on_mouse_down(MouseButton::Right, cx.listener(|this, event: &MouseDownEvent, _, cx| this.terminal_mouse_down(event, cx)))
                .on_mouse_down(MouseButton::Middle, cx.listener(|this, event: &MouseDownEvent, _, cx| this.terminal_mouse_down(event, cx)))
                .on_mouse_up_out(MouseButton::Left, cx.listener(|this, event: &MouseUpEvent, _, _| this.terminal_mouse_up(event)))
                .on_mouse_up(MouseButton::Left, cx.listener(|this, event: &MouseUpEvent, _, _| this.terminal_mouse_up(event)))
                .on_mouse_up(MouseButton::Right, cx.listener(|this, event: &MouseUpEvent, _, _| this.terminal_mouse_up(event)))
                .on_mouse_up(MouseButton::Middle, cx.listener(|this, event: &MouseUpEvent, _, _| this.terminal_mouse_up(event)))
                .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| this.terminal_mouse_move(event, cx)))
                .when(self.terminal.link.is_some(), |terminal| terminal.cursor_pointer())
                .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, window, cx| this.terminal_scroll(event, window, cx)))
                .children(self.render_shell_tabs(cx))
                .child(div().flex_1().min_h_0().debug_selector(|| "terminal-screen".into()).child(TerminalView::new(cx.entity(), active)))
                .child(self.terminal.height.handle("terminal-edge").on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, event: &MouseDownEvent, _, cx| {
                        let height = this.terminal.height.size.unwrap_or(HEIGHT);
                        this.terminal.height.grab(event.position, height);
                        cx.stop_propagation();
                    }),
                ))
                .into_any_element(),
        )
    }
}

/// What a key sends to the program in the terminal: as xterm sends it, or by kitty's
/// keyboard protocol if the program asked for that. `None` for keys that aren't the
/// terminal's (anything with cmd).
pub(super) fn key_bytes(keystroke: &Keystroke, modes: &Modes) -> Option<Vec<u8>> {
    if keystroke.modifiers.platform || keystroke.modifiers.function {
        return None;
    }
    if modes.keyboard != 0 {
        return kitty_key(keystroke, modes.keyboard);
    }
    legacy_key(keystroke, modes.app_cursor)
}

/// What a key sends to the shell, as xterm sends it; `None` for keys that
/// aren't the shell's (anything with cmd). Option types what the layout
/// says it types (`|`, `{` …), except with the keys that move and delete.
fn legacy_key(keystroke: &Keystroke, app_cursor: bool) -> Option<Vec<u8>> {
    let m = &keystroke.modifiers;
    if m.platform || m.function {
        return None;
    }
    // xterm's modifier parameter: 1, plus shift 1, alt 2, ctrl 4.
    let modifier = 1 + m.shift as u8 + 2 * m.alt as u8 + 4 * m.control as u8;
    let cursor = |c: char| match modifier {
        1 if app_cursor => format!("\x1bO{c}"),
        1 => format!("\x1b[{c}"),
        n => format!("\x1b[1;{n}{c}"),
    };
    let tilde = |code: u8| if modifier == 1 { format!("\x1b[{code}~") } else { format!("\x1b[{code};{modifier}~") };
    let named = match keystroke.key.as_str() {
        "enter" => Some("\r".to_string()),
        "tab" if m.shift => Some("\x1b[Z".into()),
        "tab" => Some("\t".into()),
        "escape" => Some("\x1b".into()),
        "backspace" if m.alt => Some("\x1b\x7f".into()),
        "backspace" if m.control => Some("\x08".into()),
        "backspace" => Some("\x7f".into()),
        "up" => Some(cursor('A')),
        "down" => Some(cursor('B')),
        "right" => Some(cursor('C')),
        "left" => Some(cursor('D')),
        "home" => Some(cursor('H')),
        "end" => Some(cursor('F')),
        "delete" => Some(tilde(3)),
        "pageup" => Some(tilde(5)),
        "pagedown" => Some(tilde(6)),
        "insert" => Some(tilde(2)),
        "f1" | "f2" | "f3" | "f4" => {
            let c = ['P', 'Q', 'R', 'S'][keystroke.key[1..].parse::<usize>().ok()? - 1];
            Some(if modifier == 1 { format!("\x1bO{c}") } else { format!("\x1b[1;{modifier}{c}") })
        }
        f if function_key(f).is_some() => function_key(f).map(tilde),
        _ => None,
    };
    if let Some(named) = named {
        return Some(named.into_bytes());
    }
    let mut chars = keystroke.key.chars();
    let single = chars.next().filter(|_| chars.next().is_none());
    if m.control {
        let control = match single? {
            c @ 'a'..='z' => c as u8 - b'a' + 1,
            '@' | ' ' | '2' => 0,
            '[' | '3' => 0x1b,
            '\\' | '4' => 0x1c,
            ']' | '5' => 0x1d,
            '^' | '6' => 0x1e,
            '_' | '-' | '7' => 0x1f,
            '?' | '8' => 0x7f,
            _ => return None,
        };
        return Some(if m.alt { vec![0x1b, control] } else { vec![control] });
    }
    let typed = keystroke.key_char.clone().or_else(|| single.map(|c| if m.shift { c.to_ascii_uppercase() } else { c }).map(String::from))?;
    (!typed.is_empty()).then(|| typed.into_bytes())
}

/// The number a function key from F5 sends (`CSI 15 ~`).
fn function_key(name: &str) -> Option<u8> {
    Some(match name {
        "f5" => 15,
        "f6" => 17,
        "f7" => 18,
        "f8" => 19,
        "f9" => 20,
        "f10" => 21,
        "f11" => 23,
        "f12" => 24,
        _ => return None,
    })
}

/// A key by kitty's keyboard protocol, with the program's `flags` (see [`Modes::keyboard`]):
/// keys told apart from what they used to share (Esc, ctrl-i and Tab), modifiers on any
/// key, and with flag 8 every key as `CSI code ; modifiers u`. Keys that move keep the
/// forms they had; only presses are sent.
fn kitty_key(keystroke: &Keystroke, flags: u8) -> Option<Vec<u8>> {
    let m = &keystroke.modifiers;
    let every = flags & 8 != 0;
    let modifiers = 1 + m.shift as u8 + 2 * m.alt as u8 + 4 * m.control as u8;
    let with = |n: u8| if n == 1 { String::new() } else { format!(";{n}") };
    let csi_u = |code: u32, shifted: Option<char>, text: Option<&str>| {
        let shifted = shifted.filter(|_| flags & 4 != 0).map_or(String::new(), |c| format!(":{}", c as u32));
        let text = text.filter(|t| flags & 16 != 0 && !t.is_empty()).map(|t| t.chars().map(|c| (c as u32).to_string()).collect::<Vec<_>>().join(":"));
        match text {
            Some(text) => format!("\x1b[{code}{shifted};{modifiers};{text}u"),
            None => format!("\x1b[{code}{shifted}{}u", with(modifiers)),
        }
        .into_bytes()
    };
    // Keys that move, and the function keys: as they always were, but always as CSI.
    let at = |c: char| if modifiers == 1 { format!("\x1b[{c}") } else { format!("\x1b[1;{modifiers}{c}") }.into_bytes();
    let tilde = |n: u8| format!("\x1b[{n}{}~", with(modifiers)).into_bytes();
    let key = keystroke.key.as_str();
    match key {
        "up" => return Some(at('A')),
        "down" => return Some(at('B')),
        "right" => return Some(at('C')),
        "left" => return Some(at('D')),
        "home" => return Some(at('H')),
        "end" => return Some(at('F')),
        "f1" => return Some(at('P')),
        "f2" => return Some(at('Q')),
        "f3" => return Some(tilde(13)),
        "f4" => return Some(at('S')),
        "insert" => return Some(tilde(2)),
        "delete" => return Some(tilde(3)),
        "pageup" => return Some(tilde(5)),
        "pagedown" => return Some(tilde(6)),
        f if function_key(f).is_some() => return function_key(f).map(tilde),
        _ => {}
    }
    // Enter, Tab and Backspace stay as they were unless modified (so a shell left in this
    // mode by a program that crashed still runs `reset`); Esc never does.
    let legacy = match key {
        "enter" => Some((13, "\r")),
        "tab" => Some((9, "\t")),
        "backspace" => Some((127, "\x7f")),
        "escape" => Some((27, "\x1b")),
        _ => None,
    };
    if let Some((code, bytes)) = legacy {
        let plain = modifiers == 1 && code != 27;
        return Some(if plain && !every { bytes.as_bytes().to_vec() } else { csi_u(code, None, None) });
    }
    let base = if key == "space" { ' ' } else { single_char(key)? };
    let typed = keystroke.key_char.clone().unwrap_or_else(|| if m.shift { base.to_uppercase().collect() } else { base.to_string() });
    // Option typing what the layout says (`|` on option-7): that's text, as before.
    if m.alt && !m.control && keystroke.key_char.as_deref().is_some_and(|t| t != key && !t.eq_ignore_ascii_case(key)) && !every {
        return Some(typed.into_bytes());
    }
    // Text, alone or shifted, is text; with ctrl or alt, or every key asked for, it's CSI u.
    if !every && !m.control && !m.alt {
        return Some(typed.into_bytes());
    }
    let shifted = m.shift.then(|| typed.chars().next()).flatten().filter(|c| *c != base);
    let text = (!m.control).then_some(typed.as_str());
    Some(csi_u(base.to_lowercase().next().unwrap_or(base) as u32, shifted, text))
}

/// The one char a key name is, if it is one.
fn single_char(key: &str) -> Option<char> {
    let mut chars = key.chars();
    chars.next().filter(|_| chars.next().is_none())
}

/// gpui's button, as a program in the terminal is told it.
fn button(button: MouseButton) -> Button {
    match button {
        MouseButton::Right => Button::Right,
        MouseButton::Middle => Button::Middle,
        _ => Button::Left,
    }
}

/// The keys held, as a program in the terminal is told them with the mouse.
fn held(modifiers: &Modifiers) -> Held {
    Held { shift: modifiers.shift, alt: modifiers.alt, ctrl: modifiers.control }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_reach_the_shell_as_xterm_sends_them() {
        let k = |s: &str| legacy_key(&Keystroke::parse(s).unwrap(), false);
        assert_eq!(k("enter"), Some(b"\r".to_vec()));
        assert_eq!(k("ctrl-c"), Some(vec![3]));
        assert_eq!(k("up"), Some(b"\x1b[A".to_vec()));
        assert_eq!(legacy_key(&Keystroke::parse("up").unwrap(), true), Some(b"\x1bOA".to_vec()), "application cursor keys");
        assert_eq!(k("ctrl-left"), Some(b"\x1b[1;5D".to_vec()));
        assert_eq!(k("shift-tab"), Some(b"\x1b[Z".to_vec()));
        assert_eq!(k("alt-backspace"), Some(b"\x1b\x7f".to_vec()));
        assert_eq!(k("shift-a"), Some(b"A".to_vec()));
        assert_eq!(k("cmd-v"), None, "the app's");
        let option_7 = Keystroke { key_char: Some("|".into()), ..Keystroke::parse("alt-7").unwrap() };
        assert_eq!(legacy_key(&option_7, false), Some(b"|".to_vec()), "option types what the layout says");
        assert_eq!(k("f1"), Some(b"\x1bOP".to_vec()));
        assert_eq!(k("f5"), Some(b"\x1b[15~".to_vec()));
    }

    #[test]
    fn keys_by_kittys_protocol_when_its_asked_for() {
        let k = |s: &str, flags: u8| kitty_key(&Keystroke::parse(s).unwrap(), flags).map(|b| String::from_utf8(b).unwrap());
        // Telling keys apart (flag 1).
        assert_eq!(k("ctrl-x", 1).as_deref(), Some("\x1b[120;5u"));
        assert_eq!(k("ctrl-s", 1).as_deref(), Some("\x1b[115;5u"));
        assert_eq!(k("escape", 1).as_deref(), Some("\x1b[27u"), "Esc isn't the start of a sequence any more");
        assert_eq!(k("alt-a", 1).as_deref(), Some("\x1b[97;3u"));
        assert_eq!(k("ctrl-shift-a", 1).as_deref(), Some("\x1b[97;6u"));
        assert_eq!(k("a", 1).as_deref(), Some("a"), "text is text");
        assert_eq!(k("shift-a", 1).as_deref(), Some("A"));
        assert_eq!(k("enter", 1).as_deref(), Some("\r"), "so a shell left like this still runs `reset`");
        assert_eq!(k("shift-enter", 1).as_deref(), Some("\x1b[13;2u"));
        assert_eq!(k("shift-tab", 1).as_deref(), Some("\x1b[9;2u"));
        assert_eq!(k("ctrl-up", 1).as_deref(), Some("\x1b[1;5A"), "keys that move keep their forms");
        assert_eq!(k("up", 1).as_deref(), Some("\x1b[A"));
        // Every key (flag 8), shifted keys (4) and text (16).
        assert_eq!(k("a", 8).as_deref(), Some("\x1b[97u"));
        assert_eq!(k("enter", 8).as_deref(), Some("\x1b[13u"));
        assert_eq!(k("shift-a", 8 | 4).as_deref(), Some("\x1b[97:65;2u"));
        assert_eq!(k("a", 8 | 16).as_deref(), Some("\x1b[97;1;97u"));
    }
}
