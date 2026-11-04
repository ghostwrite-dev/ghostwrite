//! Pages in a tab, whatever they're of ([`super::rendered`]): one under another,
//! with the selection and the cursor drawn where that text is on them. Clicks put
//! the cursor on the text under them; the wheel scrolls. The tab of what's only pages moves
//! through a line for each, as code does; a tab that's a view of a file shows that file's cursor.
//!
//! Pages are drawn as [`crate::ui::pages`] draws them: one that's the same as
//! before isn't drawn again.

use std::sync::Arc;

use ghostwrite_core::Mode;
use ghostwrite_page::{Pages, Place};
use gpui::{
    App, Bounds, DispatchPhase, Element, ElementId, ElementInputHandler, Entity, FocusHandle, GlobalElementId, IntoElement, LayoutId, MouseButton,
    MouseDownEvent, Pixels, ScrollWheelEvent, Style, Window, px, relative, size,
};

use super::element::Metrics;
use super::tabs::Autoscroll;
use super::{Editor, Measured};
use crate::theme::theme;
use crate::ui::pages::{self, Shown, Stack, Viewport};

pub struct PageView {
    editor: Entity<Editor>,
    focus: FocusHandle,
    /// The tab it shows, if it's the other pane's: the active one otherwise.
    doc: Option<super::DocId>,
}

impl PageView {
    pub fn new(editor: Entity<Editor>, focus: FocusHandle) -> Self {
        Self { editor, focus, doc: None }
    }

    /// The other pane, showing the pages of the tab of `doc`.
    pub fn of_other_pane(editor: Entity<Editor>, focus: FocusHandle, doc: super::DocId) -> Self {
        Self { editor, focus, doc: Some(doc) }
    }
}

fn rect(stack: &Stack, place: Place) -> Bounds<Pixels> {
    stack.rect(place.page, place.x, place.y, place.width, place.height)
}

pub struct Prepainted {
    stack: Stack,
    shown: Shown,
    pages: Arc<dyn Pages>,
    line_height: Pixels,
}

