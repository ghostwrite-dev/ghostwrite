//! The terminal: the user's login shell on a pseudo-terminal, its screen
//! and scrollback kept by alacritty's emulator. Only the user's keys are
//! written to it (and the emulator's own answers to the programs in it).
//! What's on it is read back as a [`Screen`] of cells to draw, in this
//! crate's own terms, or as plain text, which is how Claude sees it.

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use alacritty_terminal::event::{Event, EventListener, WindowSize};
use alacritty_terminal::event_loop::{EventLoop, EventLoopSender, Msg};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{Config, Term, TermMode};
use alacritty_terminal::tty;

mod links;
mod screen;

pub use links::{LinkTo, link_in, located, path_at};
pub use screen::{Cell, Color, Cursor, CursorShape, Screen};

/// The grid's size, as the emulator takes it.
struct Size {
    cols: usize,
    lines: usize,
}

impl Dimensions for Size {
    fn total_lines(&self) -> usize {
        self.lines
    }

    fn screen_lines(&self) -> usize {
        self.lines
    }

    fn columns(&self) -> usize {
        self.cols
    }
}

impl From<WindowSize> for Size {
    fn from(size: WindowSize) -> Self {
        Self { cols: size.num_cols.into(), lines: size.num_lines.into() }
    }
}

/// What the program in it has asked of the terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Modes {
    /// A full-screen program (vim, helix, fzf, less) has the screen.
    pub full_screen: bool,
    /// Arrow keys send their application codes (`\eOA`).
    pub app_cursor: bool,
    /// A paste comes between markers, so it isn't run as typed.
    pub bracketed_paste: bool,
    /// The wheel sends arrow keys (in a full-screen program).
    pub alternate_scroll: bool,
    /// The program has asked for the mouse: clicks and the wheel go to it.
    pub mouse: bool,
    /// ... and moves while a button's down (`drag`), or any move (`motion`).
    pub mouse_drag: bool,
    pub mouse_motion: bool,
    /// ... in the SGR encoding (`\e[<64;10;5M`), not the old one of bytes.
    pub sgr_mouse: bool,
    /// What the program asked of the keyboard by kitty's protocol: its flags (1 tell
    /// keys apart, 2 report releases, 4 shifted keys too, 8 every key as an escape, 16
    /// the text too). 0 is the old way.
    pub keyboard: u8,
}

/// Lines kept above the screen.
const SCROLLBACK: usize = 10_000;

pub struct Terminal {
    term: Arc<FairMutex<Term<Events>>>,
    events: Events,
    /// Where the user started typing on the shell's line: its line counted from
    /// the top of the scrollback, and its column.
    typed_from: Mutex<Option<(usize, usize)>>,
    /// The terminal's device and the shell's process, to tell whether the shell has
    /// the terminal (at its prompt) or a program it started does.
    #[cfg(unix)]
    foreground: Option<(std::fs::File, i32)>,
}

/// What the emulator reports: it's changed (`on_change`, called on its
/// thread), the shell exited, or it has an answer to write back.
#[derive(Clone)]
pub struct Events {
    sender: Arc<OnceLock<EventLoopSender>>,
    size: Arc<Mutex<WindowSize>>,
    exited: Arc<AtomicBool>,
    on_change: Arc<dyn Fn() + Send + Sync>,
}

impl Events {
    fn write(&self, bytes: impl Into<Cow<'static, [u8]>>) {
        if let Some(sender) = self.sender.get() {
            sender.send(Msg::Input(bytes.into())).ok();
        }
    }
}

impl EventListener for Events {
    fn send_event(&self, event: Event) {
        match event {
            Event::Wakeup => {}
            Event::PtyWrite(text) => return self.write(text.into_bytes()),
            Event::TextAreaSizeRequest(answer) => {
                let size = *self.size.lock().unwrap();
                return self.write(answer(size).into_bytes());
            }
            Event::Exit | Event::ChildExit(_) => self.exited.store(true, Ordering::SeqCst),
            _ => return,
        }
        (self.on_change)();
    }
}

