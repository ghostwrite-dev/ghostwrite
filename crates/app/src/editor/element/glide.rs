//! How the view glides to where the keys scrolled it, and the cursor to
//! where it went: eased, and only while animations are on.

use std::time::{Duration, Instant};

use gpui::{Pixels, Point, point, px};

use super::Snapshot;
use crate::editor::tabs::{self, Motion};

/// How long the view glides to where the keys scrolled it, the cursor to where
/// it went, and changed lines take to fade.
const SCROLL_GLIDE: Duration = Duration::from_millis(160);
const CURSOR_GLIDE: Duration = Duration::from_millis(80);
pub(super) const FLASH: Duration = Duration::from_millis(900);

/// How far along something started `since` and taking `over` is, if it's still under way.
pub(super) fn progress(since: Instant, over: Duration) -> Option<f32> {
    let done = since.elapsed().as_secs_f32() / over.as_secs_f32();
    (done < 1.).then_some(done)
}

/// Quick at first, settling at the end.
fn ease_out(done: f32) -> f32 {
    1. - (1. - done).powi(3)
}

type Glide<T> = Option<(T, T, Instant)>;

/// A place in the editor's state, on screen.
pub(super) fn pixels(at: tabs::Point) -> Point<Pixels> {
    point(px(at.x), px(at.y))
}

/// A place on screen, for the editor's state.
pub(super) fn points(at: Point<Pixels>) -> tabs::Point {
    tabs::Point { x: at.x.into(), y: at.y.into() }
}

/// A glide on screen, for the editor's state.
pub(super) fn kept<T, U>(glide: Glide<T>, f: impl Fn(T) -> U) -> Glide<U> {
    glide.map(|(from, to, since)| (f(from), f(to), since))
}

/// Where the text is drawn scrolled to on its way to `target`, and the glide
/// still under way. The keys moving it more than a couple of lines glide it
/// there from where it's drawn (from a screen away at most: further, the rest
/// is a jump); the wheel and the trackpad move it straight there.
pub(super) fn glide_scroll(snap: &Snapshot, target: Pixels, view_height: Pixels, line_height: Pixels, animate: bool) -> (Pixels, Glide<Pixels>) {
    let mut glide = kept(snap.motion.scroll, px).filter(|(_, to, _)| *to == target);
    if snap.autoscroll.is_some() && target != snap.scroll_top {
        let from = px(snap.motion.drawn_top).clamp(target - view_height, target + view_height);
        glide = (animate && (target - from).abs() > line_height * 2.).then(|| (from, target, Instant::now()));
    }
    match glide.and_then(|(from, to, since)| Some((from, to, since, progress(since, SCROLL_GLIDE)?))) {
        Some((from, to, since, done)) => (from + (to - from) * ease_out(done), Some((from, to, since))),
        None => (target, None),
    }
}

/// Where the cursor is drawn on its way to `to` (in the text), and the glide still under way.
pub(super) fn glide_cursor(motion: &Motion, to: Point<Pixels>, animate: bool) -> (Point<Pixels>, Glide<Point<Pixels>>) {
    let at = |(from, to, since): (Point<Pixels>, Point<Pixels>, Instant)| {
        progress(since, CURSOR_GLIDE).map(|done| point(from.x + (to.x - from.x) * ease_out(done), from.y + (to.y - from.y) * ease_out(done)))
    };
    let cursor_glide = kept(motion.cursor_glide, pixels);
    let mut glide = cursor_glide.filter(|(_, end, _)| *end == to);
    if let Some(was) = motion.cursor.map(pixels).filter(|was| *was != to) {
        let from = cursor_glide.and_then(at).unwrap_or(was);
        glide = animate.then(|| (from, to, Instant::now()));
    }
    match glide.and_then(|glide| Some((at(glide)?, glide))) {
        Some((drawn, glide)) => (drawn, Some(glide)),
        None => (to, None),
    }
}

#[cfg(test)]
mod tests {
    use ghostwrite_core::{Mode, Range};
    use gpui::px;

    use super::*;
    use crate::editor::tabs::Autoscroll;

    fn at(top: Pixels, autoscroll: Option<Autoscroll>, motion: Motion) -> Snapshot {
        Snapshot {
            mode: Mode::Normal,
            line_count: 100,
            selection: Range::point(0),
            others: vec![],
            autoscroll,
            scroll_top: top,
            cursor: 0,
            cursor_line: 0,
            elsewhere: false,
            motion,
        }
    }

    #[test]
    fn the_keys_scrolling_far_glide_there_and_the_wheel_doesnt() {
        let (line, view) = (px(20.), px(200.));
        // `ctrl-d`: half a screen down, from the top.
        let (drawn, glide) = glide_scroll(&at(px(0.), Some(Autoscroll::Fit), Motion::default()), px(100.), view, line, true);
        assert!(glide.is_some() && drawn < px(100.), "on its way: {drawn:?}");
        // The next frame keeps going to the same place.
        let motion = Motion { drawn_top: drawn.into(), scroll: kept(glide, f32::from), ..Motion::default() };
        assert_eq!(glide_scroll(&at(px(100.), None, motion.clone()), px(100.), view, line, true).1, glide);
        // The wheel scrolls somewhere else meanwhile: straight there.
        assert_eq!(glide_scroll(&at(px(140.), None, motion), px(140.), view, line, true), (px(140.), None));
        // A line or two (`j` at the edge) isn't worth gliding; nor anything with animations off.
        assert_eq!(glide_scroll(&at(px(0.), Some(Autoscroll::Fit), Motion::default()), px(20.), view, line, true), (px(20.), None));
        assert_eq!(glide_scroll(&at(px(0.), Some(Autoscroll::Fit), Motion::default()), px(100.), view, line, false), (px(100.), None));
        // From far away (`ge` in a long file), only the last screen of the way is seen.
        let (drawn, glide) = glide_scroll(&at(px(0.), Some(Autoscroll::Fit), Motion::default()), px(5000.), view, line, true);
        assert!(drawn >= px(4800.) && glide.is_some_and(|(from, ..)| from == px(4800.)), "{drawn:?}");
    }

    #[test]
    fn the_cursor_slides_from_where_it_was_drawn() {
        let (was, to) = (point(px(0.), px(0.)), point(px(80.), px(40.)));
        let (drawn, glide) = glide_cursor(&Motion { cursor: Some(points(was)), ..Motion::default() }, to, true);
        assert!(glide.is_some() && drawn != to, "on its way");
        assert_eq!(glide_cursor(&Motion { cursor: Some(points(to)), ..Motion::default() }, to, true), (to, None), "not moving");
        assert_eq!(glide_cursor(&Motion { cursor: Some(points(was)), ..Motion::default() }, to, false), (to, None), "animations off");
        assert_eq!(glide_cursor(&Motion::default(), to, true), (to, None), "nowhere to come from");
    }
}
