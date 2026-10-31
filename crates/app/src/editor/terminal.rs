//! The terminal under the code: the user's own shell. Terminal mode (`space t`,
//! `:term` or `cmd-j`) gives it the keyboard; Esc Esc goes back to the mode
//! it was entered from with the pane still showing (not while a full-screen
//! program like vim has it: Esc is its; a click on the code leaves then),
//! and `cmd-j` hides it. Dragging the mouse selects (a double click a word,
//! a triple click a line), and `cmd-c` copies it.
//! Claude is shown what's on it with every request (and, in a follow-up,
//! again when it's changed), but nothing it says reaches it. The shell
//! keeps running while it's hidden; when it exits, the pane closes. Its
//! top edge drags to resize it, and the height is kept for next time.

use futures::StreamExt as _;
use ghostwrite_core::Mode;
use ghostwrite_project::store;
use ghostwrite_project::terminal::Terminal;
use ghostwrite_project::terminal::alacritty_terminal::event::EventListener;
use ghostwrite_project::terminal::alacritty_terminal::grid::Dimensions as _;
use ghostwrite_project::terminal::alacritty_terminal::term::cell::Flags;
use ghostwrite_project::terminal::alacritty_terminal::term::color::Colors;
use ghostwrite_project::terminal::alacritty_terminal::term::{Term, TermMode};
use ghostwrite_project::terminal::alacritty_terminal::vte::ansi::{Color, CursorShape, NamedColor, Rgb};
use gpui::{
    AnyElement, App, Bounds, ContentMask, Context, Element, ElementId, Entity, FontStyle, FontWeight, GlobalElementId, Hsla, IntoElement, Keystroke, LayoutId,
    Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, PaintQuad, Pixels, Rgba, ScrollWheelEvent, ShapedLine, SharedString, StrikethroughStyle, Style,
    Task, TextRun, UnderlineStyle, Window, div, fill, outline, point, prelude::*, px, relative, size,
};

use super::Editor;
use super::element::Metrics;
use crate::theme::{Theme, pt, theme};

/// Bytes of the terminal Claude is told: the end.
const MAX_CONTEXT: usize = 8_000;
/// The pane's height to start with, and how small and how close to the
/// top of the window it can be dragged (in points at 100%, like [`pt`]).
const HEIGHT: f32 = 290.;
const MIN_HEIGHT: f32 = 60.;
const ROOM_ABOVE: f32 = 120.;

pub(super) struct TerminalPane {
    pub(super) shown: bool,
    height: f32,
    /// While its edge is dragged: where the pointer and the height started.
    pub(super) resizing: Option<(Pixels, f32)>,
    terminal: Option<Terminal>,
    /// The last key was an Esc (the shell got it): another leaves terminal mode.
    escaped: bool,
    /// Scrolling that hasn't added up to a line yet.
    scrolled: f32,
    /// Where its cells are on screen, as last drawn: the top left one, and a cell's size.
    cells: Option<(Point, Pixels, Pixels)>,
    /// The mouse is selecting.
    selecting: bool,
    _task: Option<Task<()>>,
}

impl Default for TerminalPane {
    fn default() -> Self {
        let height = store::terminal_height().unwrap_or(HEIGHT);
        Self { shown: false, height, resizing: None, terminal: None, escaped: false, scrolled: 0., cells: None, selecting: false, _task: None }
    }
}

impl Editor {
    /// While dragging the terminal's top edge.
    pub(super) fn drag_terminal_edge(&mut self, event: &MouseMoveEvent, window: &Window, cx: &mut Context<Self>) {
        let Some((from, height)) = self.terminal.resizing else { return };
        let zoom = theme(cx).zoom;
        let most = (f32::from(window.viewport_size().height) / zoom - ROOM_ABOVE).max(MIN_HEIGHT);
        self.terminal.height = (height + f32::from(from - event.position.y) / zoom).clamp(MIN_HEIGHT, most);
        cx.notify();
    }

