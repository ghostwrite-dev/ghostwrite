//! The terminal's screen ([`super::terminal`]), drawn a run of same-styled
//! text at a time, each at its column so the grid stays straight; the
//! shell is fitted to it. Its colors are the theme's. A command the story
//! proposes is drawn over it, the screen moving out of the way if it has to.

use ghostwrite_terminal::{Cell, Color, CursorShape};
use gpui::{
    AnyElement, App, AvailableSpace, Bounds, ContentMask, Element, ElementId, Entity, FontStyle, FontWeight, GlobalElementId, Hsla, IntoElement, LayoutId,
    PaintQuad, Pixels, Rgba, ShapedLine, SharedString, StrikethroughStyle, Style, TextRun, UnderlineStyle, Window, div, fill, outline, point, prelude::*, px,
    relative, size,
};

use super::Editor;
use super::element::{Block, BlockSizes, Metrics};
use crate::theme::{Theme, theme};

type Point = gpui::Point<Pixels>;

pub(super) struct TerminalView {
    editor: Entity<Editor>,
    /// Typing in it: the cursor is drawn solid.
    active: bool,
}

/// Text at a cell, in one style, and what's behind it.
struct Run {
    line: usize,
    col: usize,
    text: String,
    color: Hsla,
    background: Option<Hsla>,
    weight: FontWeight,
    italic: bool,
    underline: bool,
    strike: bool,
}

impl TerminalView {
    pub(super) fn new(editor: Entity<Editor>, active: bool) -> Self {
        Self { editor, active }
    }
}

/// The screen's cells as runs of same-styled text, in the theme's colors.
fn runs(cells: &[Cell], theme: &Theme) -> Vec<Run> {
    let mut runs: Vec<Run> = vec![];
    let mut next_col = None;
    for cell in cells {
        let (mut fg, mut bg) = (color(cell.fg, theme), (cell.bg != Color::Background).then(|| color(cell.bg, theme)));
        if cell.inverse {
            (fg, bg) = (bg.unwrap_or(theme.background), Some(fg));
        }
        if cell.dim {
            fg = fg.opacity(0.7);
        }
        if cell.hidden {
            fg = fg.opacity(0.);
        }
        if cell.selected {
            bg = Some(theme.selection);
        }
        let run = Run {
            line: cell.line,
            col: cell.col,
            text: cell.c.to_string(),
            color: fg,
            background: bg,
            weight: if cell.bold { FontWeight::BOLD } else { FontWeight::NORMAL },
            italic: cell.italic,
            underline: cell.underline,
            strike: cell.strike,
        };
        let blank = cell.c == ' ' && run.background.is_none() && !run.underline;
        let joins = next_col == Some((cell.line, cell.col)) && !blank && runs.last().is_some_and(|last| last.styled_like(&run));
        match runs.last_mut() {
            Some(last) if joins => last.text.push(cell.c),
            _ if !blank => runs.push(run),
            _ => {}
        }
        // A wide char stands alone, so the next cell starts where the grid says.
        next_col = (!blank && !cell.wide).then_some((cell.line, cell.col + 1));
    }
    runs
}

impl Run {
    fn styled_like(&self, other: &Run) -> bool {
        (self.color, self.background, self.weight, self.italic, self.underline, self.strike)
            == (other.color, other.background, other.weight, other.italic, other.underline, other.strike)
    }
}

/// The rows `command` takes, typed at column `col` of `cols`: each's column and text.
fn command_rows(command: &str, col: usize, cols: usize) -> Vec<(usize, String)> {
    let cols = cols.max(1);
    let mut rows = vec![];
    let mut start = col.min(cols);
    for line in command.split('\n') {
        let mut rest: Vec<char> = line.chars().collect();
        loop {
            let taken = (cols - start).min(rest.len());
            rows.push((start, rest.drain(..taken).collect()));
            start = 0;
            if rest.is_empty() {
                break;
            }
        }
    }
    rows
}

