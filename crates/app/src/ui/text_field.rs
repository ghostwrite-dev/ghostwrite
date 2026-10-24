//! A single-line text field, for prompts: the `:` line, asking Claude, and
//! naming files. It takes the font of where it's placed, and tells its owner
//! about Enter, Esc, ↑/↓ and Tab through [`TextFieldEvent`]s.

use std::ops::Range;

use gpui::{
    App, Bounds, ClipboardItem, Context, CursorStyle, Element, ElementId, ElementInputHandler, Entity, EntityInputHandler, EventEmitter, FocusHandle,
    Focusable, GlobalElementId, KeyBinding, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad, Pixels, Point, ShapedLine,
    SharedString, Style, TextRun, UTF16Selection, UnderlineStyle, Window, actions, div, fill, point, prelude::*, px, relative, size,
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
        KeyBinding::new("cmd-backspace", DeleteToStart, ctx),
        KeyBinding::new("left", Left, ctx),
        KeyBinding::new("right", Right, ctx),
        KeyBinding::new("alt-left", WordLeft, ctx),
        KeyBinding::new("alt-right", WordRight, ctx),
        KeyBinding::new("shift-left", SelectLeft, ctx),
        KeyBinding::new("shift-right", SelectRight, ctx),
        KeyBinding::new("cmd-a", SelectAll, ctx),
        KeyBinding::new("cmd-v", Paste, ctx),
        KeyBinding::new("cmd-c", Copy, ctx),
        KeyBinding::new("cmd-x", Cut, ctx),
        KeyBinding::new("home", Home, ctx),
        KeyBinding::new("cmd-left", Home, ctx),
        KeyBinding::new("ctrl-a", Home, ctx),
        KeyBinding::new("end", End, ctx),
        KeyBinding::new("cmd-right", End, ctx),
        KeyBinding::new("ctrl-e", End, ctx),
        KeyBinding::new("enter", Submit, ctx),
        KeyBinding::new("escape", Cancel, ctx),
        KeyBinding::new("up", Up, ctx),
        KeyBinding::new("down", Down, ctx),
        KeyBinding::new("tab", Complete, ctx),
    ]);
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
    last_layout: Option<ShapedLine>,
    last_bounds: Option<Bounds<Pixels>>,
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
            last_bounds: None,
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
        let (Some(bounds), Some(line)) = (self.last_bounds.as_ref(), self.last_layout.as_ref()) else { return 0 };
        if self.text.is_empty() || position.y < bounds.top() {
            return 0;
        }
        if position.y > bounds.bottom() {
            return self.text.len();
        }
        line.closest_index_for_x(position.x - bounds.left()).min(self.text.len())
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
        self.selected = new_selected
            .as_ref()
            .map(|r| self.range_from_utf16(r))
            .map(|r| range.start + r.start..range.start + r.end)
            .unwrap_or_else(|| range.start + text.len()..range.start + text.len());
        cx.emit(TextFieldEvent::Changed);
        cx.notify();
    }

    fn bounds_for_range(&mut self, range: Range<usize>, bounds: Bounds<Pixels>, _: &mut Window, _: &mut Context<Self>) -> Option<Bounds<Pixels>> {
        let layout = self.last_layout.as_ref()?;
        let range = self.range_from_utf16(&range);
        Some(Bounds::from_corners(
            point(bounds.left() + layout.x_for_index(range.start), bounds.top()),
            point(bounds.left() + layout.x_for_index(range.end), bounds.bottom()),
        ))
    }

    fn character_index_for_point(&mut self, point: Point<Pixels>, _: &mut Window, _: &mut Context<Self>) -> Option<usize> {
        let local = self.last_bounds?.localize(&point)?;
        let index = self.last_layout.as_ref()?.index_for_x(local.x)?;
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
    line: ShapedLine,
    cursor: Option<PaintQuad>,
    selection: Option<PaintQuad>,
}

impl IntoElement for TextFieldElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
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
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = window.line_height().into();
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
        let field = self.field.read(cx);
        let style = window.text_style();
        let theme = theme(cx);
        let (shown, color) = if field.text.is_empty() { (field.placeholder.clone(), theme.muted_foreground) } else { (field.text.clone().into(), style.color) };
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
        let font_size = style.font_size.to_pixels(window.rem_size());
        let line = window.text_system().shape_line(shown, font_size, &runs, None);
        let (cursor, selection) = if field.selected.is_empty() {
            let x = if field.text.is_empty() { px(0.) } else { line.x_for_index(field.cursor()) };
            (Some(fill(Bounds::new(point(bounds.left() + x, bounds.top()), size(px(2.), bounds.size.height)), theme.cursor_insert)), None)
        } else {
            let (from, to) = (line.x_for_index(field.selected.start), line.x_for_index(field.selected.end));
            (None, Some(fill(Bounds::from_corners(point(bounds.left() + from, bounds.top()), point(bounds.left() + to, bounds.bottom())), theme.selection)))
        };
        Prepainted { line, cursor, selection }
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
        window.handle_input(&focus, ElementInputHandler::new(bounds, self.field.clone()), cx);
        if let Some(selection) = prepainted.selection.take() {
            window.paint_quad(selection);
        }
        prepainted.line.paint(bounds.origin, window.line_height(), window, cx).ok();
        if focus.is_focused(window)
            && let Some(cursor) = prepainted.cursor.take()
        {
            window.paint_quad(cursor);
        }
        let line = prepainted.line.clone();
        self.field.update(cx, |field, _| {
            field.last_layout = Some(line);
            field.last_bounds = Some(bounds);
        });
    }
}
