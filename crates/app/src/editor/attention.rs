//! Where to look next. Wherever a story's step is shown (lines of code, a proposal in the file
//! tree, a command in the terminal, or the model thinking, or a command running, in the corner), it says where it is
//! on screen ([`Editor::seen`]). When the story moves on to a step, a light stretches from where
//! it was to where it is now, its leading edge first and the rest catching up, and settles there,
//! so the eye follows it without looking for it. `"animations": false` in the config turns
//! it off.

use std::time::{Duration, Instant};

use gpui::{AnimationExt as _, AnyElement, App, Bounds, Context, Entity, Hsla, Pixels, Window, canvas, div, point, prelude::*, px, size};

use super::{DocId, Editor};
use crate::theme::{pt, theme};

/// How long the light takes from one place to the next, and then to settle there.
const TRAVEL: Duration = Duration::from_millis(420);
const SETTLE: Duration = Duration::from_millis(380);

/// A place a story can be, that isn't where it was.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum Panel {
    #[default]
    Code,
    Tree,
    Terminal,
}

/// What of the story is shown: the prompt, the model thinking, one of its steps, or that
/// one's ready while the user looks elsewhere (in the corner, until Tab brings them to it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Mark {
    Asking,
    Thinking,
    Item(usize),
    Ready(usize),
    /// Where the user was, as Tab brings them back to the step.
    Away,
}

/// Where what's shown of the story is on screen, this frame.
#[derive(Debug, Clone, Copy)]
pub(super) struct Seen {
    pub mark: Mark,
    pub rect: Bounds<Pixels>,
    /// The document it's in, how far that's scrolled, and the view it's in: the light from
    /// another place in the same file comes from where that place has scrolled to.
    pub scrolled: Option<(DocId, Pixels, Bounds<Pixels>)>,
}

/// The light on its way, from and to places on screen.
#[derive(Debug, Clone, Copy)]
pub(super) struct Jump {
    pub from: Bounds<Pixels>,
    pub to: Bounds<Pixels>,
    started: Instant,
}

#[derive(Default)]
pub(super) struct Attention {
    /// Where the story last was.
    pub(super) on: Panel,
    /// Where what's shown of it was last seen.
    last: Option<Seen>,
    pub(super) jump: Option<Jump>,
    /// How many times it's jumped.
    pub(super) jumps: u64,
}

impl Editor {
    /// The story is now at `panel`.
    pub(super) fn look_at(&mut self, panel: Panel, _anyway: bool, cx: &mut Context<Self>) {
        self.attention.on = panel;
        cx.notify();
    }

    /// What of the story should be on screen now, if there's a story.
    pub(super) fn story_mark(&self) -> Option<Mark> {
        let state = self.story.as_ref()?;
        Some(match state.story.as_ref().and_then(|story| story.current()) {
            Some(id) if state.wandered && !state.showing(id) => Mark::Ready(id.0),
            Some(id) => Mark::Item(id.0),
            None if self.asking() || state.story.is_none() => Mark::Asking,
            None => Mark::Thinking,
        })
    }

    /// What of the story is shown, and where, as it's drawn: if that's moved on from what
    /// was, the light goes from there to here. Only what should be shown counts.
    pub(super) fn seen(&mut self, seen: Seen, window: &mut Window) {
        if self.story_mark() != Some(seen.mark) {
            return;
        }
        let attention = &mut self.attention;
        // Going to think, in the corner, isn't somewhere to look.
        if let Some(last) = attention.last.filter(|last| last.mark != seen.mark)
            && seen.mark != Mark::Thinking
            && crate::config::get().animations
        {
            attention.jump = Some(Jump { from: came_from(&last, &seen), to: seen.rect, started: Instant::now() });
            attention.jumps += 1;
            window.request_animation_frame();
        } else if let Some(jump) = attention.jump.as_mut().filter(|_| attention.last.is_some_and(|last| last.mark == seen.mark)) {
            // Still the same place, moving as the view glides there: the light follows it.
            jump.to = seen.rect;
        }
        attention.last = Some(seen);
    }

    /// What of the story the code, the tree or the terminal shows, where it's at: the
    /// prompt or a step (thinking, or a step that waits, is in the corner).
    pub(super) fn step_mark(&self) -> Option<Mark> {
        self.story_mark().filter(|mark| matches!(mark, Mark::Asking | Mark::Item(_)))
    }

    /// Tab brings the user back to the step: the light goes there from `rect`, where they were.
    pub(super) fn came_back_from(&mut self, rect: Bounds<Pixels>) {
        self.attention.last = Some(Seen { mark: Mark::Away, rect, scrolled: None });
    }

