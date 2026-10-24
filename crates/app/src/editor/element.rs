//! Drawing a document: the gutter, highlighted lines, selections, helix's
//! block cursor (a bar in insert mode), diagnostics, the marks a story or
//! tour puts on lines, and a [`Block`] folded out between two lines that
//! pushes the code below it down. Also scrolling, and clicks into the text.
//!
//! Prepaint runs in passes: copy what's needed out of the editor
//! ([`Snapshot`]), measure the font ([`Metrics`]), lay out the block
//! ([`Geometry`]), scroll, shape the lines on screen, then work out what to
//! fill behind and on top of them. Paint just draws the result.

use std::ops::Range as Bytes;

use ghostwrite_core::syntax::SCOPES;
use ghostwrite_core::{Mode, Range};
use ghostwrite_lsp::Severity;
use gpui::{
    AnyElement, App, AvailableSpace, Bounds, ContentMask, DispatchPhase, Element, ElementId, ElementInputHandler, Entity, FocusHandle, Font, GlobalElementId,
    Hsla, IntoElement, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, PaintQuad, Pixels, Point, ScrollWheelEvent, ShapedLine, SharedString,
    StrikethroughStyle, Style, TextRun, UnderlineStyle, Window, fill, font, point, px, relative,
};

use super::tabs::{Autoscroll, Tab};
use super::{Editor, Measured};
use crate::theme::{Theme, theme};

/// A mark on some text: a background, a strike, or a different color.
#[derive(Clone, Debug)]
pub struct Decoration {
    pub range: Bytes<usize>,
    pub background: Option<Hsla>,
    pub strike: Option<Hsla>,
    pub color: Option<Hsla>,
}

impl Decoration {
    pub fn background(range: Bytes<usize>, color: Hsla) -> Self {
        Self { range, background: Some(color), strike: None, color: None }
    }
}

/// The sizes a block is built for, measured as the code view lays out.
#[derive(Debug, Clone, Copy)]
pub struct BlockSizes {
    pub line_height: Pixels,
    pub char_width: Pixels,
    /// The text area's width.
    pub width: Pixels,
}

/// Something folded out under `line`, as wide as the text area. It's built
/// when the view knows its sizes, so it fits the frame it's drawn in.
pub struct Block {
    pub line: usize,
    pub build: Box<dyn FnOnce(BlockSizes) -> AnyElement>,
}

pub struct CodeView {
    editor: Entity<Editor>,
    focus: FocusHandle,
    decorations: Vec<Decoration>,
    block: Option<Block>,
    /// The block once built: its line and element.
    built: Option<(usize, AnyElement)>,
}

impl CodeView {
    pub fn new(editor: Entity<Editor>, focus: FocusHandle, decorations: Vec<Decoration>, block: Option<Block>) -> Self {
        Self { editor, focus, decorations, block, built: None }
    }
}

/// What prepaint needs from the editor, copied out so the block can lay out
/// (which needs the app) without the editor borrowed.
struct Snapshot {
    mode: Mode,
    line_count: usize,
    selection: Range,
    autoscroll: Option<Autoscroll>,
    scroll_top: Pixels,
    scroll_left: Pixels,
    /// Where the cursor is drawn: the block's char, or the insert bar's gap.
    cursor: usize,
    cursor_line: usize,
}

impl Snapshot {
    fn take(tab: &Tab, mode: Mode) -> Self {
        let cursor = if mode == Mode::Insert { tab.view.selection.head } else { tab.cursor() };
        Self {
            mode,
            line_count: tab.doc.buffer.line_count(),
            selection: tab.view.selection,
            autoscroll: tab.view.autoscroll.clone(),
            scroll_top: tab.view.scroll_top,
            scroll_left: tab.view.scroll_left,
            cursor,
            cursor_line: tab.doc.buffer.line_of(cursor),
        }
    }
}

/// The editor font's sizes.
struct Metrics {
    font: Font,
    size: Pixels,
    line_height: Pixels,
    char_width: Pixels,
}

impl Metrics {
    fn measure(theme: &Theme, window: &Window) -> Self {
        let font = font(theme.mono_font.clone());
        let size = theme.mono_size;
        let font_id = window.text_system().resolve_font(&font);
        let char_width = window.text_system().advance(font_id, size, 'm').map_or(size * 0.6, |a| a.width);
        Self { font, size, line_height: (size * 1.5).round(), char_width }
    }

    fn run(&self, len: usize, color: Hsla) -> TextRun {
        TextRun { len, font: self.font.clone(), color, background_color: None, underline: None, strikethrough: None }
    }
}