impl IntoElement for PageView {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for PageView {
    type RequestLayoutState = ();
    type PrepaintState = Option<Prepainted>;

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
    ) -> Option<Prepainted> {
        let theme = theme(cx).clone();
        let metrics = Metrics::measure(&theme, window);
        let focused = self.focus.is_focused(window);
        let view_height = bounds.size.height;
        let (id, pages, stack, scroll_top, marks) = {
            let editor = self.editor.read(cx);
            let id = self.doc.map_or_else(|| editor.shown_pages(), |doc| editor.pages_of(doc))?;
            let tab = editor.project.tabs.by_id(id)?;
            let pages = tab.doc.rendered.as_ref()?.pages.clone()?;
            let scroll = &tab.view.pages;
            let max_width = pages.max_width().map(|width| px(width * theme.zoom));
            let stack = Stack::new(pages.sizes(), bounds.size.width, max_width, scroll.zoom, scroll.left);
            let buffer = &tab.doc.buffer;
            let (selection, text) = (tab.view.selection, buffer.text());
            let cursor = selection.cursor(text);

            // Scroll to what was asked for. Only a tab showing its own text follows
            // its cursor: a file's goes with the file's code, on its own tab.
            let line_box = |line: usize| {
                let (start, end) = (buffer.line_start(line), buffer.line_end(line));
                pages.covering(start..end).first().copied().or_else(|| pages.cursor(start)).map(|place| rect(&stack, place))
            };
            let own = editor.tab().is_some_and(|active| active.doc.id == id);
            let mut top = scroll.top;
            let cursor_line = buffer.line_of(cursor);
            match tab.view.autoscroll.as_ref().filter(|_| own) {
                Some(Autoscroll::Fit) => {
                    if let Some(r) = line_box(cursor_line) {
                        let margin = (metrics.line_height * crate::config::get().margin_lines).min(view_height / 3.);
                        if r.top() - margin < top {
                            top = r.top() - margin;
                        } else if r.bottom() + margin > top + view_height {
                            top = r.bottom() + margin - view_height;
                        }
                    }
                }
                Some(Autoscroll::Top) => top = line_box(cursor_line).map_or(top, |r| r.top()),
                Some(Autoscroll::Bottom) => top = line_box(cursor_line).map_or(top, |r| r.bottom() - view_height),
                Some(Autoscroll::Center(lines)) => {
                    let last = lines.end.max(lines.start + 1) - 1;
                    if let (Some(from), Some(to)) = (line_box(lines.start), line_box(last)) {
                        let (from, to) = (from.top(), to.bottom());
                        top = if to - from > view_height { from - metrics.line_height } else { (from + to) / 2. - view_height / 2. };
                    }
                }
                None => {}
            }
            let wanted = scroll.reveal.and_then(|(at, _)| pages.nearest(at)).map(|place| rect(&stack, place));
            let scroll_top = stack.reveal(top, wanted, scroll.reveal.is_some_and(|(_, always)| always), view_height);

            // The selection and the cursor.
            let mode = editor.modal.mode;
            let mut marks = vec![];
            if !matches!(mode, Mode::Tree | Mode::Terminal) && !editor.reading() {
                if selection.is_wide(text) {
                    marks.extend(pages.covering(selection.bytes()).into_iter().map(|place| (rect(&stack, place), theme.selection)));
                }
                if let Some(r) = pages.cursor(cursor).map(|place| rect(&stack, place)) {
                    let color = pages::cursor_color(mode == Mode::Select, &theme, focused);
                    marks.push((Bounds::new(r.origin, size(r.size.width.max(px(3.)), r.size.height.max(metrics.line_height * 0.8))), color));
                }
            }
            (id, pages, stack, scroll_top, marks)
        };

        // The pages on screen, and their drawings when they're ready.
        let pixels = stack.pixels(window);
        let mut shown = Shown::new(&stack, Viewport { bounds, scroll_top }, &pages, id, pixels, window, cx);
        for (r, color) in marks {
            shown.mark(r, color);
        }

        let rows = (f32::from(view_height) / f32::from(metrics.line_height)).floor() as usize;
        let in_other_pane = self.doc.is_some();
        self.editor.update(cx, |editor, _| {
            let own = editor.tab().is_some_and(|active| active.doc.id == id);
            if let Some(tab) = editor.project.tabs.by_id_mut(id) {
                (tab.view.pages.top, tab.view.pages.left, tab.view.pages.reveal) = (scroll_top, stack.scroll_left, None);
                if own {
                    tab.view.autoscroll = None;
                }
            }
            if !in_other_pane {
                editor.measured = Measured { rows, cursor: None, top_line: None };
            }
        });
        Some(Prepainted { stack, shown, pages, line_height: metrics.line_height })
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        state: &mut Option<Prepainted>,
        window: &mut Window,
        cx: &mut App,
    ) {
        window.handle_input(&self.focus, ElementInputHandler::new(bounds, self.editor.clone()), cx);
        let Some(state) = state else { return };
        pages::paint(&mut state.shown, window);
        self.listen(bounds, state, window);
    }
}

impl PageView {
    /// A click puts the cursor on the text under it; the wheel scrolls, and with ⌘
    /// (or Ctrl) held zooms, around the pointer.
    fn listen(&self, bounds: Bounds<Pixels>, state: &Prepainted, window: &mut Window) {
        let (stack, pages, viewport, line_height) = (state.stack.clone(), state.pages.clone(), state.shown.viewport, state.line_height);
        let (editor, focus, doc) = (self.editor.clone(), self.focus.clone(), self.doc);
        window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
            if phase != DispatchPhase::Bubble || event.button != MouseButton::Left || !bounds.contains(&event.position) {
                return;
            }
            window.focus(&focus);
            let byte = stack.at(viewport.unplace(event.position)).and_then(|(page, x, y)| pages.at(page, x, y));
            editor.update(cx, |editor, cx| editor.click_page_of(doc, byte, event.modifiers.shift, event.click_count, cx));
        });
        let editor = self.editor.clone();
        window.on_mouse_event(move |event: &ScrollWheelEvent, phase, _, cx| {
            if phase != DispatchPhase::Bubble || !bounds.contains(&event.position) {
                return;
            }
            let delta = event.delta.pixel_delta(line_height);
            if event.modifiers.platform || event.modifiers.control {
                let around = event.position - bounds.origin;
                // Rolled away from you (as it scrolls up), it zooms in.
                let by = (f32::from(delta.y) / 200.).exp();
                return editor.update(cx, |editor, cx| editor.zoom_pages_by(doc, by, around, cx));
            }
            editor.update(cx, |editor, cx| editor.scroll_pages(doc, delta, cx));
        });
    }
}
