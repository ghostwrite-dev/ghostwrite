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

mod screen;

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
}

/// Lines kept above the screen.
const SCROLLBACK: usize = 10_000;

pub struct Terminal {
    term: Arc<FairMutex<Term<Events>>>,
    events: Events,
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

/// What clears what's been typed on the shell's line: Ctrl-U, or Esc for PowerShell.
pub const CLEAR_LINE: &str = if cfg!(windows) { "\x1b" } else { "\x15" };

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
        let events = Events { sender: Arc::default(), size: Arc::new(Mutex::new(size)), exited: Arc::default(), on_change: Arc::new(on_change) };
        let config = Config { scrolling_history: SCROLLBACK, ..Config::default() };
        let term = Arc::new(FairMutex::new(Term::new(config, &Size::from(size), events.clone())));
        let event_loop = EventLoop::new(term.clone(), events.clone(), pty, false, false).map_err(|e| format!("couldn't start a shell: {e}"))?;
        events.sender.set(event_loop.channel()).ok();
        event_loop.spawn();
        Ok(Self { term, events })
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
        }
    }

    /// What the user typed: to the shell, with the view back at the bottom
    /// (and nothing selected).
    pub fn input(&self, bytes: impl Into<Cow<'static, [u8]>>) {
        let mut term = self.term.lock();
        term.scroll_display(Scroll::Bottom);
        term.selection = None;
        drop(term);
        self.events.write(bytes);
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

/// Every line, oldest first: lines the screen wrapped are joined again,
/// and blank lines at the end are left off.
fn transcript<T>(term: &Term<T>) -> Vec<String> {
    let grid = term.grid();
    let mut lines: Vec<String> = vec![];
    let mut joining = false;
    for line in grid.topmost_line().0..=grid.bottommost_line().0 {
        let row = &grid[Line(line)];
        let mut text = String::new();
        for col in 0..grid.columns() {
            let cell = &row[Column(col)];
            if !cell.flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
                text.push(cell.c);
            }
        }
        let wraps = row[Column(grid.columns() - 1)].flags.contains(Flags::WRAPLINE);
        if !wraps {
            text.truncate(text.trim_end().len());
        }
        match lines.last_mut() {
            Some(last) if joining => last.push_str(&text),
            _ => lines.push(text),
        }
        joining = wraps;
    }
    while lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    lines
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use alacritty_terminal::event::VoidListener;
    use alacritty_terminal::vte::ansi::Processor;

    use super::*;

    #[test]
    fn reads_back_the_scrollback_and_screen() {
        let mut term = Term::new(Config::default(), &Size { cols: 10, lines: 3 }, VoidListener);
        let mut parser: Processor = Processor::new();
        parser.advance(&mut term, b"one\r\ntwo\r\n\x1b[31mred\x1b[0m\r\nabcdefghijklmno\r\nlast\r\n");
        assert_eq!(transcript(&term), ["one", "two", "red", "abcdefghijklmno", "last"]);
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
