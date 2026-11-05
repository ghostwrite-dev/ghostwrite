//! A one-paragraph text field, for prompts: the `:` line, asking the model, and
//! naming files. It takes the font of where it's placed, wraps at its width
//! (growing a line at a time, or scrolling to the cursor where its height is
//! fixed), and tells its owner about Enter, Esc, ↑/↓ and Tab through
//! [`TextFieldEvent`]s.

use std::ops::Range;

use gpui::{
    App, AvailableSpace, Bounds, ClipboardItem, Context, CursorStyle, Element, ElementId, ElementInputHandler, Entity, EntityInputHandler, EventEmitter,
    FocusHandle, Focusable, GlobalElementId, KeyBinding, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad, Pixels, Point,
    SharedString, Style, TextAlign, TextRun, UTF16Selection, UnderlineStyle, Window, WrappedLine, actions, div, fill, point, prelude::*, px, relative, size,
};

use crate::theme::theme;

actions!(
    text_field,
    [
        Backspace,
        Delete,
        DeleteWord,
        DeleteToStart,
        Left,
        Right,
        WordLeft,
        WordRight,
        SelectLeft,
        SelectRight,
        SelectAll,
        Home,
        End,
        Paste,
        Cut,
        Copy,
        Submit,
        Cancel,
        Up,
        Down,
        Complete
    ]
);