/// How many rows the screen moves down (or up, if negative) for what goes around
/// the prompt on `line` to fit: a note of `above` rows, and `below` rows from it on,
/// on a screen of `rows`. If it can't all fit, the prompt and what's below it do.
fn screen_shift(line: usize, above: usize, below: usize, rows: usize) -> i32 {
    let (line, above, below, rows) = (line as i32, above as i32, below as i32, rows as i32);
    let most = rows - line - below;
    if most < 0 { most } else { (above - line).max(0).min(most) }
}

/// `block`, built and laid out for the terminal: covering what's under it, and
/// keeping its clicks from the screen.
fn lay_out(block: Block, name: &'static str, sizes: BlockSizes, theme: &Theme, window: &mut Window, cx: &mut App) -> (AnyElement, Pixels) {
    let mut element = div()
        .block_mouse_except_scroll()
        .bg(theme.background)
        .debug_selector(move || name.to_string())
        .child((block.build)(sizes, window, cx))
        .into_any_element();
    let height = element.layout_as_root(size(AvailableSpace::Definite(sizes.width), AvailableSpace::MinContent), window, cx).height;
    (element, height)
}

/// A terminal color in the theme: its sixteen, or exact.
fn color(color: Color, theme: &Theme) -> Hsla {
    match color {
        Color::Foreground => theme.foreground,
        Color::Background => theme.background,
        Color::Cursor => theme.cursor_normal,
        Color::DimForeground => theme.foreground.opacity(0.7),
        Color::Ansi(i) => theme.ansi[i as usize % 16],
        Color::DimAnsi(i) => theme.ansi[i as usize % 16].opacity(0.7),
        Color::Rgb(r, g, b) => Rgba { r: r as f32 / 255., g: g as f32 / 255., b: b as f32 / 255., a: 1. }.into(),
    }
}

pub struct Prepainted {
    texts: Vec<(Point, ShapedLine)>,
    quads: Vec<PaintQuad>,
    cursor: Option<PaintQuad>,
    cursor_glyph: Option<(Point, ShapedLine)>,
    /// What goes around a proposed command.
    blocks: Vec<AnyElement>,
    line_height: Pixels,
}