/// The user's shell and how it's started: `$SHELL` as a login shell; on Windows PowerShell.
fn user_shell() -> (String, Vec<String>) {
    if cfg!(windows) {
        let path = std::env::var_os("PATH").unwrap_or_default();
        let newer = std::env::split_paths(&path).any(|dir| dir.join("pwsh.exe").is_file());
        return (if newer { "pwsh.exe" } else { "powershell.exe" }.to_string(), vec!["-NoLogo".into()]);
    }
    let fallback = if cfg!(target_os = "macos") { "/bin/zsh" } else { "/bin/bash" };
    (std::env::var("SHELL").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| fallback.into()), vec!["-l".into()])
}

/// What clears what's been typed on the shell's line: Ctrl-E then Ctrl-U (to the
/// end, then all of it back to the prompt, in bash's way as well as zsh's), or Esc
/// for PowerShell.
pub const CLEAR_LINE: &str = if cfg!(windows) { "\x1b" } else { "\x05\x15" };

impl Terminal {
    /// Start the user's shell (see [`user_shell`]) in `dir`,
    /// `cols` by `rows`. `on_change` is called (on the terminal's thread) when
    /// there's something new to draw, and when the shell exits.
    pub fn start(dir: &Path, cols: u16, rows: u16, on_change: impl Fn() + Send + Sync + 'static) -> Result<Self, String> {
        let (shell, shell_args) = user_shell();
        let size = WindowSize { num_lines: rows.max(1), num_cols: cols.max(1), cell_width: 8, cell_height: 16 };
        let mut env = HashMap::from([("TERM".to_string(), "xterm-256color".to_string()), ("COLORTERM".to_string(), "truecolor".to_string())]);
        // Launched from the Finder there's no locale, and the shell would draw UTF-8 as escapes.
        if std::env::var_os("LANG").is_none() {
            env.insert("LANG".into(), "en_US.UTF-8".into());
        }
        let options = tty::Options {
            shell: Some(tty::Shell::new(shell, shell_args)),
            working_directory: Some(dir.to_path_buf()),
            drain_on_exit: false,
            env,
            #[cfg(windows)]
            escape_args: false,
        };
        let pty = tty::new(&options, size, 0).map_err(|e| format!("couldn't start a shell: {e}"))?;
        #[cfg(unix)]
        let foreground = pty.file().try_clone().ok().map(|file| (file, pty.child().id() as i32));
        let events = Events { sender: Arc::default(), size: Arc::new(Mutex::new(size)), exited: Arc::default(), on_change: Arc::new(on_change) };
        // Programs that ask (`CSI ? u`) are told kitty's keyboard protocol is here.
        let config = Config { scrolling_history: SCROLLBACK, kitty_keyboard: true, ..Config::default() };
        let term = Arc::new(FairMutex::new(Term::new(config, &Size::from(size), events.clone())));
        let event_loop = EventLoop::new(term.clone(), events.clone(), pty, false, false).map_err(|e| format!("couldn't start a shell: {e}"))?;
        events.sender.set(event_loop.channel()).ok();
        event_loop.spawn();
        Ok(Self {
            term,
            events,
            typed_from: Mutex::new(None),
            #[cfg(unix)]
            foreground,
        })
    }

    /// What's on the screen now, to draw.
    pub fn screen(&self) -> Screen {
        Screen::of(&self.term.lock())
    }

    /// What the program in it has asked for.
    pub fn modes(&self) -> Modes {
        let mode = *self.term.lock().mode();
        Modes {
            full_screen: mode.contains(TermMode::ALT_SCREEN),
            app_cursor: mode.contains(TermMode::APP_CURSOR),
            bracketed_paste: mode.contains(TermMode::BRACKETED_PASTE),
            alternate_scroll: mode.contains(TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL),
            mouse: mode.intersects(TermMode::MOUSE_MODE),
            mouse_drag: mode.contains(TermMode::MOUSE_DRAG),
            mouse_motion: mode.contains(TermMode::MOUSE_MOTION),
            sgr_mouse: mode.contains(TermMode::SGR_MOUSE),
            keyboard: [
                (TermMode::DISAMBIGUATE_ESC_CODES, 1),
                (TermMode::REPORT_EVENT_TYPES, 2),
                (TermMode::REPORT_ALTERNATE_KEYS, 4),
                (TermMode::REPORT_ALL_KEYS_AS_ESC, 8),
                (TermMode::REPORT_ASSOCIATED_TEXT, 16),
            ]
            .into_iter()
            .filter(|(flag, _)| mode.contains(*flag))
            .map(|(_, bit)| bit)
            .sum(),
        }
    }