pub fn bind_keys(cx: &mut App) {
    let ctx = Some("TextField");
    cx.bind_keys([
        KeyBinding::new("backspace", Backspace, ctx),
        KeyBinding::new("delete", Delete, ctx),
        KeyBinding::new("alt-backspace", DeleteWord, ctx),
        KeyBinding::new(&crate::keys::in_field("cmd-backspace"), DeleteToStart, ctx),
        KeyBinding::new("left", Left, ctx),
        KeyBinding::new("right", Right, ctx),
        KeyBinding::new("alt-left", WordLeft, ctx),
        KeyBinding::new("alt-right", WordRight, ctx),
        KeyBinding::new("shift-left", SelectLeft, ctx),
        KeyBinding::new("shift-right", SelectRight, ctx),
        KeyBinding::new(&crate::keys::in_field("cmd-a"), SelectAll, ctx),
        KeyBinding::new(&crate::keys::in_field("cmd-v"), Paste, ctx),
        KeyBinding::new(&crate::keys::in_field("cmd-c"), Copy, ctx),
        KeyBinding::new(&crate::keys::in_field("cmd-x"), Cut, ctx),
        KeyBinding::new("home", Home, ctx),
        KeyBinding::new(&crate::keys::in_field("cmd-left"), Home, ctx),
        KeyBinding::new("end", End, ctx),
        KeyBinding::new(&crate::keys::in_field("cmd-right"), End, ctx),
        KeyBinding::new("ctrl-e", End, ctx),
        KeyBinding::new("enter", Submit, ctx),
        KeyBinding::new("escape", Cancel, ctx),
        KeyBinding::new("up", Up, ctx),
        KeyBinding::new("down", Down, ctx),
        KeyBinding::new("tab", Complete, ctx),
    ]);
    // On a Mac Ctrl+A goes to the start, as in Emacs. Elsewhere it's select all (`cmd-a` above),
    // and a later binding of it here would win over that.
    if cfg!(target_os = "macos") {
        cx.bind_keys([KeyBinding::new("ctrl-a", Home, ctx)]);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextFieldEvent {
    Submit,
    Cancel,
    Up,
    Down,
    Complete,
    Changed,
}

pub struct TextField {
    focus: FocusHandle,
    text: String,
    placeholder: SharedString,
    selected: Range<usize>,
    reversed: bool,
    marked: Option<Range<usize>>,
    /// As last drawn: the text's lines, where its top left was, and how tall a line is.
    last_layout: Option<(WrappedLine, Point<Pixels>, Pixels)>,
    selecting: bool,
}

impl EventEmitter<TextFieldEvent> for TextField {}

impl Focusable for TextField {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl TextField {
    pub fn new(text: impl Into<String>, placeholder: impl Into<SharedString>, cx: &mut Context<Self>) -> Self {
        let text = text.into();
        let end = text.len();
        Self {
            focus: cx.focus_handle(),
            text,
            placeholder: placeholder.into(),
            selected: end..end,
            reversed: false,
            marked: None,
            last_layout: None,
            selecting: false,
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    /// Replace the text, with the cursor at the end.
    pub fn set_text(&mut self, text: impl Into<String>, cx: &mut Context<Self>) {
        self.text = text.into();
        self.selected = self.text.len()..self.text.len();
        self.marked = None;
        cx.notify();
    }

    pub fn focus(&self, window: &mut Window) {
        window.focus(&self.focus);
    }

    fn cursor(&self) -> usize {
        if self.reversed { self.selected.start } else { self.selected.end }
    }

    fn move_to(&mut self, at: usize, cx: &mut Context<Self>) {
        self.selected = at..at;
        self.reversed = false;
        cx.notify();
    }

    fn select_to(&mut self, at: usize, cx: &mut Context<Self>) {
        if self.reversed {
            self.selected.start = at;
        } else {
            self.selected.end = at;
        }
        if self.selected.end < self.selected.start {
            self.reversed = !self.reversed;
            self.selected = self.selected.end..self.selected.start;
        }
        cx.notify();
    }

    fn prev(&self, at: usize) -> usize {
        ghostwrite_core::prev_char(&self.text, at)
    }

    fn next(&self, at: usize) -> usize {
        ghostwrite_core::next_char(&self.text, at)
    }

    fn word_start(&self, at: usize) -> usize {
        let before = self.text[..at].trim_end();
        before.rfind(|c: char| !c.is_alphanumeric() && c != '_').map_or(0, |i| i + 1).min(at)
    }

    fn word_end(&self, at: usize) -> usize {
        let skipped = at + (self.text[at..].len() - self.text[at..].trim_start().len());
        self.text[skipped..].find(|c: char| !c.is_alphanumeric() && c != '_').map_or(self.text.len(), |i| skipped + i)
    }

    fn replace(&mut self, range: Range<usize>, text: &str, cx: &mut Context<Self>) {
        self.text.replace_range(range.clone(), text);
        let at = range.start + text.len();
        self.selected = at..at;
        self.reversed = false;
        self.marked = None;
        cx.emit(TextFieldEvent::Changed);
        cx.notify();
    }

    fn delete_back_to(&mut self, to: usize, cx: &mut Context<Self>) {
        let range = if self.selected.is_empty() { to..self.cursor() } else { self.selected.clone() };
        self.replace(range, "", cx);
    }

    fn backspace(&mut self, _: &Backspace, _: &mut Window, cx: &mut Context<Self>) {
        let to = self.prev(self.cursor());
        self.delete_back_to(to, cx);
    }

    fn delete(&mut self, _: &Delete, _: &mut Window, cx: &mut Context<Self>) {
        let range = if self.selected.is_empty() { self.cursor()..self.next(self.cursor()) } else { self.selected.clone() };
        self.replace(range, "", cx);
    }

    fn delete_word(&mut self, _: &DeleteWord, _: &mut Window, cx: &mut Context<Self>) {
        let to = self.word_start(self.cursor());
        self.delete_back_to(to, cx);
    }

    fn delete_to_start(&mut self, _: &DeleteToStart, _: &mut Window, cx: &mut Context<Self>) {
        self.delete_back_to(0, cx);
    }

    fn left(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        let at = if self.selected.is_empty() { self.prev(self.cursor()) } else { self.selected.start };
        self.move_to(at, cx);
    }

    fn right(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        let at = if self.selected.is_empty() { self.next(self.cursor()) } else { self.selected.end };
        self.move_to(at, cx);
    }

    fn word_left(&mut self, _: &WordLeft, _: &mut Window, cx: &mut Context<Self>) {
        let at = self.word_start(self.cursor());
        self.move_to(at, cx);
    }

    fn word_right(&mut self, _: &WordRight, _: &mut Window, cx: &mut Context<Self>) {
        let at = self.word_end(self.cursor());
        self.move_to(at, cx);
    }

    fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        let at = self.prev(self.cursor());
        self.select_to(at, cx);
    }

    fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        let at = self.next(self.cursor());
        self.select_to(at, cx);
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.selected = 0..self.text.len();
        self.reversed = false;
        cx.notify();
    }

    fn home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
    }

    fn end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.text.len(), cx);
    }

    fn paste(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            let range = self.selected.clone();
            self.replace(range, &text.replace('\n', " "), cx);
        }
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selected.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(self.text[self.selected.clone()].to_string()));
        }
    }

    fn cut(&mut self, _: &Cut, window: &mut Window, cx: &mut Context<Self>) {
        self.copy(&Copy, window, cx);
        if !self.selected.is_empty() {
            let range = self.selected.clone();
            self.replace(range, "", cx);
        }
    }

    fn on_mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.focus(window);
        self.selecting = true;
        let at = self.index_for_mouse(event.position);
        if event.modifiers.shift { self.select_to(at, cx) } else { self.move_to(at, cx) }
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.selecting = false;
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.selecting {
            let at = self.index_for_mouse(event.position);
            self.select_to(at, cx);
        }
    }

    fn index_for_mouse(&self, position: Point<Pixels>) -> usize {
        let Some((line, origin, line_height)) = self.last_layout.as_ref().filter(|_| !self.text.is_empty()) else { return 0 };
        let rows = line.wrap_boundaries().len() + 1;
        let local = position - *origin;
        let local = point(local.x.max(px(0.)), local.y.clamp(px(0.), *line_height * rows as f32 - px(1.)));
        line.closest_index_for_position(local, *line_height).unwrap_or_else(|at| at).min(self.text.len())
    }

    /// Where byte `at` is drawn, as of the last time: the top of its line.
    fn position_of(&self, at: usize) -> Option<Point<Pixels>> {
        let (line, origin, line_height) = self.last_layout.as_ref()?;
        Some(*origin + line.position_for_index(at, *line_height)?)
    }

    fn offset_from_utf16(&self, offset: usize) -> usize {
        let mut utf8 = 0;
        let mut utf16 = 0;
        for ch in self.text.chars() {
            if utf16 >= offset {
                break;
            }
            utf16 += ch.len_utf16();
            utf8 += ch.len_utf8();
        }
        utf8
    }

    fn offset_to_utf16(&self, offset: usize) -> usize {
        self.text[..offset.min(self.text.len())].chars().map(char::len_utf16).sum()
    }

    fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    fn range_from_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range.start)..self.offset_from_utf16(range.end)
    }
}

