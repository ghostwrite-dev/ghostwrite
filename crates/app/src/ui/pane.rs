//! A pane's size, changed by dragging one of its edges (the file tree's
//! right edge, the side pane's left, the terminal's top), and kept between
//! launches. Sizes are in points at 100%, so a pane zooms with its text.

use std::ops::RangeInclusive;

use ghostwrite_project::store;
use gpui::{Div, Pixels, Point, Stateful, div, prelude::*};

use crate::theme::pt;

/// Which of its edges drags.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    Left,
    Right,
    Top,
}

pub struct Resizable {
    edge: Edge,
    /// Where it's kept.
    key: &'static str,
    /// `None`: as the layout has it, until it's dragged.
    pub size: Option<f32>,
    /// While the edge is dragged: where the pointer started, and the size.
    drag: Option<(f32, f32)>,
}

impl Resizable {
    /// The size kept as `key`, else `default`.
    pub fn new(key: &'static str, edge: Edge, default: Option<f32>) -> Self {
        let kept = store::read(key).and_then(|s| s.trim().parse().ok());
        Self { edge, key, size: kept.or(default), drag: None }
    }

    pub fn dragging(&self) -> bool {
        self.drag.is_some()
    }

    /// Where `at` is, along the way the edge moves.
    fn along(&self, at: Point<Pixels>) -> f32 {
        f32::from(if self.edge == Edge::Top { at.y } else { at.x })
    }

    /// The edge was grabbed at `at`, the pane `size` points big as it's drawn.
    pub fn grab(&mut self, at: Point<Pixels>, size: f32) {
        self.drag = Some((self.along(at), size));
    }

    /// The pointer moved to `at`, zoomed by `zoom`: the size follows, within
    /// `range` points. Whether it changed.
    pub fn drag(&mut self, at: Point<Pixels>, zoom: f32, range: RangeInclusive<f32>) -> bool {
        let Some((from, size)) = self.drag else { return false };
        let moved = (self.along(at) - from) / zoom;
        let grown = if self.edge == Edge::Right { moved } else { -moved };
        self.size = Some((size + grown).clamp(*range.start(), range.end().max(*range.start())));
        true
    }

    /// The edge was let go: the size is kept.
    pub fn release(&mut self) {
        if self.drag.take().is_some()
            && let Some(size) = self.size
        {
            store::write(self.key, &size.to_string());
        }
    }

    /// The strip along the edge to grab, for the pane to put in itself (it's
    /// `relative`): the caller says what a press on it does.
    pub fn handle(&self, id: &'static str) -> Stateful<Div> {
        let strip = div().id(id).absolute();
        match self.edge {
            Edge::Left => strip.top_0().left(pt(-3.)).w(pt(6.)).h_full().cursor_col_resize(),
            Edge::Right => strip.top_0().right(pt(-3.)).w(pt(6.)).h_full().cursor_col_resize(),
            Edge::Top => strip.left_0().top(pt(-3.)).w_full().h(pt(6.)).cursor_row_resize(),
        }
    }
}