/// Where lines are, top to bottom, with the block pushing those below it down.
struct Geometry {
    line_height: Pixels,
    block: Option<(usize, Pixels)>,
}

impl Geometry {
    fn top(&self, line: usize) -> Pixels {
        let pushed = self.block.filter(|(at, _)| line > *at).map_or(px(0.), |(_, height)| height);
        self.line_height * line as f32 + pushed
    }

    /// The bottom of `line`, including the block if it hangs from it.
    fn bottom(&self, line: usize) -> Pixels {
        let hanging = self.block.filter(|(at, _)| line == *at).map_or(px(0.), |(_, height)| height);
        self.top(line) + self.line_height + hanging
    }

    fn block_height(&self) -> Pixels {
        self.block.map_or(px(0.), |(_, height)| height)
    }

    /// Scroll so what was asked for is in view, kept within the text.
    fn scroll(&self, snap: &Snapshot, view_height: Pixels) -> Pixels {
        let mut top = snap.scroll_top;
        match &snap.autoscroll {
            Some(Autoscroll::Fit) => {
                let margin = (self.line_height * crate::config::get().margin_lines).min(view_height / 3.);
                let (from, to) = (self.top(snap.cursor_line), self.bottom(snap.cursor_line));
                if from - margin < top {
                    top = from - margin;
                } else if to + margin > top + view_height {
                    top = to + margin - view_height;
                }
            }
            Some(Autoscroll::Center(lines)) => {
                let last = lines.end.max(lines.start + 1) - 1;
                let (from, to) = (self.top(lines.start), self.bottom(last));
                top = if to - from > view_height { from - self.line_height } else { (from + to) / 2. - view_height / 2. };
            }
            None => {}
        }
        let max = (self.top(snap.line_count.saturating_sub(1)) + self.block_height() - self.line_height * 2.).max(px(0.));
        top.clamp(px(0.), max)
    }
}

/// A line as laid out on screen.
#[derive(Clone)]
struct LaidLine {
    start: usize,
    end: usize,
    y: Pixels,
    shaped: ShapedLine,
    /// Byte in the line → byte in what's shown (tabs are expanded), if they differ.
    map: Option<Vec<usize>>,
}

impl LaidLine {
    fn shown(&self, at: usize) -> usize {
        let local = at.clamp(self.start, self.end) - self.start;
        self.map.as_ref().map_or(local, |m| m[local])
    }

    fn x(&self, at: usize) -> Pixels {
        self.shaped.x_for_index(self.shown(at))
    }

    /// The byte at `x`, nearest boundary.
    fn byte_at_x(&self, x: Pixels) -> usize {
        let shown = self.shaped.closest_index_for_x(x);
        let local = match &self.map {
            Some(map) => map.iter().position(|&m| m >= shown).unwrap_or(map.len() - 1),
            None => shown,
        };
        self.start + local.min(self.end - self.start)
    }
}

/// A shaped piece of text and where it's drawn.
type Glyph = (ShapedLine, Point<Pixels>);

pub struct Prepainted {
    lines: Vec<LaidLine>,
    quads: Vec<PaintQuad>,
    cursor_glyph: Option<Glyph>,
    gutter: Vec<Glyph>,
    text_bounds: Bounds<Pixels>,
    gutter_bounds: Bounds<Pixels>,
    line_height: Pixels,
    scroll_left: Pixels,
}

impl IntoElement for CodeView {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for CodeView {
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
        let focused = self.focus.is_focused(window);
        let snap = {
            let editor = self.editor.read(cx);
            Snapshot::take(editor.project.tabs.active().expect("a code view shows the active tab"), editor.modal.mode)
        };
        let gutter_width = metrics.char_width * (snap.line_count.to_string().len().max(3) + 2) as f32;
        let text_bounds = Bounds::from_corners(point(bounds.left() + gutter_width + px(8.), bounds.top()), bounds.bottom_right());
        let gutter_bounds = Bounds::from_corners(bounds.origin, point(text_bounds.left(), bounds.bottom()));