impl EntityInputHandler for TextField {
    fn text_for_range(&mut self, range: Range<usize>, actual: &mut Option<Range<usize>>, _: &mut Window, _: &mut Context<Self>) -> Option<String> {
        let range = self.range_from_utf16(&range);
        actual.replace(self.range_to_utf16(&range));
        Some(self.text[range].to_string())
    }

    fn selected_text_range(&mut self, _: bool, _: &mut Window, _: &mut Context<Self>) -> Option<UTF16Selection> {
        Some(UTF16Selection { range: self.range_to_utf16(&self.selected), reversed: self.reversed })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked.as_ref().map(|range| self.range_to_utf16(range))
    }

    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {
        self.marked = None;
    }

    fn replace_text_in_range(&mut self, range: Option<Range<usize>>, text: &str, _: &mut Window, cx: &mut Context<Self>) {
        let range = range.as_ref().map(|r| self.range_from_utf16(r)).or(self.marked.clone()).unwrap_or(self.selected.clone());
        self.replace(range, text, cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        new_selected: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range.as_ref().map(|r| self.range_from_utf16(r)).or(self.marked.clone()).unwrap_or(self.selected.clone());
        self.text.replace_range(range.clone(), text);
        self.marked = (!text.is_empty()).then(|| range.start..range.start + text.len());
        // The selection is in the composed `text`, in UTF-16 units.
        let in_text = |units: usize| {
            let mut counted = 0;
            for (i, c) in text.char_indices() {
                if counted >= units {
                    return i;
                }
                counted += c.len_utf16();
            }
            text.len()
        };
        self.selected = new_selected
            .map(|r| range.start + in_text(r.start)..range.start + in_text(r.end))
            .unwrap_or_else(|| range.start + text.len()..range.start + text.len());
        cx.emit(TextFieldEvent::Changed);
        cx.notify();
    }

    fn bounds_for_range(&mut self, range: Range<usize>, _: Bounds<Pixels>, _: &mut Window, _: &mut Context<Self>) -> Option<Bounds<Pixels>> {
        let range = self.range_from_utf16(&range);
        let line_height = self.last_layout.as_ref()?.2;
        let (from, to) = (self.position_of(range.start)?, self.position_of(range.end)?);
        Some(Bounds::from_corners(from, point(if to.y > from.y { from.x } else { to.x }, from.y + line_height)))
    }

    fn character_index_for_point(&mut self, point: Point<Pixels>, _: &mut Window, _: &mut Context<Self>) -> Option<usize> {
        let (line, origin, line_height) = self.last_layout.as_ref()?;
        let index = line.index_for_position(point - *origin, *line_height).ok()?;
        Some(self.offset_to_utf16(index))
    }
}

impl Render for TextField {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .key_context("TextField")
            .track_focus(&self.focus)
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::delete_word))
            .on_action(cx.listener(Self::delete_to_start))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::word_left))
            .on_action(cx.listener(Self::word_right))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(|_, _: &Submit, _, cx| cx.emit(TextFieldEvent::Submit)))
            .on_action(cx.listener(|_, _: &Cancel, _, cx| cx.emit(TextFieldEvent::Cancel)))
            .on_action(cx.listener(|_, _: &Up, _, cx| cx.emit(TextFieldEvent::Up)))
            .on_action(cx.listener(|_, _: &Down, _, cx| cx.emit(TextFieldEvent::Down)))
            .on_action(cx.listener(|_, _: &Complete, _, cx| cx.emit(TextFieldEvent::Complete)))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .w_full()
            .child(TextFieldElement { field: cx.entity() })
    }
}

struct TextFieldElement {
    field: Entity<TextField>,
}

