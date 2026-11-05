//! The terminal's text: in normal, select or AI mode with the keys on the
//! terminal, its scrollback and screen are a text the editor's own keys go
//! through, as they would a file that can't be edited. Motions, selecting,
//! `y`, `/` and `n` work as in a file, and the selection shows on the
//! terminal; what's printed meanwhile comes in under it, and the selection
//! stays on its chars. `i`, `a` (and the other keys that would type) go into
//! insert mode, which types in the shell: Esc Esc there (or `ctrl-\ ctrl-n`)
//! comes back here, and Esc from here goes back to the code. `gf` or Enter
//! opens the file a line names (`src/main.rs:12:5`), and `space a` asks about
//! what's selected.
//!
//! A story's command waiting in the terminal is reviewed here in AI mode:
//! look through what's on it, and Tab runs it as ever.
//!
//! Reading it again is the whole scrollback, on the UI thread: while the shell
//! prints, that's done at most every [`REREAD`].

use std::path::Path;
use std::time::{Duration, Instant};

use futures::channel::mpsc::UnboundedSender;

use ghostwrite_core::{Buffer, Mode, Range, ViewKind};
use ghostwrite_terminal::{Transcript, located, path_at};
use gpui::{Context, Window};

use super::document::{DocId, Document};
use super::tabs::{Tab, Viewport};
use super::{Autoscroll, Editor};

/// The id the terminal's text goes by: no document has it.
pub(super) const READING: DocId = DocId::MAX;
/// How often the terminal being read is read again while the shell prints.
const REREAD: Duration = Duration::from_millis(100);

pub(super) struct Reading {
    pub(super) tab: Tab,
    transcript: Transcript,
    /// The number of the shell it's of.
    shell: usize,
    /// The story brought the keys here, for its command: when it's over, they go back.
    for_story: bool,
    /// No key has moved it yet: the cursor stays with the shell's.
    with_the_shell: bool,
    /// When it was last read, how many times it has been, and whether it's to be read again
    /// once [`REREAD`] has gone by.
    read_at: Instant,
    pub(super) reads: usize,
    due: bool,
}

/// Where byte `at` is: its line, and char along it.
fn place(buffer: &Buffer, at: usize) -> (usize, usize) {
    let line = buffer.line_of(at);
    (line, buffer.slice(buffer.line_start(line)..at).chars().count())
}

/// The byte of the `ch`th char of `line`, or its end.
fn offset(buffer: &Buffer, (line, ch): (usize, usize)) -> usize {
    let line = line.min(buffer.last_line());
    let start = buffer.line_start(line);
    let text = buffer.line(line);
    let text = text.strip_suffix('\n').unwrap_or(&text);
    start + text.char_indices().nth(ch).map_or(text.len(), |(at, _)| at)
}

/// Where the shell's cursor is in the text.
fn at_the_shells_cursor(buffer: &Buffer, transcript: &Transcript, terminal: &ghostwrite_terminal::Terminal) -> usize {
    let (row, col) = terminal.cursor_cell();
    offset(buffer, transcript.at(row, col))
}

fn cell_of(transcript: &Transcript, (line, ch): (usize, usize)) -> (usize, usize) {
    transcript.cell(line, ch)
}

fn output(text: String) -> Document {
    let mut doc = Document::new(READING, None, text, None);
    doc.output = true;
    doc
}

impl Editor {
    /// Whether the keys are on the terminal's text: on the terminal, out of insert mode.
    pub(super) fn reading(&self) -> bool {
        self.modal.view == ViewKind::Terminal && self.terminal.reading.is_some() && self.modal.mode != Mode::Insert
    }

    /// Whether the terminal has the keys: typing in it, or reading it.
    pub(super) fn in_terminal(&self) -> bool {
        self.modal.view == ViewKind::Terminal
    }

    /// The text the keys are on: the terminal's, while reading it, or the file in front.
    pub(super) fn keyed(&self) -> Option<&Tab> {
        match self.terminal.reading.as_ref().filter(|_| self.reading()) {
            Some(reading) => Some(&reading.tab),
            None => self.tab(),
        }
    }

    pub(super) fn keyed_mut(&mut self) -> Option<&mut Tab> {
        if self.reading() {
            return self.terminal.reading.as_mut().map(|reading| &mut reading.tab);
        }
        self.tab_mut()
    }

    /// A tab by its id, the terminal's text included.
    pub(super) fn keyed_by_id_mut(&mut self, id: DocId) -> Option<&mut Tab> {
        if id == READING {
            return self.terminal.reading.as_mut().map(|reading| &mut reading.tab);
        }
        self.project.tabs.by_id_mut(id)
    }