        let sizes = BlockSizes { line_height: metrics.line_height, char_width: metrics.char_width, width: text_bounds.size.width };
        let geometry = Geometry { line_height: metrics.line_height, block: self.lay_out_block(sizes, window, cx) };
        let scroll_top = geometry.scroll(&snap, bounds.size.height);
        let (lines, gutter) = self.shape_lines(&snap, &metrics, &geometry, &theme, bounds, gutter_bounds, scroll_top, window, cx);
        let scroll_left = scroll_sideways(&snap, &lines, &metrics, text_bounds);
        let (quads, cursor_bounds, cursor_glyph) = {
            let text = self.editor.read(cx).project.tabs.active().expect("checked above").doc.text();
            let mut quads = self.backgrounds(&snap, &lines, text, &theme, text_bounds, scroll_left, metrics.line_height, focused);
            let (bounds, glyph) = cursor(&snap, &lines, text, &metrics, &theme, text_bounds.left() - scroll_left, focused, &mut quads, window);
            (quads, bounds, glyph)
        };

        if let Some((line, element)) = &mut self.built {
            let y = bounds.top() + geometry.top(*line) + metrics.line_height - scroll_top;
            element.prepaint_at(point(text_bounds.left(), y), window, cx);
        }

        let rows = (f32::from(bounds.size.height) / f32::from(metrics.line_height)).floor() as usize;
        let measured = Measured { rows, cursor: cursor_bounds };
        self.editor.update(cx, |editor, _| {
            editor.measured = measured;
            if let Some(tab) = editor.project.tabs.active_mut() {
                tab.view.scroll_top = scroll_top;
                tab.view.scroll_left = scroll_left;
                tab.view.autoscroll = None;
            }
        });
        Prepainted { lines, quads, cursor_glyph, gutter, text_bounds, gutter_bounds, line_height: metrics.line_height, scroll_left }
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
        window.handle_input(&self.focus, ElementInputHandler::new(bounds, self.editor.clone()), cx);
        let background = theme(cx).background;
        let line_height = state.line_height;
        window.with_content_mask(Some(ContentMask { bounds: state.text_bounds }), |window| {
            for quad in state.quads.drain(..) {
                window.paint_quad(quad);
            }
            let origin_x = state.text_bounds.left() - state.scroll_left;
            for laid in &state.lines {
                laid.shaped.paint(point(origin_x, laid.y), line_height, window, cx).ok();
            }
            if let Some((glyph, at)) = &state.cursor_glyph {
                glyph.paint(*at, line_height, window, cx).ok();
            }
        });
        window.with_content_mask(Some(ContentMask { bounds: state.gutter_bounds }), |window| {
            window.paint_quad(fill(state.gutter_bounds, background));
            for (number, at) in &state.gutter {
                number.paint(*at, line_height, window, cx).ok();
            }
        });
        if let Some((_, element)) = &mut self.built {
            window.with_content_mask(Some(ContentMask { bounds }), |window| element.paint(window, cx));
        }
        self.listen(bounds, state, window);
    }
}

impl CodeView {
    /// Build the block for these sizes and lay it out: its line and height.
    fn lay_out_block(&mut self, sizes: BlockSizes, window: &mut Window, cx: &mut App) -> Option<(usize, Pixels)> {
        let block = self.block.take()?;
        let mut element = (block.build)(sizes);
        let size = element.layout_as_root(gpui::size(AvailableSpace::Definite(sizes.width), AvailableSpace::MinContent), window, cx);
        self.built = Some((block.line, element));
        Some((block.line, size.height))
    }

    /// The lines on screen, shaped, and their numbers for the gutter.
    #[allow(clippy::too_many_arguments)]
    fn shape_lines(
        &self,
        snap: &Snapshot,
        metrics: &Metrics,
        geometry: &Geometry,
        theme: &Theme,
        bounds: Bounds<Pixels>,
        gutter_bounds: Bounds<Pixels>,
        scroll_top: Pixels,
        window: &mut Window,
        cx: &App,
    ) -> (Vec<LaidLine>, Vec<Glyph>) {
        let editor = self.editor.read(cx);
        let doc = &editor.project.tabs.active().expect("a code view shows the active tab").doc;
        let text = doc.text();
        let first = ((f32::from(scroll_top - geometry.block_height()) / f32::from(metrics.line_height)).floor().max(0.)) as usize;
        let mut lines = vec![];
        let mut gutter = vec![];
        for line in first..snap.line_count {
            let y = bounds.top() + geometry.top(line) - scroll_top;
            if y > bounds.bottom() {
                break;
            }
            if y + metrics.line_height < bounds.top() {
                continue;
            }
            let (start, end) = (doc.buffer.line_start(line), doc.buffer.line_end(line));
            let (shown, map) = expand_tabs(&text[start..end]);
            let runs = line_runs(start..end, &map, doc.cached_highlights(), &self.decorations, &doc.diagnostics, theme, &metrics.font);
            let shaped = window.text_system().shape_line(SharedString::from(shown), metrics.size, &runs, None);
            lines.push(LaidLine { start, end, y, shaped, map });
            // The line number, right-aligned; colored by a diagnostic on the line.
            let worst = doc.diagnostics.iter().filter(|d| doc.buffer.line_of(d.range.start) == line).map(|d| d.severity).min();
            let color = match worst {
                Some(Severity::Error) => theme.danger,
                Some(Severity::Warning) => theme.warning,
                _ if line == snap.cursor_line => theme.active_line_number,
                _ => theme.line_number,
            };
            let number: SharedString = (line + 1).to_string().into();
            let shaped_number = window.text_system().shape_line(number.clone(), metrics.size, &[metrics.run(number.len(), color)], None);
            let x = gutter_bounds.right() - px(8.) - shaped_number.width - metrics.char_width;
            gutter.push((shaped_number, point(x, y)));
        }
        (lines, gutter)
    }