    pub(super) fn end_terminal_drag(&mut self) {
        if self.terminal.resizing.take().is_some() {
            store::save_terminal_height(self.terminal.height);
        }
    }

    /// `space t`, `:term`, `cmd-j`, a click on the pane: into terminal mode,
    /// starting a shell if there's none.
    pub(super) fn enter_terminal(&mut self, cx: &mut Context<Self>) {
        if self.terminal.terminal.is_none()
            && let Err(e) = self.start_terminal(cx)
        {
            self.project.say(e);
            return cx.notify();
        }
        self.leave_insert();
        self.stop_selecting();
        self.terminal.shown = true;
        self.terminal.escaped = false;
        self.modal.enter(Mode::Terminal);
        cx.notify();
    }

    /// `cmd-j`: into terminal mode, or from it, hide the terminal.
    pub(super) fn toggle_terminal(&mut self, cx: &mut Context<Self>) {
        if self.modal.mode != Mode::Terminal {
            return self.enter_terminal(cx);
        }
        self.terminal.shown = false;
        self.modal.leave(Mode::Terminal);
        cx.notify();
    }

    fn start_terminal(&mut self, cx: &mut Context<Self>) -> Result<(), String> {
        let (tx, mut rx) = futures::channel::mpsc::unbounded();
        // Sized for real when it's first drawn.
        let terminal = Terminal::start(&self.project.root, 80, 24, move || drop(tx.unbounded_send(())))?;
        self.terminal.terminal = Some(terminal);
        self.terminal._task = Some(cx.spawn(async move |this, cx| {
            while rx.next().await.is_some() {
                // One redraw for everything that's come in.
                while rx.try_recv().is_ok() {}
                let alive = this.update(cx, |this, cx| this.terminal_changed(cx)).unwrap_or(false);
                if !alive {
                    break;
                }
            }
        }));
        Ok(())
    }

    /// There's something new on the terminal, or its shell exited (then
    /// the pane closes, and `false`: there's nothing more to come).
    fn terminal_changed(&mut self, cx: &mut Context<Self>) -> bool {
        // Hidden, there's nothing to draw (Claude reads it when it asks).
        if self.terminal.shown {
            cx.notify();
        }
        if !self.terminal.terminal.as_ref().is_some_and(Terminal::exited) {
            return true;
        }
        self.terminal.terminal = None;
        self.terminal.shown = false;
        self.modal.leave(Mode::Terminal);
        self.project.say("the terminal's shell exited");
        false
    }

    /// What Claude is told: the end of what's on the terminal, if there's a shell.
    pub(super) fn terminal_context(&self) -> Option<String> {
        self.terminal.terminal.as_ref().map(|t| t.text(MAX_CONTEXT)).filter(|text| !text.trim().is_empty())
    }

    /// A key in terminal mode: to the shell, unless it's the app's (cmd)
    /// or a second Esc in a row, which leaves.
    pub(super) fn terminal_key(&mut self, keystroke: &Keystroke) -> bool {
        let Some(terminal) = &self.terminal.terminal else { return false };
        // In a full-screen program Esc is the program's, however often.
        let escape = keystroke.key == "escape" && keystroke.modifiers == Modifiers::none() && !terminal.full_screen();
        if escape && self.terminal.escaped {
            self.terminal.escaped = false;
            self.modal.leave(Mode::Terminal);
            return true;
        }
        let app_cursor = terminal.with(|t| t.mode().contains(TermMode::APP_CURSOR));
        let Some(bytes) = key_bytes(keystroke, app_cursor) else { return false };
        terminal.input(bytes);
        self.terminal.escaped = escape;
        true
    }

    /// `cmd-c` in terminal mode: what's selected.
    pub(super) fn terminal_copy(&mut self, cx: &mut Context<Self>) {
        if let Some(text) = self.terminal.terminal.as_ref().and_then(Terminal::selected) {
            cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
        }
    }