    /// The story's over: there's nowhere to jump from.
    pub(super) fn forget_where_the_story_was(&mut self) {
        self.attention.last = None;
    }

    /// The light, over everything, while it's on its way or settling. Its parent has to be
    /// `relative`; the mouse goes straight through it.
    pub(super) fn render_jump(&self, cx: &Context<Self>) -> AnyElement {
        let editor = cx.entity();
        let accent = theme(cx).accent;
        canvas(|_, _, _| {}, move |_, _, window, cx| paint_jump(&editor, accent, window, cx)).absolute().inset_0().into_any_element()
    }

    /// The model thinking, or what it's looking at, or what went wrong, in the top right of the
    /// code: not on any line, as it isn't about one yet.
    pub(super) fn render_thinking(&self, cx: &Context<Self>) -> Option<AnyElement> {
        let state = self.story.as_ref()?;
        let story = state.story.as_ref()?;
        let theme = theme(cx);
        let mark = self.story_mark()?;
        let (said, color, mark) = match mark {
            Mark::Thinking if let Some(path) = &state.asking_to_read => {
                (format!("read {}? ⇥ yes · ⏎ no", ghostwrite_ai::tools::shown(path)), theme.foreground, Some(mark))
            }
            // The story's command is running: what's next is asked once it's done.
            Mark::Thinking if let Some(running) = &self.terminal.running => {
                (format!("{}{}", running.doing, super::story_view::dots()), theme.muted_foreground, Some(mark))
            }
            Mark::Thinking if story.error.is_some() => (super::story_view::waiting_hint(Some(story)), theme.danger, Some(mark)),
            Mark::Thinking => (super::story_view::waiting_hint(Some(story)), theme.muted_foreground, Some(mark)),
            Mark::Ready(_) => ("the next step's ready · ⇥ to go to it".to_string(), theme.foreground, Some(mark)),
            // Looked away from the step: Tab comes back to it.
            Mark::Item(_) if state.wandered => ("⇥ back to the step".to_string(), theme.muted_foreground, None),
            // A command that can't go where it would (a program's running there): here, what Tab does instead.
            Mark::Item(_) if let Some(elsewhere) = self.command_elsewhere() => (elsewhere, theme.foreground, Some(mark)),
            _ => return None,
        };
        // What it's thinking about, as the user said it, over what it's doing.
        let asked = state.said.as_deref().filter(|_| matches!(mark, Some(Mark::Thinking)) && story.error.is_none()).map(|said| {
            let said = said.lines().next().unwrap_or_default();
            let short: String = said.chars().take(60).collect();
            let short = if short.len() < said.len() { format!("“{}…”", short.trim_end()) } else { format!("“{short}”") };
            div().min_w_0().overflow_hidden().whitespace_nowrap().text_color(theme.foreground).child(short)
        });
        let editor = cx.entity();
        let glyph = div().text_color(theme.warning).child("✦").with_animation(
            "thinking",
            gpui::Animation::new(Duration::from_millis(1400)).repeat().with_easing(gpui::ease_in_out),
            |glyph, done| glyph.opacity(0.45 + 0.55 * (1. - (done * 2. - 1.).abs())),
        );
        let doing = div().flex().items_center().gap(pt(6.)).child(glyph);
        let doing = doing
            .child(div().min_w_0().overflow_hidden().whitespace_nowrap().text_color(color).child(said.trim_end_matches('.').to_string()))
            // The dots come and go in a place of their own, so the box keeps its width.
            .children(said.ends_with('.').then(|| {
                let dots = said.len() - said.trim_end_matches('.').len();
                div().flex_none().w(pt(14.)).whitespace_nowrap().text_color(color).child(".".repeat(dots))
            }));
        Some(
            div()
                .absolute()
                .top(pt(8.))
                .right(pt(12.))
                .max_w(pt(420.))
                .flex()
                .flex_col()
                .gap(pt(2.))
                .px(pt(8.))
                .py(pt(3.))
                .rounded(pt(6.))
                .bg(theme.popover)
                .border_1()
                .border_color(theme.border)
                .text_xs()
                .debug_selector(|| "story-thinking".to_string())
                .children(asked)
                .child(doing)
                .children(mark.map(|mark| marker(editor, mark)))
                .into_any_element(),
        )
    }
}

/// An element filling its parent (which has to be `relative`) that says where that is, as `mark`.
pub(super) fn marker(editor: Entity<Editor>, mark: Mark) -> AnyElement {
    canvas(
        move |bounds, window, cx| {
            editor.update(cx, |editor, _| editor.seen(Seen { mark, rect: bounds, scrolled: None }, window));
        },
        |_, _, _, _| {},
    )
    .absolute()
    .inset_0()
    .into_any_element()
}