    /// What's filled behind the text: the cursor's line, marks, the selection.
    #[allow(clippy::too_many_arguments)]
    fn backgrounds(
        &self,
        snap: &Snapshot,
        lines: &[LaidLine],
        text: &str,
        theme: &Theme,
        text_bounds: Bounds<Pixels>,
        scroll_left: Pixels,
        line_height: Pixels,
        focused: bool,
    ) -> Vec<PaintQuad> {
        let origin_x = text_bounds.left() - scroll_left;
        let mut quads = vec![];
        let span = |range: &Bytes<usize>, color: Hsla, quads: &mut Vec<PaintQuad>| {
            for laid in lines.iter().filter(|l| range.start <= l.end && range.end >= l.start && !(range.end == l.start && l.start != l.end)) {
                let from = origin_x + laid.x(range.start.max(laid.start));
                // Past the line's end: the rest of the row is marked too.
                let to = if range.end > laid.end { text_bounds.right() } else { origin_x + laid.x(range.end) };
                if to > from {
                    quads.push(fill(Bounds::from_corners(point(from, laid.y), point(to, laid.y + line_height)), color));
                }
            }
        };
        if focused && let Some(laid) = lines.iter().find(|l| l.start <= snap.cursor && snap.cursor <= l.end) {
            quads.push(fill(Bounds::from_corners(point(text_bounds.left(), laid.y), point(text_bounds.right(), laid.y + line_height)), theme.active_line));
        }
        for deco in &self.decorations {
            if let Some(color) = deco.background {
                span(&deco.range, color, &mut quads);
            }
        }
        let selected = if snap.mode == Mode::Insert { !snap.selection.is_empty() } else { snap.selection.is_wide(text) };
        if selected {
            span(&snap.selection.bytes(), theme.selection, &mut quads);
        }
        quads
    }

    /// Clicks and drags place the cursor; the wheel scrolls.
    fn listen(&self, bounds: Bounds<Pixels>, state: &Prepainted, window: &mut Window) {
        let lines = state.lines.clone();
        let (text_bounds, scroll_left, line_height) = (state.text_bounds, state.scroll_left, state.line_height);
        let hit = move |position: Point<Pixels>| -> Option<usize> {
            let laid = lines
                .iter()
                .find(|l| position.y >= l.y && position.y < l.y + line_height)
                .or_else(|| if position.y < bounds.top() { lines.first() } else { lines.last() })?;
            Some(laid.byte_at_x(position.x - text_bounds.left() + scroll_left))
        };
        let hit_drag = hit.clone();
        let (editor, focus) = (self.editor.clone(), self.focus.clone());
        window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
            if phase != DispatchPhase::Bubble || event.button != MouseButton::Left || !text_bounds.contains(&event.position) {
                return;
            }
            let Some(at) = hit(event.position) else { return };
            window.focus(&focus);
            editor.update(cx, |editor, cx| editor.click(at, event.modifiers.shift, event.click_count, cx));
        });
        let editor = self.editor.clone();
        window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
            if phase != DispatchPhase::Bubble || event.pressed_button != Some(MouseButton::Left) {
                return;
            }
            let Some(at) = hit_drag(event.position) else { return };
            editor.update(cx, |editor, cx| editor.drag_to(at, cx));
        });
        let editor = self.editor.clone();
        window.on_mouse_event(move |event: &ScrollWheelEvent, phase, _, cx| {
            if phase != DispatchPhase::Bubble || !bounds.contains(&event.position) {
                return;
            }
            let delta = event.delta.pixel_delta(line_height);
            editor.update(cx, |editor, cx| editor.scroll_by(delta, cx));
        });
    }
}