    /// The cell under `position`: its row from the top of the view, and column.
    fn terminal_cell(&self, position: Point) -> Option<(usize, usize)> {
        let (origin, cw, lh) = self.terminal.cells?;
        let (x, y) = (f32::from(position.x - origin.x).max(0.), f32::from(position.y - origin.y).max(0.));
        Some(((y / f32::from(lh)) as usize, (x / f32::from(cw)).round() as usize))
    }

    fn terminal_mouse_down(&mut self, event: &MouseDownEvent, cx: &mut Context<Self>) {
        self.enter_terminal(cx);
        let Some((row, col)) = self.terminal_cell(event.position) else { return };
        if let Some(terminal) = &self.terminal.terminal {
            terminal.select_from(row, col, event.click_count == 2, event.click_count >= 3);
            self.terminal.selecting = true;
            cx.notify();
        }
    }

    fn terminal_mouse_move(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        if !self.terminal.selecting {
            return;
        }
        if event.pressed_button != Some(MouseButton::Left) {
            self.terminal.selecting = false;
            return;
        }
        if let (Some((row, col)), Some(terminal)) = (self.terminal_cell(event.position), &self.terminal.terminal) {
            terminal.select_to(row, col);
            cx.notify();
        }
    }

    /// `cmd-v` in terminal mode.
    pub(super) fn terminal_paste(&mut self, cx: &mut Context<Self>) {
        let Some(terminal) = &self.terminal.terminal else { return };
        let Some(text) = cx.read_from_clipboard().and_then(|c| c.text()) else { return };
        let bracketed = terminal.with(|t| t.mode().contains(TermMode::BRACKETED_PASTE));
        let text = if bracketed { format!("\x1b[200~{}\x1b[201~", text.replace('\x1b', "")) } else { text.replace("\r\n", "\r").replace('\n', "\r") };
        terminal.input(text.into_bytes());
        self.terminal.escaped = false;
    }

    /// The wheel: back through the scrollback, or (in a full-screen program
    /// that asks for it) arrow keys.
    fn terminal_scroll(&mut self, event: &ScrollWheelEvent, window: &Window, cx: &mut Context<Self>) {
        let Some(terminal) = &self.terminal.terminal else { return };
        let line_height = Metrics::measure(theme(cx), window).line_height;
        self.terminal.scrolled += f32::from(event.delta.pixel_delta(line_height).y) / f32::from(line_height);
        let lines = self.terminal.scrolled.trunc() as i32;
        if lines == 0 {
            return;
        }
        self.terminal.scrolled -= lines as f32;
        let (mode, app_cursor) = terminal.with(|t| (*t.mode(), t.mode().contains(TermMode::APP_CURSOR)));
        if mode.contains(TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL) {
            let key = if lines > 0 { 'A' } else { 'B' };
            let arrow = if app_cursor { format!("\x1bO{key}") } else { format!("\x1b[{key}") };
            terminal.input(arrow.repeat(lines.unsigned_abs() as usize).into_bytes());
        } else {
            terminal.scroll(lines);
        }
        cx.notify();
    }

    pub(super) fn render_terminal(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.terminal.shown {
            return None;
        }
        let theme = theme(cx);
        let active = self.modal.mode == Mode::Terminal;
        Some(
            div()
                .id("terminal")
                .relative()
                .flex_none()
                .h(pt(self.terminal.height))
                .px_2()
                .py_1()
                .border_t_1()
                .border_color(if active { theme.success } else { theme.border })
                .bg(theme.background)
                .on_mouse_down(MouseButton::Left, cx.listener(|this, event: &MouseDownEvent, _, cx| this.terminal_mouse_down(event, cx)))
                .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| this.terminal_mouse_move(event, cx)))
                .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, window, cx| this.terminal_scroll(event, window, cx)))
                .child(TerminalView { editor: cx.entity(), active })
                // The top edge drags to resize.
                .child(div().id("terminal-edge").absolute().left_0().top(pt(-3.)).w_full().h(pt(6.)).cursor_row_resize().on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, event: &MouseDownEvent, _, cx| {
                        this.terminal.resizing = Some((event.position.y, this.terminal.height));
                        cx.stop_propagation();
                    }),
                ))
                .into_any_element(),
        )
    }
}