impl IntoElement for TerminalView {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for TerminalView {
    type RequestLayoutState = ();
    type PrepaintState = Prepainted;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(&mut self, _: Option<&GlobalElementId>, _: Option<&gpui::InspectorElementId>, window: &mut Window, cx: &mut App) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> Prepainted {
        let theme = theme(cx).clone();
        let metrics = Metrics::measure(&theme, window);
        let (cw, lh) = (metrics.char_width, metrics.line_height);
        let cols = (f32::from(bounds.size.width) / f32::from(cw)).floor().max(1.) as u16;
        let rows = (f32::from(bounds.size.height) / f32::from(lh)).floor().max(1.) as u16;
        let screen = self.editor.update(cx, |editor, _| {
            let terminal = editor.terminal.current()?;
            terminal.resize(cols, rows, f32::from(cw) as u16, f32::from(lh) as u16);
            Some((terminal.screen(), terminal.typed_from()))
        });
        let mut state = Prepainted { texts: vec![], quads: vec![], cursor: None, cursor_glyph: None, blocks: vec![], line_height: lh };
        let Some((mut screen, typed_from)) = screen else { return state };

        // A command proposed: where the shell's line starts, in place of what's
        // typed on it (which running it clears), or at the cursor (scrolled away
        // from it, at the bottom).
        let editor = self.editor.read(cx);
        let proposed = editor.command_proposed().then(|| editor.story.as_ref()?.proposed_command(editor.ask_field(), &theme)).flatten();
        let cursor_at = screen.cursor.as_ref().map(|cursor| (cursor.line, cursor.col));
        let prompt = typed_from.or(cursor_at).unwrap_or((rows as usize - 1, 0));
        let sizes = BlockSizes { line_height: lh, char_width: cw, width: bounds.size.width };
        let mut around = None;
        if let Some(proposed) = proposed {
            let typed = command_rows(&proposed.command, prompt.1, cols as usize);
            let note = proposed.note.map(|note| lay_out(note, "terminal-note", sizes, &theme, window, cx));
            let keys = lay_out(proposed.keys, "terminal-keys", sizes, &theme, window, cx);
            around = Some((typed, note, keys));
        }
        let rows_of = |height: Pixels| (f32::from(height) / f32::from(lh)).ceil() as usize;
        let shift = around.as_ref().map_or(0, |(typed, note, keys)| {
            let above = note.as_ref().map_or(0, |(_, height)| rows_of(*height));
            screen_shift(prompt.0, above, typed.len() + rows_of(keys.1), rows as usize)
        });
        let origin = point(bounds.left(), bounds.top() + lh * shift as f32);
        self.editor.update(cx, |editor, _| editor.terminal.cells = Some((origin, cw, lh)));
        let at = |line: usize, col: usize| point(origin.x + cw * col as f32, origin.y + lh * line as f32);

        if let Some((typed, note, keys)) = around {
            // What's on the screen where the command would go isn't shown under it,
            // and neither is what the user typed: the command takes its place.
            let typed_to = cursor_at.map_or(prompt.0, |(line, _)| line.max(prompt.0));
            screen.cells.retain(|cell| {
                let typed_over = (cell.line, cell.col) >= prompt && cell.line <= typed_to;
                let under =
                    cell.line >= prompt.0 && typed.get(cell.line - prompt.0).is_some_and(|(col, text)| cell.col >= *col && !(text.is_empty() && *col == 0));
                !typed_over && !under
            });
            if let Some(cursor) = screen.cursor.as_mut() {
                (cursor.line, cursor.col, cursor.c) = (prompt.0, prompt.1, ' ');
            }
            for (ix, (col, text)) in typed.iter().enumerate().filter(|(_, (_, text))| !text.is_empty()) {
                let run = metrics.run(text.len(), theme.muted_foreground);
                state.texts.push((at(prompt.0 + ix, *col), window.text_system().shape_line(text.clone().into(), metrics.size, &[run], None)));
            }
            let top = at(prompt.0, 0).y;
            let mut from = top;
            if let Some((mut note, height)) = note {
                note.prepaint_at(point(bounds.left(), top - height), window, cx);
                state.blocks.push(note);
                from = top - height;
            }
            let (mut keys, keys_height) = keys;
            keys.prepaint_at(point(bounds.left(), top + lh * typed.len() as f32), window, cx);
            state.blocks.push(keys);
            // Where the story's step is: its note, the command and its keys.
            let to = top + lh * typed.len() as f32 + keys_height;
            let rect = Bounds::new(point(bounds.left(), from), size(bounds.size.width, to - from));
            self.editor.update(cx, |editor, _| {
                if let Some(mark) = editor.step_mark() {
                    editor.seen(super::attention::Seen { mark, rect, scrolled: None }, window);
                }
            });
        }

        // Reading it: the cursor is the reading one, a block, where the editor's keys are.
        let editor = self.editor.read(cx);
        if editor.reading() {
            screen.cursor = editor.reading_cursor().map(|(line, col)| {
                let c = screen.cells.iter().find(|cell| (cell.line, cell.col) == (line, col)).map_or(' ', |cell| cell.c);
                ghostwrite_terminal::Cursor { line, col, shape: CursorShape::Block, c }
            });
        }

        // A link under the mouse while ⌘ is held: underlined, to be clicked.
        if let Some(link) = &editor.terminal.link {
            screen.cells.iter_mut().filter(|cell| cell.line == link.row && link.cols.contains(&cell.col)).for_each(|cell| cell.underline = true);
        }
        for run in &runs(&screen.cells, &theme) {
            let origin = at(run.line, run.col);
            if let Some(background) = run.background {
                let cells = run.text.chars().count().max(1);
                state.quads.push(fill(Bounds::new(origin, size(cw * cells as f32, lh)), background));
            }
            let mut font = metrics.font.clone();
            font.weight = run.weight;
            if run.italic {
                font.style = FontStyle::Italic;
            }
            let text_run = TextRun {
                len: run.text.len(),
                font,
                color: run.color,
                background_color: None,
                underline: run.underline.then(|| UnderlineStyle { thickness: px(1.), color: Some(run.color), wavy: false }),
                strikethrough: run.strike.then(|| StrikethroughStyle { thickness: px(1.), color: Some(run.color) }),
            };
            let shaped = window.text_system().shape_line(SharedString::from(run.text.clone()), metrics.size, &[text_run], None);
            state.texts.push((origin, shaped));
        }
        if let Some(cursor) = screen.cursor {
            let (origin, shape, c) = (at(cursor.line, cursor.col), cursor.shape, cursor.c.to_string());
            let cell = Bounds::new(origin, size(cw, lh));
            let color = theme.cursor_normal;
            match shape {
                _ if !self.active => state.cursor = Some(outline(cell, color, gpui::BorderStyle::Solid)),
                CursorShape::Beam => state.cursor = Some(fill(Bounds::new(origin, size(px(2.) * theme.zoom, lh)), color)),
                CursorShape::Underline => {
                    let thick = px(2.) * theme.zoom;
                    state.cursor = Some(fill(Bounds::new(point(origin.x, origin.y + lh - thick), size(cw, thick)), color))
                }
                CursorShape::HollowBlock => state.cursor = Some(outline(cell, color, gpui::BorderStyle::Solid)),
                _ => {
                    state.cursor = Some(fill(cell, color));
                    let run = metrics.run(c.len(), theme.cursor_text);
                    state.cursor_glyph = Some((origin, window.text_system().shape_line(c.into(), metrics.size, &[run], None)));
                }
            }
        }
        state
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        state: &mut Prepainted,
        window: &mut Window,
        cx: &mut App,
    ) {
        let line_height = state.line_height;
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            for quad in state.quads.drain(..) {
                window.paint_quad(quad);
            }
            for (at, text) in &state.texts {
                text.paint(*at, line_height, window, cx).ok();
            }
            if let Some(cursor) = state.cursor.take() {
                window.paint_quad(cursor);
            }
            if let Some((at, glyph)) = &state.cursor_glyph {
                glyph.paint(*at, line_height, window, cx).ok();
            }
            for block in &mut state.blocks {
                block.paint(window, cx);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_command_wraps_as_the_terminal_would_wrap_it() {
        let rows = command_rows;
        assert_eq!(rows("ls", 2, 10), [(2, "ls".to_string())]);
        assert_eq!(rows("abcdef", 2, 5), [(2, "abc".to_string()), (0, "def".to_string())]);
        assert_eq!(rows("ab\ncd", 2, 10), [(2, "ab".to_string()), (0, "cd".to_string())]);
        assert_eq!(rows("ab", 5, 5), [(5, String::new()), (0, "ab".to_string())], "the prompt fills its row");
    }

    #[test]
    fn the_screen_moves_only_for_what_doesnt_fit() {
        assert_eq!(screen_shift(10, 2, 2, 24), 0, "it fits");
        assert_eq!(screen_shift(0, 2, 2, 24), 2, "a fresh shell makes room for the note");
        assert_eq!(screen_shift(23, 2, 2, 24), -1, "at the bottom, room for the keys");
        assert_eq!(screen_shift(0, 5, 2, 4), 2, "a small pane: as much of the note as there's room for");
        assert_eq!(screen_shift(3, 5, 3, 4), -2, "the prompt and the keys come first");
    }
}