/// Keep the cursor in view sideways when asked to show it.
fn scroll_sideways(snap: &Snapshot, lines: &[LaidLine], metrics: &Metrics, text_bounds: Bounds<Pixels>) -> Pixels {
    let Some(laid) = lines.iter().find(|l| l.start <= snap.cursor && snap.cursor <= l.end).filter(|_| snap.autoscroll.is_some()) else {
        return snap.scroll_left;
    };
    let x = laid.x(snap.cursor);
    let room = text_bounds.size.width - metrics.char_width * 4.;
    if x < snap.scroll_left {
        (x - metrics.char_width * 4.).max(px(0.))
    } else if x > snap.scroll_left + room {
        x - room
    } else {
        snap.scroll_left
    }
}

/// The cursor: a filled cell with its char redrawn on top, or a bar in
/// insert mode. Returns where it is (for popups and input methods) and the
/// redrawn char; its fill goes into `quads`.
#[allow(clippy::too_many_arguments)]
fn cursor(
    snap: &Snapshot,
    lines: &[LaidLine],
    text: &str,
    metrics: &Metrics,
    theme: &Theme,
    origin_x: Pixels,
    focused: bool,
    quads: &mut Vec<PaintQuad>,
    window: &mut Window,
) -> (Option<Bounds<Pixels>>, Option<Glyph>) {
    let Some(laid) = lines.iter().find(|l| l.start <= snap.cursor && snap.cursor <= l.end) else { return (None, None) };
    let x = origin_x + laid.x(snap.cursor);
    let next = ghostwrite_core::next_char(text, snap.cursor).min(laid.end);
    let width = if next > snap.cursor { (origin_x + laid.x(next) - x).max(metrics.char_width * 0.5) } else { metrics.char_width };
    let cell = Bounds::new(point(x, laid.y), gpui::size(width, metrics.line_height));
    if !focused || snap.mode == Mode::Tree {
        return (Some(cell), None);
    }
    let color = match snap.mode {
        Mode::Insert => theme.cursor_insert,
        Mode::Select => theme.cursor_select,
        _ => theme.cursor_normal,
    };
    if snap.mode == Mode::Insert {
        quads.push(fill(Bounds::new(cell.origin, gpui::size(px(2.), metrics.line_height)), color));
        return (Some(cell), None);
    }
    quads.push(fill(cell, color));
    let glyph = text[snap.cursor..].chars().next().filter(|c| !c.is_whitespace()).map(|c| {
        let glyph: SharedString = c.to_string().into();
        let shaped = window.text_system().shape_line(glyph.clone(), metrics.size, &[metrics.run(glyph.len(), theme.cursor_text)], None);
        (shaped, cell.origin)
    });
    (Some(cell), glyph)
}

/// `text` with tabs expanded, and where each byte lands in it.
fn expand_tabs(text: &str) -> (String, Option<Vec<usize>>) {
    if !text.contains('\t') {
        return (text.to_string(), None);
    }
    let mut out = String::with_capacity(text.len() + 8);
    let mut map = Vec::with_capacity(text.len() + 1);
    let mut column = 0;
    for (i, c) in text.char_indices() {
        map.resize(i + 1, out.len());
        if c == '\t' {
            let tab_width = crate::config::get().tab_width;
            let spaces = tab_width - column % tab_width;
            out.extend(std::iter::repeat_n(' ', spaces));
            column += spaces;
        } else {
            out.push(c);
            column += 1;
        }
    }
    map.resize(text.len() + 1, out.len());
    (out, Some(map))
}