/// What a key sends to the shell, as xterm sends it; `None` for keys that
/// aren't the shell's (anything with cmd). Option types what the layout
/// says it types (`|`, `{` …), except with the keys that move and delete.
pub(super) fn key_bytes(keystroke: &Keystroke, app_cursor: bool) -> Option<Vec<u8>> {
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

/// The terminal's cells, drawn a run of same-styled text at a time, each
/// at its column so the grid stays straight; the shell is fitted to it.
struct TerminalView {
    editor: Entity<Editor>,
    /// In terminal mode: the cursor is drawn solid.
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

/// What's on the terminal to draw, copied out of it.
struct Snapshot {
    runs: Vec<Run>,
    /// Where the cursor is, how it's drawn, and the char under it.
    cursor: Option<(usize, usize, CursorShape, String)>,
}

impl Snapshot {
    fn take<T: EventListener>(term: &Term<T>, theme: &Theme) -> Self {
        let content = term.renderable_content();
        let (offset, colors, selection) = (content.display_offset as i32, content.colors, content.selection);
        let mut runs: Vec<Run> = vec![];
        let mut next_col = None;
        for indexed in content.display_iter {
            let cell = indexed.cell;
            let (line, col) = ((indexed.point.line.0 + offset) as usize, indexed.point.column.0);
            if cell.flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
                continue;
            }
            let (mut fg, mut bg) = (resolve(cell.fg, colors, theme), background(cell.bg, colors, theme));
            if cell.flags.contains(Flags::INVERSE) {
                (fg, bg) = (bg.unwrap_or(theme.background), Some(fg));
            }
            if cell.flags.contains(Flags::DIM) {
                fg = fg.opacity(0.7);
            }
            if cell.flags.contains(Flags::HIDDEN) {
                fg = fg.opacity(0.);
            }
            if selection.is_some_and(|s| s.contains(indexed.point)) {
                bg = Some(theme.selection);
            }
            let run = Run {
                line,
                col,
                text: cell.c.to_string(),
                color: fg,
                background: bg,
                weight: if cell.flags.contains(Flags::BOLD) { FontWeight::BOLD } else { FontWeight::NORMAL },
                italic: cell.flags.contains(Flags::ITALIC),
                underline: cell.flags.intersects(Flags::ALL_UNDERLINES),
                strike: cell.flags.contains(Flags::STRIKEOUT),
            };
            let blank = cell.c == ' ' && run.background.is_none() && !run.underline;
            let wide = cell.flags.contains(Flags::WIDE_CHAR);
            let joins = next_col == Some((line, col)) && !blank && runs.last().is_some_and(|last| last.styled_like(&run));
            match runs.last_mut() {
                Some(last) if joins => last.text.push(cell.c),
                _ if !blank => runs.push(run),
                _ => {}
            }
            // A wide char stands alone, so the next cell starts where the grid says.
            next_col = (!blank && !wide).then_some((line, col + 1));
        }
        let at = content.cursor.point;
        let line = at.line.0 + offset;
        let cursor = (content.cursor.shape != CursorShape::Hidden && line >= 0 && (line as usize) < term.grid().screen_lines()).then(|| {
            let c = term.grid()[at.line][at.column].c;
            (line as usize, at.column.0, content.cursor.shape, c.to_string())
        });
        Self { runs, cursor }
    }
}

impl Run {
    fn styled_like(&self, other: &Run) -> bool {
        (self.color, self.background, self.weight, self.italic, self.underline, self.strike)
            == (other.color, other.background, other.weight, other.italic, other.underline, other.strike)
    }
}

/// A terminal color in the theme: gruvbox's sixteen, xterm's 256, or exact.
fn resolve(color: Color, colors: &Colors, theme: &Theme) -> Hsla {
    let rgb = |c: Rgb| Hsla::from(Rgba { r: c.r as f32 / 255., g: c.g as f32 / 255., b: c.b as f32 / 255., a: 1. });
    match color {
        Color::Spec(c) => rgb(c),
        Color::Indexed(i) => colors[i as usize].map_or_else(|| indexed(i, theme), rgb),
        Color::Named(name) => {
            if let Some(c) = colors[name as usize] {
                return rgb(c);
            }
            match name {
                NamedColor::Background => theme.background,
                NamedColor::Cursor => theme.cursor_normal,
                NamedColor::DimForeground => theme.foreground.opacity(0.7),
                n if (n as usize) < 16 => theme.ansi[n as usize],
                n if (NamedColor::DimBlack as usize..=NamedColor::DimWhite as usize).contains(&(n as usize)) => {
                    theme.ansi[n as usize - NamedColor::DimBlack as usize].opacity(0.7)
                }
                _ => theme.foreground,
            }
        }
    }
}

/// A cell's background, if it isn't the terminal's own.
fn background(color: Color, colors: &Colors, theme: &Theme) -> Option<Hsla> {
    (color != Color::Named(NamedColor::Background)).then(|| resolve(color, colors, theme))
}

/// One of xterm's 256 colors: the sixteen, a 6×6×6 cube, then grays.
fn indexed(i: u8, theme: &Theme) -> Hsla {
    let level = |v: u8| if v == 0 { 0. } else { (55. + 40. * v as f32) / 255. };
    let rgba = match i {
        0..16 => return theme.ansi[i as usize],
        16..232 => {
            let i = i - 16;
            Rgba { r: level(i / 36), g: level(i / 6 % 6), b: level(i % 6), a: 1. }
        }
        _ => {
            let gray = (8. + 10. * (i - 232) as f32) / 255.;
            Rgba { r: gray, g: gray, b: gray, a: 1. }
        }
    };
    rgba.into()
}

pub struct Prepainted {
    texts: Vec<(Point, ShapedLine)>,
    quads: Vec<PaintQuad>,
    cursor: Option<PaintQuad>,
    cursor_glyph: Option<(Point, ShapedLine)>,
    line_height: Pixels,
}

type Point = gpui::Point<Pixels>;

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
        let snapshot = self.editor.update(cx, |editor, _| {
            editor.terminal.cells = Some((bounds.origin, cw, lh));
            let terminal = editor.terminal.terminal.as_ref()?;
            terminal.resize(cols, rows, f32::from(cw) as u16, f32::from(lh) as u16);
            Some(terminal.with(|term| Snapshot::take(term, &theme)))
        });
        let mut state = Prepainted { texts: vec![], quads: vec![], cursor: None, cursor_glyph: None, line_height: lh };
        let Some(snapshot) = snapshot else { return state };
        let at = |line: usize, col: usize| point(bounds.left() + cw * col as f32, bounds.top() + lh * line as f32);
        for run in &snapshot.runs {
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
        if let Some((line, col, shape, c)) = snapshot.cursor {
            let origin = at(line, col);
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
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_reach_the_shell_as_xterm_sends_them() {
        let k = |s: &str| key_bytes(&Keystroke::parse(s).unwrap(), false);
        assert_eq!(k("enter"), Some(b"\r".to_vec()));
        assert_eq!(k("ctrl-c"), Some(vec![3]));
        assert_eq!(k("up"), Some(b"\x1b[A".to_vec()));
        assert_eq!(key_bytes(&Keystroke::parse("up").unwrap(), true), Some(b"\x1bOA".to_vec()), "application cursor keys");
        assert_eq!(k("ctrl-left"), Some(b"\x1b[1;5D".to_vec()));
        assert_eq!(k("shift-tab"), Some(b"\x1b[Z".to_vec()));
        assert_eq!(k("alt-backspace"), Some(b"\x1b\x7f".to_vec()));
        assert_eq!(k("shift-a"), Some(b"A".to_vec()));
        assert_eq!(k("cmd-v"), None, "the app's");
        let option_7 = Keystroke { key_char: Some("|".into()), ..Keystroke::parse("alt-7").unwrap() };
        assert_eq!(key_bytes(&option_7, false), Some(b"|".to_vec()), "option types what the layout says");
    }
}