/// Where the light starts: where the last place was, or, in the same file scrolled since,
/// where it's scrolled to, at the edge of the view if it's out of it, so the light comes
/// in from the way the file went.
fn came_from(last: &Seen, now: &Seen) -> Bounds<Pixels> {
    match (last.scrolled, now.scrolled) {
        (Some((doc, was, _)), Some((same, is, view))) if doc == same => {
            let moved = Bounds::new(point(last.rect.left(), last.rect.top() + (was - is)), last.rect.size);
            let sliver = px(6.);
            let top = moved.top().clamp(view.top() - moved.size.height + sliver, view.bottom() - sliver);
            Bounds::new(point(moved.left(), top), moved.size)
        }
        _ => last.rect,
    }
}

fn paint_jump(editor: &Entity<Editor>, accent: Hsla, window: &mut Window, cx: &mut App) {
    let Some(jump) = editor.read(cx).attention.jump else { return };
    let elapsed = jump.started.elapsed();
    if elapsed >= TRAVEL + SETTLE {
        editor.update(cx, |editor, _| editor.attention.jump = None);
        return;
    }
    let (rect, strength) = if elapsed < TRAVEL {
        let t = elapsed.as_secs_f32() / TRAVEL.as_secs_f32();
        (stretched(jump.from, jump.to, t), (t * 6.).min(1.))
    } else {
        let t = (elapsed - TRAVEL).as_secs_f32() / SETTLE.as_secs_f32();
        (jump.to, 1. - gpui::ease_in_out(t))
    };
    let corners = gpui::Corners::all(px(6.));
    window.paint_quad(gpui::quad(rect, corners, accent.opacity(0.10 * strength), px(1.5), accent.opacity(0.6 * strength), gpui::BorderStyle::Solid));
    window.request_animation_frame();
}

/// The light `t` of the way from `from` to `to`: the edges it's going towards lead, and the
/// others catch up, so it stretches out on its way and gathers in where it lands.
fn stretched(from: Bounds<Pixels>, to: Bounds<Pixels>, t: f32) -> Bounds<Pixels> {
    let lead = 1. - (1. - (t * 1.3).min(1.)).powi(3);
    let trail = gpui::ease_in_out(t);
    let between = |a: Pixels, b: Pixels, k: f32| a + (b - a) * k;
    let down = to.center().y >= from.center().y;
    let right = to.center().x >= from.center().x;
    let top = between(from.top(), to.top(), if down { trail } else { lead });
    let bottom = between(from.bottom(), to.bottom(), if down { lead } else { trail });
    let left = between(from.left(), to.left(), if right { trail } else { lead });
    let right_edge = between(from.right(), to.right(), if right { lead } else { trail });
    Bounds::new(point(left, top), size((right_edge - left).max(px(1.)), (bottom - top).max(px(1.))))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: f32, y: f32, w: f32, h: f32) -> Bounds<Pixels> {
        Bounds::new(point(px(x), px(y)), size(px(w), px(h)))
    }

    #[test]
    fn the_light_stretches_towards_where_its_going() {
        let (from, to) = (rect(0., 0., 100., 20.), rect(0., 400., 100., 20.));
        assert_eq!(stretched(from, to, 0.), from);
        assert_eq!(stretched(from, to, 1.), to);
        let halfway = stretched(from, to, 0.4);
        assert!(halfway.size.height > px(200.), "longer on its way: {halfway:?}");
        assert!(halfway.bottom() > halfway.top() + px(100.) && halfway.top() < px(200.), "its leading edge ahead: {halfway:?}");
        let up = stretched(to, from, 0.4);
        assert!(up.top() < px(200.) && up.bottom() > px(200.), "and the other way: {up:?}");
    }

    #[test]
    fn in_the_same_file_it_comes_from_where_the_last_place_scrolled_to() {
        let view = rect(0., 0., 800., 600.);
        let last = Seen { mark: Mark::Item(0), rect: rect(0., 300., 800., 40.), scrolled: Some((1, px(0.), view)) };
        let now = Seen { mark: Mark::Item(1), rect: rect(0., 300., 800., 40.), scrolled: Some((1, px(2000.), view)) };
        let from = came_from(&last, &now);
        assert!(from.bottom() <= px(6.) + px(0.01) && from.bottom() > px(0.), "scrolled up out of view, it comes in from the top: {from:?}");
        let elsewhere = Seen { scrolled: Some((2, px(2000.), view)), ..now };
        assert_eq!(came_from(&last, &elsewhere), last.rect, "another file: from where it was");
    }
}