    /// What the user typed: to the shell, with the view back at the bottom
    /// (and nothing selected).
    pub fn input(&self, bytes: impl Into<Cow<'static, [u8]>>) {
        let bytes = bytes.into();
        let mut term = self.term.lock();
        term.scroll_display(Scroll::Bottom);
        term.selection = None;
        let mut from = self.typed_from.lock().unwrap();
        // Enter, Ctrl-C, Ctrl-D, Ctrl-L or a cleared line: a new line to type on.
        let ends = bytes.iter().any(|b| matches!(b, b'\r' | b'\n' | 0x03 | 0x04 | 0x0c)) || bytes.ends_with(CLEAR_LINE.as_bytes());
        if ends || term.mode().contains(TermMode::ALT_SCREEN) {
            *from = None;
        } else if from.is_none() {
            let cursor = term.grid().cursor.point;
            *from = Some((term.grid().history_size() + cursor.line.0.max(0) as usize, cursor.column.0));
        }
        drop((from, term));
        self.events.write(bytes);
    }

    /// The wheel turned `lines` (up when positive) over cell `row`, `col` of the screen, as
    /// a program that asked for the mouse is told it.
    pub fn wheel(&self, lines: i32, row: usize, col: usize, held: Held) {
        let button = if lines > 0 { Button::WheelUp } else { Button::WheelDown };
        let one = mouse_bytes(self.modes().sgr_mouse, button, Press::Down, row, col, held);
        self.events.write(one.repeat(lines.unsigned_abs() as usize));
    }

    /// A button pressed or let go, or the mouse moved, over cell `row`, `col`, as a program
    /// that asked for the mouse is told it.
    pub fn mouse(&self, button: Button, press: Press, row: usize, col: usize, held: Held) {
        self.events.write(mouse_bytes(self.modes().sgr_mouse, button, press, row, col, held));
    }