/// The styled runs of one line, over the shown text: syntax colors, marks
/// that strike or recolor, and diagnostics' wavy underlines.
#[allow(clippy::too_many_arguments)]
fn line_runs(
    line: Bytes<usize>,
    map: &Option<Vec<usize>>,
    spans: &[ghostwrite_core::syntax::Span],
    decorations: &[Decoration],
    diagnostics: &[ghostwrite_lsp::Diagnostic],
    theme: &Theme,
    base: &Font,
) -> Vec<TextRun> {
    if line.is_empty() {
        return vec![];
    }
    let first_span = spans.partition_point(|s| s.range.end <= line.start);
    let spans = spans[first_span..].iter().take_while(|s| s.range.start < line.end);
    let styling: Vec<&Decoration> = decorations.iter().filter(|d| d.strike.is_some() || d.color.is_some()).collect();
    let mut cuts: Vec<usize> = vec![line.start, line.end];
    let ranges = spans.clone().map(|s| &s.range).chain(styling.iter().map(|d| &d.range)).chain(diagnostics.iter().map(|d| &d.range));
    for r in ranges {
        cuts.extend([r.start, r.end].into_iter().filter(|&b| b > line.start && b < line.end));
    }
    cuts.sort_unstable();
    cuts.dedup();
    let shown = |b: usize| map.as_ref().map_or(b - line.start, |m| m[b - line.start]);
    let covers = |r: &Bytes<usize>, at: usize| r.start <= at && at < r.end.max(r.start + 1);
    cuts.windows(2)
        .map(|w| {
            let at = w[0];
            let syntax = spans.clone().find(|s| covers(&s.range, at)).and_then(|s| theme.syntax_style(SCOPES[s.scope]));
            let deco = styling.iter().find(|d| covers(&d.range, at));
            let diag = diagnostics.iter().filter(|d| covers(&d.range, at)).min_by_key(|d| d.severity);
            let mut font = base.clone();
            font.weight = syntax.as_ref().map_or(gpui::FontWeight::NORMAL, Theme::weight);
            if syntax.is_some_and(|s| s.italic) {
                font.style = gpui::FontStyle::Italic;
            }
            let underline = diag.map(|d| match d.severity {
                Severity::Error => theme.danger,
                Severity::Warning => theme.warning,
                Severity::Info => theme.info,
                Severity::Hint => theme.hint,
            });
            TextRun {
                len: shown(w[1]) - shown(w[0]),
                font,
                color: deco.and_then(|d| d.color).or(syntax.and_then(|s| s.color)).unwrap_or(theme.foreground),
                background_color: None,
                underline: underline.map(|color| UnderlineStyle { color: Some(color), thickness: px(1.), wavy: true }),
                strikethrough: deco.and_then(|d| d.strike).map(|color| StrikethroughStyle { thickness: px(1.), color: Some(color) }),
            }
        })
        .filter(|run| run.len > 0)
        .collect()
}

impl Editor {
    /// A click in the text: put the cursor there (shift extends; a double click selects the word).
    fn click(&mut self, at: usize, extend: bool, clicks: usize, cx: &mut gpui::Context<Self>) {
        if self.modal.mode == Mode::Tree {
            self.modal.mode = Mode::Normal;
        }
        let insert = self.modal.mode == Mode::Insert;
        let Some(tab) = self.tab_mut() else { return };
        let text = tab.doc.text();
        tab.view.selection = if clicks >= 2 {
            ghostwrite_core::textobject::text_object(&tab.doc.buffer, Range::point(at), 'w', false).unwrap_or(Range::point(at))
        } else if extend {
            Range::new(tab.view.selection.anchor, if insert { at } else { ghostwrite_core::next_char(text, at) })
        } else {
            Range::point(at)
        };
        self.completion = None;
        cx.notify();
    }

    fn drag_to(&mut self, at: usize, cx: &mut gpui::Context<Self>) {
        let insert = self.modal.mode == Mode::Insert;
        let Some(tab) = self.tab_mut() else { return };
        let anchor = tab.view.selection.anchor;
        let head = if insert || at < anchor { at } else { ghostwrite_core::next_char(tab.doc.text(), at) };
        tab.select(Range::new(anchor, head), Autoscroll::Fit);
        cx.notify();
    }

    fn scroll_by(&mut self, delta: Point<Pixels>, cx: &mut gpui::Context<Self>) {
        let Some(tab) = self.tab_mut() else { return };
        tab.view.scroll_top = (tab.view.scroll_top - delta.y).max(px(0.));
        tab.view.scroll_left = (tab.view.scroll_left - delta.x).max(px(0.));
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::{Geometry, expand_tabs};
    use gpui::px;

    #[test]
    fn tabs_expand_to_stops() {
        assert_eq!(expand_tabs("ab"), ("ab".into(), None));
        let (shown, map) = expand_tabs("\tx\ty");
        assert_eq!(shown, "    x   y");
        assert_eq!(map.unwrap(), [0, 4, 5, 8, 9]);
    }

    #[test]
    fn the_block_pushes_lines_below_it_down() {
        let g = Geometry { line_height: px(20.), block: Some((2, px(50.))) };
        assert_eq!(g.top(2), px(40.));
        assert_eq!(g.bottom(2), px(110.), "the block hangs from its line");
        assert_eq!(g.top(3), px(110.));
    }
}