    /// The keys on the terminal in front, out of insert mode, on its text from the
    /// shell's cursor. `for_story`: the story's bringing the keys here, and will take them back.
    pub(super) fn start_reading(&mut self, for_story: bool, cx: &mut Context<Self>) {
        let for_story = for_story && self.terminal.reading.as_ref().is_none_or(|reading| reading.for_story);
        if self.terminal.shells.is_empty() {
            return self.focus_on(ViewKind::Code, cx);
        }
        self.terminal.shown = true;
        self.focus_on(ViewKind::Terminal, cx);
        if self.modal.mode == Mode::Insert {
            self.let_go_of_held_keys();
            self.modal.leave(Mode::Insert);
        }
        self.stop_reading();
        let Some(shell) = self.terminal.shells.get(self.terminal.active) else { return };
        let transcript = shell.terminal.transcript();
        self.find.forget();
        let doc = output(transcript.text.clone());
        let at = at_the_shells_cursor(&doc.buffer, &transcript, &shell.terminal);
        let view = Viewport { selection: Range::point(at), ..Viewport::default() };
        let tab = Tab { doc, view };
        let reading = Reading { tab, transcript, shell: shell.number, for_story, with_the_shell: true, read_at: Instant::now(), reads: 1, due: false };
        self.terminal.reading = Some(reading);
        self.terminal.shown = true;
        self.show_reading(true);
        cx.notify();
    }

    /// The terminal's text is let go of (the keys are going elsewhere, or typing in
    /// its shell): nothing's marked on it.
    pub(super) fn stop_reading(&mut self) {
        let Some(reading) = self.terminal.reading.take() else { return };
        if let Some(shell) = self.terminal.shells.iter().find(|shell| shell.number == reading.shell) {
            shell.terminal.mark(None);
        }
    }

    /// The story's over: if it brought the keys here, they go back.
    pub(super) fn story_left_the_terminal(&mut self, cx: &mut Context<Self>) {
        if self.terminal.reading.as_ref().is_some_and(|reading| reading.for_story) {
            self.focus_on(ViewKind::Code, cx);
        }
    }

    /// Another shell is in front: it's the one read.
    pub(super) fn read_the_shell_in_front(&mut self, cx: &mut Context<Self>) {
        let front = self.terminal.shells.get(self.terminal.active).map(|shell| shell.number);
        if let Some(reading) = self.terminal.reading.as_ref()
            && Some(reading.shell) != front
        {
            let for_story = reading.for_story;
            self.start_reading(for_story, cx);
        }
    }

    /// Shell `number` printed something: if it's being read, it's read again, unless it
    /// was a moment ago; then once [`REREAD`] has gone by, when `poke` sends it round again.
    pub(super) fn printed(&mut self, number: usize, poke: &UnboundedSender<()>) {
        let Some(reading) = self.terminal.reading.as_mut().filter(|reading| reading.shell == number) else { return };
        if reading.read_at.elapsed() >= REREAD {
            return self.read_again(number);
        }
        if !std::mem::replace(&mut reading.due, true) {
            let poke = poke.clone();
            std::thread::spawn(move || {
                std::thread::sleep(REREAD);
                poke.unbounded_send(()).ok();
            });
        }
    }

    /// Shell `number` printed something: if it's being read, the text is read
    /// again, the selection moving with its chars, and the view staying put.
    pub(super) fn read_again(&mut self, number: usize) {
        let Some(shell) = self.terminal.shells.iter().find(|shell| shell.number == number) else { return };
        let Some(reading) = self.terminal.reading.as_mut().filter(|reading| reading.shell == number) else { return };
        (reading.read_at, reading.reads, reading.due) = (Instant::now(), reading.reads + 1, false);
        let transcript = shell.terminal.transcript();
        if *reading.tab.doc.buffer.rope() != transcript.text {
            reading.tab.reload(transcript.text.clone());
        }
        if reading.with_the_shell {
            let at = at_the_shells_cursor(&reading.tab.doc.buffer, &transcript, &shell.terminal);
            reading.tab.view.selection = Range::point(at);
        }
        reading.transcript = transcript;
        let scroll = reading.with_the_shell;
        self.show_reading(scroll);
    }