    /// Whether a program the shell started has the terminal (a command is running), or
    /// the shell has it back, at its prompt. `None` where that can't be told (Windows).
    pub fn busy(&self) -> Option<bool> {
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd as _;
            let (file, shell) = self.foreground.as_ref()?;
            // SAFETY: the descriptor is ours, open for as long as `file` is.
            let group = unsafe { libc::tcgetpgrp(file.as_raw_fd()) };
            (group > 0).then_some(group != *shell)
        }
        #[cfg(not(unix))]
        None
    }

    /// Where what the user has typed on the shell's line starts, on the screen
    /// as [`screen`](Self::screen) has it, if they've typed on it: what was
    /// printed since on lines of its own (typed ahead while a command ran) isn't it.
    pub fn typed_from(&self) -> Option<(usize, usize)> {
        let term = self.term.lock();
        let (line, col) = (*self.typed_from.lock().unwrap())?;
        let grid = term.grid();
        let start = line as i64 - grid.history_size() as i64;
        let cursor = grid.cursor.point;
        let end = i64::from(cursor.line.0);
        if start < 0 || start > end || start == end && col > cursor.column.0 {
            return None;
        }
        let last = Column(grid.columns().saturating_sub(1));
        if (start..end).any(|line| !grid[Line(line as i32)][last].flags.contains(Flags::WRAPLINE)) {
            return None;
        }
        let shown = start + grid.display_offset() as i64;
        (shown < grid.screen_lines() as i64).then_some((shown as usize, col))
    }

    /// Start selecting at a cell on screen (`row` from the top of the view);
    /// `words` selects whole words (a double click), `lines` whole lines.
    pub fn select_from(&self, row: usize, col: usize, words: bool, lines: bool) {
        let mut term = self.term.lock();
        let at = Point::new(Line(row as i32 - term.grid().display_offset() as i32), Column(col));
        let kind = if lines {
            SelectionType::Lines
        } else if words {
            SelectionType::Semantic
        } else {
            SelectionType::Simple
        };
        term.selection = Some(Selection::new(kind, at, Side::Left));
    }

    /// Select on to a cell on screen (the mouse dragged there).
    pub fn select_to(&self, row: usize, col: usize) {
        let mut term = self.term.lock();
        let at = Point::new(Line(row as i32 - term.grid().display_offset() as i32), Column(col));
        if let Some(selection) = term.selection.as_mut() {
            selection.update(at, Side::Right);
        }
    }

    /// What's selected, as text.
    pub fn selected(&self) -> Option<String> {
        self.term.lock().selection_to_string().filter(|s| !s.is_empty())
    }

    /// Fit it to `cols` by `rows` cells of `cell_width` by `cell_height` pixels.
    pub fn resize(&self, cols: u16, rows: u16, cell_width: u16, cell_height: u16) {
        let size = WindowSize { num_lines: rows.max(1), num_cols: cols.max(1), cell_width, cell_height };
        let mut was = self.events.size.lock().unwrap();
        if (was.num_lines, was.num_cols) == (size.num_lines, size.num_cols) {
            return;
        }
        *was = size;
        // Let go first: the emulator's thread takes the term, then this (a
        // program asking the size), so holding both here could deadlock.
        drop(was);
        self.term.lock().resize(Size::from(size));
        if let Some(sender) = self.events.sender.get() {
            sender.send(Msg::Resize(size)).ok();
        }
    }

    /// Scroll the view through the scrollback: up for positive `lines`.
    pub fn scroll(&self, lines: i32) {
        self.term.lock().scroll_display(Scroll::Delta(lines));
    }

    /// Back from the scrollback, to the cursor.
    pub fn scroll_to_bottom(&self) {
        self.term.lock().scroll_display(Scroll::Bottom);
    }

    /// The scrollback and screen, to read through (see [`Transcript`]).
    pub fn transcript(&self) -> Transcript {
        Transcript::of(&self.term.lock())
    }

    /// The shell's cursor, as a cell of the [`Transcript`]'s grid.
    pub fn cursor_cell(&self) -> (usize, usize) {
        let term = self.term.lock();
        let grid = term.grid();
        let at = grid.cursor.point;
        ((at.line.0 + grid.history_size() as i32).max(0) as usize, at.column.0)
    }

    /// Select from one cell to another (rows from the top of the scrollback),
    /// both in, as the mouse would; `None`, nothing.
    pub fn mark(&self, cells: Option<((usize, usize), (usize, usize))>) {
        let mut term = self.term.lock();
        let history = term.grid().history_size() as i32;
        term.selection = cells.map(|(from, to)| {
            let point = |(row, col): (usize, usize)| Point::new(Line(row as i32 - history), Column(col));
            let mut selection = Selection::new(SelectionType::Simple, point(from), Side::Left);
            selection.update(point(to), Side::Right);
            selection
        });
    }

    /// Where a cell (its row from the top of the scrollback) is in the view: its row from the top.
    pub fn in_view(&self, row: usize) -> Option<usize> {
        let term = self.term.lock();
        let grid = term.grid();
        let top = grid.history_size() as i64 - grid.display_offset() as i64;
        let shown = row as i64 - top;
        (0..grid.screen_lines() as i64).contains(&shown).then_some(shown as usize)
    }

    /// Scroll the view as little as shows the row (from the top of the scrollback).
    pub fn show(&self, row: usize) {
        let mut term = self.term.lock();
        let grid = term.grid();
        let (history, offset, rows) = (grid.history_size() as i64, grid.display_offset() as i64, grid.screen_lines() as i64);
        let top = history - offset;
        let delta = if (row as i64) < top {
            top - row as i64
        } else if row as i64 >= top + rows {
            top + rows - 1 - row as i64
        } else {
            return;
        };
        term.scroll_display(Scroll::Delta(delta as i32));
    }

    pub fn exited(&self) -> bool {
        self.events.exited.load(Ordering::SeqCst)
    }

    /// The scrollback and screen as plain text, ending at the last line with
    /// anything on it: at most `max_bytes` from the end, whole lines.
    pub fn text(&self, max_bytes: usize) -> String {
        let lines = transcript(&self.term.lock());
        let mut tail = vec![];
        let mut bytes = 0;
        for line in lines.iter().rev() {
            bytes += line.len() + 1;
            if bytes > max_bytes {
                tail.push("… (earlier lines cut)");
                break;
            }
            tail.push(line);
        }
        tail.reverse();
        tail.join("\n")
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        // The loop ends, and with it the pty, which hangs up on the shell.
        if let Some(sender) = self.events.sender.get() {
            sender.send(Msg::Shutdown).ok();
        }
    }
}

/// A mouse button, as a program is told it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    Left,
    Middle,
    Right,
    /// Moving with none down.
    None,
    WheelUp,
    WheelDown,
}

