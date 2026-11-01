//! Where to look next. When a story moves on to another panel (another file, the file tree,
//! the terminal), that panel lights up for a moment, so the eye goes there. `"animations":
//! false` in the config turns it off.

use std::time::Duration;

use gpui::{Animation, AnimationExt as _, AnyElement, Context, div, prelude::*};

use super::Editor;
use crate::theme::theme;

const FADES_IN: Duration = Duration::from_millis(900);

/// A place a story can be, that isn't where it was.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum Panel {
    #[default]
    Code,
    Tree,
    Terminal,
}

#[derive(Default)]
pub(super) struct Attention {
    /// Where the story last was.
    pub(super) on: Panel,
    /// What's lit up, and how many times anything has been, so that it starts over each time.
    pub(super) lit: Option<(Panel, u64)>,
    times: u64,
}

impl Editor {
    /// The story is now at `panel`: if that's somewhere new, or `anyway` (another file, in the
    /// same panel), light it up.
    pub(super) fn look_at(&mut self, panel: Panel, anyway: bool, cx: &mut Context<Self>) {
        let attention = &mut self.attention;
        if attention.on != panel || anyway {
            attention.times += 1;
            attention.lit = Some((panel, attention.times));
        }
        attention.on = panel;
        cx.notify();
    }

    /// Over `panel`, if it's lit up. Its parent has to be `relative`. It has no listeners, so
    /// the mouse goes straight through it.
    pub(super) fn render_attention(&self, panel: Panel, cx: &Context<Self>) -> Option<AnyElement> {
        let (lit, times) = self.attention.lit?;
        if lit != panel || !crate::config::get().animations {
            return None;
        }
        let accent = theme(cx).accent;
        let name = match panel {
            Panel::Code => "attention-code",
            Panel::Tree => "attention-tree",
            Panel::Terminal => "attention-terminal",
        };
        let light = div().absolute().inset_0().border_2().border_color(accent).bg(accent.opacity(0.12));
        // A new id each time: the animation is of an element, and only starts once.
        let fading =
            light.with_animation(gpui::ElementId::NamedInteger(name.into(), times), Animation::new(FADES_IN).with_easing(gpui::ease_in_out), |light, done| {
                light.opacity(1. - done)
            });
        Some(div().absolute().inset_0().debug_selector(move || name.to_string()).child(fading).into_any_element())
    }
}