struct Prepainted {
    line: WrappedLine,
    /// Where the first line is drawn: up from the top, to keep the cursor in a short field.
    origin: Point<Pixels>,
    cursor: Option<PaintQuad>,
    selection: Vec<PaintQuad>,
}

impl IntoElement for TextFieldElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

/// What the field shows, in what runs: its text (newlines as spaces, so
/// it's one paragraph) or the placeholder, with what's being composed underlined.
fn shown(field: &TextField, window: &Window, cx: &App) -> (SharedString, Vec<TextRun>, Pixels) {
    let style = window.text_style();
    let (shown, color) =
        if field.text.is_empty() { (field.placeholder.clone(), theme(cx).muted_foreground) } else { (field.text.replace('\n', " ").into(), style.color) };
    let run = TextRun { len: shown.len(), font: style.font(), color, background_color: None, underline: None, strikethrough: None };
    let runs = match field.marked.as_ref().filter(|_| !field.text.is_empty()) {
        Some(marked) => [
            TextRun { len: marked.start, ..run.clone() },
            TextRun { len: marked.len(), underline: Some(UnderlineStyle { color: Some(color), thickness: px(1.), wavy: false }), ..run.clone() },
            TextRun { len: shown.len() - marked.end, ..run },
        ]
        .into_iter()
        .filter(|r| r.len > 0)
        .collect(),
        None => vec![run],
    };
    (shown, runs, style.font_size.to_pixels(window.rem_size()))
}

fn shape(shown: SharedString, runs: &[TextRun], font_size: Pixels, width: Pixels, window: &Window) -> WrappedLine {
    window.text_system().shape_text(shown, font_size, runs, Some(width), None).ok().and_then(|lines| lines.into_iter().next()).unwrap_or_default()
}

impl Element for TextFieldElement {
    type RequestLayoutState = ();
    type PrepaintState = Prepainted;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(&mut self, _: Option<&GlobalElementId>, _: Option<&gpui::InspectorElementId>, window: &mut Window, cx: &mut App) -> (LayoutId, ()) {
        let (shown, runs, font_size) = shown(self.field.read(cx), window, cx);
        let line_height = window.line_height();
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        let layout = window.request_measured_layout(style, move |known, available, window, _| {
            let width = known.width.or(match available.width {
                AvailableSpace::Definite(width) => Some(width),
                _ => None,
            });
            let rows = width.map_or(1, |width| shape(shown.clone(), &runs, font_size, width, window).wrap_boundaries().len() + 1);
            let height = line_height * rows as f32;
            // Where its height is fixed, it scrolls instead.
            let room = match available.height {
                AvailableSpace::Definite(room) => room.max(line_height),
                _ => height,
            };
            size(width.unwrap_or_default(), height.min(room))
        });
        (layout, ())
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
        let field = self.field.read(cx);
        let theme = theme(cx);
        let line_height = window.line_height();
        let (shown, runs, font_size) = shown(field, window, cx);
        let line = shape(shown, &runs, font_size, bounds.size.width, window);
        let at = |index: usize| line.position_for_index(index, line_height).unwrap_or_default();
        let cursor = if field.text.is_empty() { point(px(0.), px(0.)) } else { at(field.cursor()) };
        let scroll = (cursor.y + line_height - bounds.size.height).max(px(0.));
        let origin = bounds.origin - point(px(0.), scroll);
        let (cursor, selection) = if field.selected.is_empty() {
            (Some(fill(Bounds::new(origin + cursor, size(px(2.), line_height)), theme.cursor_insert)), vec![])
        } else {
            let (from, to) = (at(field.selected.start), at(field.selected.end));
            let rows = ((to.y - from.y) / line_height).round() as usize;
            let quads = (0..=rows).map(|row| {
                let y = from.y + line_height * row as f32;
                let left = if row == 0 { from.x } else { px(0.) };
                let right = if row == rows { to.x } else { bounds.size.width };
                fill(Bounds::from_corners(origin + point(left, y), origin + point(right, y + line_height)), theme.selection)
            });
            (None, quads.collect())
        };
        Prepainted { line, origin, cursor, selection }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        prepainted: &mut Prepainted,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus = self.field.read(cx).focus.clone();
        let line_height = window.line_height();
        window.handle_input(&focus, ElementInputHandler::new(bounds, self.field.clone()), cx);
        window.with_content_mask(Some(gpui::ContentMask { bounds }), |window| {
            for quad in prepainted.selection.drain(..) {
                window.paint_quad(quad);
            }
            prepainted.line.paint(prepainted.origin, line_height, TextAlign::Left, None, window, cx).ok();
            if focus.is_focused(window)
                && let Some(cursor) = prepainted.cursor.take()
            {
                window.paint_quad(cursor);
            }
        });
        let layout = (prepainted.line.clone(), prepainted.origin, line_height);
        self.field.update(cx, |field, _| field.last_layout = Some(layout));
    }
}