/// What happened to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Press {
    Down,
    Up,
    /// Moved while it's down (or with none, `Button::None`).
    Moved,
}

/// The keys held with it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Held {
    pub shift: bool,
    pub alt: bool,
    pub ctrl: bool,
}

/// The bytes that tell a program of a mouse event at cell `row`, `col` (from the top left,
/// from 0): xterm's SGR encoding, or the old one of bytes, which can't go past cell 223.
pub fn mouse_bytes(sgr: bool, button: Button, press: Press, row: usize, col: usize, held: Held) -> Vec<u8> {
    let mut code: u8 = match button {
        Button::Left => 0,
        Button::Middle => 1,
        Button::Right => 2,
        Button::None => 3,
        Button::WheelUp => 64,
        Button::WheelDown => 65,
    };
    code += 4 * held.shift as u8 + 8 * held.alt as u8 + 16 * held.ctrl as u8;
    if press == Press::Moved {
        code += 32;
    }
    let (row, col) = (row + 1, col + 1);
    if sgr {
        let end = if press == Press::Up { 'm' } else { 'M' };
        return format!("\x1b[<{code};{col};{row}{end}").into_bytes();
    }
    // The old way says a release with the button 3, whichever it was.
    if press == Press::Up {
        code = (code & !3) | 3;
    }
    let at = |n: usize| (32 + n.min(223)) as u8;
    vec![0x1b, b'[', b'M', 32 + code, at(col), at(row)]
}

/// Every line, oldest first: lines the screen wrapped are joined again,
/// and blank lines at the end are left off.
fn transcript<T>(term: &Term<T>) -> Vec<String> {
    let text = Transcript::of(term).text;
    if text.is_empty() { vec![] } else { text.split('\n').map(str::to_string).collect() }
}

/// The scrollback and screen as text to read through: a line for each line
/// the program printed (rows it wrapped onto are joined), and where each char
/// of it is on the grid. Rows count from the top of the scrollback, so they
/// stay where they are as more comes in (until the oldest go).
#[derive(Debug, Clone, Default)]
pub struct Transcript {
    pub text: String,
    /// Rows on the screen.
    pub rows: usize,
    lines: Vec<Placed>,
    cols: usize,
}

/// A line of the transcript: the row it starts on, and, if it has wide
/// chars (which take two columns), each char's row and column.
#[derive(Debug, Clone)]
struct Placed {
    row: usize,
    wide: Option<Vec<(usize, usize)>>,
}