    /// Mark the selection on the terminal, and with `scroll`, bring its cursor into view.
    fn show_reading(&self, scroll: bool) {
        let Some(reading) = self.terminal.reading.as_ref() else { return };
        let Some(shell) = self.terminal.shells.iter().find(|shell| shell.number == reading.shell) else { return };
        let (buffer, text) = (&reading.tab.doc.buffer, &reading.tab.doc.buffer);
        let selection = reading.tab.view.selection.min_width_1(text);
        let cell = |at: usize| cell_of(&reading.transcript, place(buffer, at));
        let (from, to) = (cell(selection.from()), cell(buffer.prev(selection.to())));
        shell.terminal.mark(Some((from, to)));
        if scroll {
            shell.terminal.show(cell(reading.tab.cursor()).0);
        }
    }

    /// Where the reading cursor is on the terminal's screen (row from the top of the view, column).
    pub(super) fn reading_cursor(&self) -> Option<(usize, usize)> {
        let reading = self.terminal.reading.as_ref().filter(|_| self.reading())?;
        let shell = self.terminal.shells.iter().find(|shell| shell.number == reading.shell)?;
        let (row, col) = cell_of(&reading.transcript, place(&reading.tab.doc.buffer, reading.tab.cursor()));
        Some((shell.terminal.in_view(row)?, col))
    }

    /// Rows on the terminal's screen, for paging through it.
    pub(super) fn reading_rows(&self) -> usize {
        self.terminal.reading.as_ref().map_or(2, |reading| reading.transcript.rows.max(2))
    }

    /// After a key the editor's modes took while reading: an edit is put back,
    /// and the selection shows.
    pub(super) fn read_on(&mut self, edited: bool) {
        if let Some(reading) = self.terminal.reading.as_mut() {
            if edited {
                reading.tab.reload(reading.transcript.text.clone());
                self.project.say("this is the terminal's output: i types on its line");
            }
            reading.tab.view.autoscroll = None;
            reading.with_the_shell = false;
        }
        self.show_reading(true);
    }

    /// `space a` while reading: ask about what's selected on the terminal (or the cursor's line).
    pub(super) fn ask_about_output(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.keyed() else { return };
        let (text, selection) = (&tab.doc.buffer, tab.view.selection);
        let quoted = if selection.is_wide(text) {
            text.slice(selection.bytes()).into_owned()
        } else {
            let line = tab.doc.buffer.line_of(tab.cursor());
            text.slice(tab.doc.buffer.lines_range(line..line + 1)).trim_end().to_string()
        };
        self.open_story_about(Some(quoted), "", window, cx);
    }

    /// `gf` (or Enter, reading the terminal): open the file named where the
    /// cursor is, or what's selected, at the line and column given after it
    /// (`src/main.rs:12:5`), as found from the project, or the file it's in.
    pub(super) fn goto_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let _ = window;
        let Some(tab) = self.keyed() else { return };
        let (text, selection) = (&tab.doc.buffer, tab.view.selection);
        let near = tab.doc.path.as_deref().and_then(Path::parent).map(Path::to_path_buf);
        // What's selected, if it's a file; or what's under the cursor (a path is on one line).
        let selected = selection.is_wide(text).then(|| text.slice(selection.bytes()).trim().to_string());
        let (cursor, line) = (tab.cursor(), text.line_of(tab.cursor()));
        let under = path_at(&text.line(line), cursor - text.line_start(line));
        let found = selected.iter().chain([&under]).find_map(|named| located(named, &self.project.root, near.as_deref()));
        let named = selected.unwrap_or(under);
        let Some((path, line, column)) = found else {
            return self.project.say(if named.is_empty() { "no file named here".to_string() } else { format!("no file {named}") });
        };
        let from = self.before_jump();
        self.focus_on(ViewKind::Code, cx);
        self.open_at(&path, line, column, cx);
        self.note_jump(from);
    }

    /// `path` in its tab, at `line` and `column` (from zero), the keys on the code.
    pub(super) fn open_at(&mut self, path: &Path, line: usize, column: usize, cx: &mut Context<Self>) {
        let Some(ix) = self.project.open(path) else { return self.project.say(format!("couldn't open {}", path.display())) };
        self.activate(ix, cx);
        if let Some(tab) = self.tab_mut() {
            let at = tab.doc.buffer.at_line_col(line, column);
            tab.select(Range::point(at), Autoscroll::Center(line..line + 1));
        }
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_place_is_a_line_and_char() {
        let buffer = Buffer::new("ab\nçd\n");
        assert_eq!(place(&buffer, 5), (1, 1));
        assert_eq!(offset(&buffer, (1, 1)), 5);
        assert_eq!(offset(&buffer, (1, 9)), 6, "past the end: the line's end");
    }
}