impl Transcript {
    fn of<T>(term: &Term<T>) -> Self {
        let grid = term.grid();
        let (cols, history) = (grid.columns(), grid.history_size());
        let mut texts: Vec<String> = vec![];
        let mut lines: Vec<Placed> = vec![];
        let mut joining = false;
        for line in grid.topmost_line().0..=grid.bottommost_line().0 {
            let row_ix = (line + history as i32) as usize;
            let row = &grid[Line(line)];
            if !joining {
                texts.push(String::new());
                lines.push(Placed { row: row_ix, wide: None });
            }
            let (text, placed) = (texts.last_mut().unwrap(), lines.last_mut().unwrap());
            let before = text.chars().count();
            let mut cells = vec![];
            for col in 0..cols {
                let cell = &row[Column(col)];
                if !cell.flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
                    text.push(cell.c);
                    cells.push((row_ix, col));
                }
            }
            if cells.len() != cols && placed.wide.is_none() {
                // Wide chars: from here on each char's cell is kept.
                let first = placed.row;
                placed.wide = Some((0..before).map(|k| (first + k / cols, k % cols)).collect());
            }
            if let Some(wide) = placed.wide.as_mut() {
                wide.extend(cells);
            }
            let wraps = row[Column(cols - 1)].flags.contains(Flags::WRAPLINE);
            if !wraps {
                let kept = text.trim_end().len();
                text.truncate(kept);
                if let Some(wide) = placed.wide.as_mut() {
                    wide.truncate(text.chars().count());
                }
            }
            joining = wraps;
        }
        while texts.len() > 1 && texts.last().is_some_and(String::is_empty) {
            texts.pop();
            lines.pop();
        }
        Self { text: texts.join("\n"), rows: grid.screen_lines(), lines, cols }
    }

    /// The grid cell (row from the top of the scrollback, column) of the
    /// `ch`th char of line `line` (past its end: where it would go).
    pub fn cell(&self, line: usize, ch: usize) -> (usize, usize) {
        let Some(placed) = self.lines.get(line.min(self.lines.len().saturating_sub(1))) else { return (0, 0) };
        if let Some(wide) = &placed.wide
            && let Some(at) = wide.get(ch)
        {
            return *at;
        }
        let cols = self.cols.max(1);
        (placed.row + ch / cols, ch % cols)
    }

    /// The line, and char along it, of a grid cell (or the nearest there is).
    pub fn at(&self, row: usize, col: usize) -> (usize, usize) {
        let line = self.lines.partition_point(|placed| placed.row <= row).saturating_sub(1);
        let Some(placed) = self.lines.get(line) else { return (0, 0) };
        let ch = match &placed.wide {
            Some(wide) => wide.partition_point(|at| *at < (row, col)),
            None => (row.saturating_sub(placed.row)) * self.cols + col,
        };
        (line, ch)
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use alacritty_terminal::event::VoidListener;
    use alacritty_terminal::vte::ansi::Processor;

    use super::*;

    #[test]
    fn the_mouse_is_told_as_xterm_tells_it() {
        let held = Held::default();
        let sgr = |button, press, row, col| String::from_utf8(mouse_bytes(true, button, press, row, col, held)).unwrap();
        assert_eq!(sgr(Button::Left, Press::Down, 4, 9), "\x1b[<0;10;5M");
        assert_eq!(sgr(Button::Left, Press::Up, 4, 9), "\x1b[<0;10;5m");
        assert_eq!(sgr(Button::Left, Press::Moved, 4, 9), "\x1b[<32;10;5M", "dragged");
        assert_eq!(sgr(Button::None, Press::Moved, 0, 0), "\x1b[<35;1;1M", "moved with none down");
        assert_eq!(sgr(Button::WheelDown, Press::Down, 0, 0), "\x1b[<65;1;1M");
        let ctrl = mouse_bytes(true, Button::Right, Press::Down, 0, 0, Held { ctrl: true, ..held });
        assert_eq!(ctrl, b"\x1b[<18;1;1M");
        assert_eq!(mouse_bytes(false, Button::Left, Press::Down, 0, 0, held), [0x1b, b'[', b'M', 32, 33, 33]);
        assert_eq!(mouse_bytes(false, Button::Right, Press::Up, 0, 0, held), [0x1b, b'[', b'M', 35, 33, 33], "a release is button 3");
    }

    #[test]
    fn reads_back_the_scrollback_and_screen() {
        let mut term = Term::new(Config::default(), &Size { cols: 10, lines: 3 }, VoidListener);
        let mut parser: Processor = Processor::new();
        parser.advance(&mut term, b"one\r\ntwo\r\n\x1b[31mred\x1b[0m\r\nabcdefghijklmno\r\nlast\r\n");
        assert_eq!(transcript(&term), ["one", "two", "red", "abcdefghijklmno", "last"]);
    }

    #[test]
    fn a_transcripts_chars_know_their_cells() {
        let mut term = Term::new(Config::default(), &Size { cols: 10, lines: 3 }, VoidListener);
        let mut parser: Processor = Processor::new();
        parser.advance(&mut term, "one\r\nabcdefghijklmno\r\n日本x\r\nlast".as_bytes());
        let read = Transcript::of(&term);
        assert_eq!(read.text, "one\nabcdefghijklmno\n日本x\nlast");
        assert_eq!(read.cell(1, 12), (2, 2), "wrapped onto the next row");
        assert_eq!(read.at(2, 2), (1, 12));
        assert_eq!(read.cell(2, 2), (3, 4), "after two wide chars");
        assert_eq!(read.at(3, 4), (2, 2));
        assert_eq!(read.cell(3, 1), (4, 1));
    }

    /// Once the shell's prompt is up, and has stayed where it is a moment.
    fn prompted(terminal: &Terminal) -> (usize, usize) {
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut last = None;
        loop {
            std::thread::sleep(Duration::from_millis(300));
            let now = terminal.screen().cursor.map(|c| (c.line, c.col)).filter(|(_, col)| *col > 0);
            if let Some(at) = now.filter(|_| now == last) {
                return at;
            }
            assert!(Instant::now() < deadline, "no prompt: {}", terminal.text(10_000));
            last = now;
        }
    }

    fn shows(terminal: &Terminal, what: &str, shown: bool) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while terminal.text(10_000).contains(what) != shown {
            assert!(Instant::now() < deadline, "{what} {}: {}", if shown { "never came" } else { "never went" }, terminal.text(10_000));
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    #[test]
    #[cfg(unix)]
    fn it_knows_when_a_command_is_running() {
        let terminal = Terminal::start(&std::env::temp_dir(), 80, 24, || {}).unwrap();
        prompted(&terminal);
        assert_eq!(terminal.busy(), Some(false), "at the prompt");
        terminal.input(b"sleep 1\r".as_slice());
        let deadline = Instant::now() + Duration::from_secs(5);
        while terminal.busy() != Some(true) {
            assert!(Instant::now() < deadline, "the command never ran");
            std::thread::sleep(Duration::from_millis(20));
        }
        while terminal.busy() != Some(false) {
            assert!(Instant::now() < deadline, "it never finished");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    #[cfg(unix)]
    fn the_wheel_goes_to_a_program_that_asked_for_the_mouse() {
        let terminal = Terminal::start(&std::env::temp_dir(), 80, 24, || {}).unwrap();
        prompted(&terminal);
        assert!(!terminal.modes().mouse, "a shell doesn't ask");
        // Ask for the mouse in the SGR encoding, then show what comes in.
        terminal.input(b"printf '\\033[?1000h\\033[?1006h'; cat -v\r".as_slice());
        let deadline = Instant::now() + Duration::from_secs(10);
        while !terminal.modes().mouse {
            assert!(Instant::now() < deadline, "it never asked: {}", terminal.text(10_000));
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(terminal.modes().sgr_mouse);
        terminal.wheel(1, 4, 9, Held::default());
        terminal.input(b"\r".as_slice());
        shows(&terminal, "[<64;10;5M", true);
        terminal.input(b"\x03".as_slice());
    }

    #[test]
    fn knows_where_the_user_started_typing() {
        let terminal = Terminal::start(&std::env::temp_dir(), 80, 24, || {}).unwrap();
        let prompt = prompted(&terminal);
        assert_eq!(terminal.typed_from(), None, "nothing's typed");
        terminal.input(b"echo half-typed".as_slice());
        shows(&terminal, "echo half-typed", true);
        terminal.input(b" more".as_slice());
        assert_eq!(terminal.typed_from(), Some(prompt), "where the prompt ends, however much is typed");
        terminal.input(CLEAR_LINE.as_bytes());
        assert_eq!(terminal.typed_from(), None, "the line's cleared");
        shows(&terminal, "echo half-typed", false);
        terminal.input(format!("{CLEAR_LINE}echo ran-$((40 + 2))\r").into_bytes());
        shows(&terminal, "ran-42\n", true);
        assert_eq!(terminal.typed_from(), None, "and run");
        assert!(!terminal.text(10_000).contains("half-typed"), "what was typed went before it ran");
    }

    #[test]
    fn runs_what_the_user_types() {
        let dir = std::env::temp_dir();
        let terminal = Terminal::start(&dir, 80, 24, || {}).unwrap();
        terminal.input(b"echo hi-$((40 + 2))\r".as_slice());
        let deadline = Instant::now() + Duration::from_secs(20);
        while !terminal.text(10_000).lines().any(|l| l.trim() == "hi-42") {
            assert!(Instant::now() < deadline, "no output: {}", terminal.text(10_000));
            std::thread::sleep(Duration::from_millis(50));
        }
        // Select the output with the mouse: its row, from the first column to the fifth.
        let screen = terminal.screen();
        let line = |row: usize| screen.cells.iter().filter(|c| c.line == row && c.col < 5).map(|c| c.c).collect::<String>();
        let row = (0..24).find(|&row| line(row) == "hi-42").expect("on screen");
        terminal.select_from(row, 0, false, false);
        terminal.select_to(row, 4);
        assert_eq!(terminal.selected().as_deref(), Some("hi-42"));
        terminal.select_from(row, 1, true, false);
        assert_eq!(terminal.selected().as_deref(), Some("hi-42"), "a double click is the word");
        assert!(!terminal.modes().full_screen);
        terminal.input(b"exit\r".as_slice());
        assert_eq!(terminal.selected(), None, "typing ends the selection");
        while !terminal.exited() {
            assert!(Instant::now() < deadline, "the shell didn't exit");
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}
