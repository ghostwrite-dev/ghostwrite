//! Drawing a story: the current item folded out under its lines (a step's
//! removed lines struck through and its new lines as ghost text, or a note
//! under the lines it's about), its keys on a faint line below, and the
//! prompt when it's open. What's decided in the file tree shows there.
//!
//! The block is described as rows of text; the code view builds it with the
//! sizes it measures as it lays out, so wrapping always fits this frame.

use ghostwrite_ai::placement;
use ghostwrite_ai::protocol::{self, Note};
use ghostwrite_ai::{Item, Story};
use gpui::{AnyElement, Entity, Hsla, div, prelude::*, px};

use super::element::{Block, BlockSizes, Decoration};
use super::story::{Located, StoryState};
use super::tree::Annex;
use super::{Project, Tab};
use crate::theme::Theme;
use crate::ui::text_field::TextField;

/// A row of the block.
enum Row {
    /// Suggested code, wrapped at the edge with continuations indented.
    Code(String),
    /// A note's text, word-wrapped right of the indentation.
    Prose(String),
    /// What the keys do (or what's happening), in a color.
    Hint(String, Hsla),
}

/// What folds out, before it's sized.
struct BlockSpec {
    /// Columns of indentation for prose, hints and the prompt.
    indent: usize,
    rows: Vec<Row>,
    /// The prompt, if it's open here, and its hint.
    prompt: Option<(Entity<TextField>, &'static str)>,
}

impl StoryState {
    /// What shows under a proposal in the tree, if the story is at one.
    pub(super) fn annex(&self, project: &Project, asking: bool) -> Option<Annex> {
        let story = self.story.as_ref()?;
        let id = story.current()?;
        let item = story.item(id)?;
        let about = story.asked_about(id).and_then(|a| story.item(a)).filter(Item::is_file_op);
        let (note, keys) = match (&item, &about) {
            (Item::Note(note), Some(_)) => (Some(note.text.clone()), "⇥ next · ⏎ ask".to_string()),
            (Item::Create(_), _) => (None, "⇥ create · ⌫ skip · ⏎ ask or redirect".into()),
            (Item::Remove(_), _) => (None, "⇥ remove · ⌫ keep · ⏎ ask or redirect".into()),
            (Item::Rename { .. }, _) => (None, "⇥ rename · ⌫ keep · ⏎ ask or redirect".into()),
            (Item::Step(step), _) if self.proposal(project, id, &item).is_some() => {
                let first = step.new.first().map_or(String::new(), |l| format!(": {}", l.trim()));
                (Some(format!("starts with{first}")), "⇥ create it and write · ⌫ skip · ⏎ redirect".into())
            }
            _ => return None,
        };
        Some(Annex { note, keys, asking })
    }

    /// What the story draws in `tab`: marks on its lines, and the block that
    /// folds out under them. `ask` is the prompt's field while it's open.
    pub(super) fn overlay(&self, project: &Project, tab: &Tab, ask: Option<Entity<TextField>>, theme: &Theme) -> (Vec<Decoration>, Option<Block>) {
        if self.annex(project, ask.is_some()).is_some() {
            return (vec![], None);
        }
        let doc = &tab.doc;
        let story = self.story.as_ref();
        let prompt = |hint| ask.clone().map(|field| (field, hint));
        let indent_of = |line: usize| {
            let text = doc.buffer.line(line);
            let lead = &text[..text.len() - text.trim_start_matches([' ', '\t']).len()];
            lead.len() + lead.matches('\t').count() * 3
        };
        let shown_here = |located: &Located| located.tab.and_then(|ix| project.tabs.get(ix)).is_some_and(|t| t.doc.id == doc.id);

        // Before asking, or waiting for the reply: where the question was
        // asked (an item under review), else under the marked lines.
        let current = story.and_then(|s| Some((s.current()?, s.current_item()?)));
        let Some((id, item)) = current else {
            let asked_at = story.and_then(Story::writing_about).and_then(|about| self.lines_of(project, about));
            let (line, indent) = match &asked_at {
                Some(located) if shown_here(located) => (located.near.end.saturating_sub(1), indent_of(located.near.start)),
                Some(_) => return (vec![], None),
                None if self.in_panel || self.doc != doc.id => return (vec![], None),
                None => (self.lines.end.saturating_sub(1), indent_of(self.lines.start)),
            };
            let hint = waiting_hint(story);
            let rows = if ask.is_some() { vec![] } else { vec![Row::Hint(hint, theme.warning)] };
            let spec = BlockSpec { indent, rows, prompt: prompt(if story.is_some() { "⏎ send · esc" } else { "⏎ ask · esc" }) };
            return (vec![], Some(block(line, spec, theme)));
        };
        if self.in_panel_item(id, &item) {
            return (vec![], None);
        }

        let Some(located) = self.locate_at(project, id, &item).filter(shown_here) else {
            return (vec![], None);
        };
        // An item in another file than the one asked about says which.
        let file = located.path.as_ref().map_or(String::new(), |p| format!("{} · ", project.display(p)));
        let progress = progress(story);
        let keys = |what: &str| (ask.is_none()).then(|| Row::Hint(format!("{file}{what}{progress}"), theme.muted_foreground));
        match &item {
            Item::Step(step) => {
                let Some(view) = placement::view(doc.text(), step, located.near.clone()) else {
                    let line = located.near.end.saturating_sub(1).min(doc.buffer.last_line());
                    let rows = vec![Row::Hint(format!("{file}can't place this one · ⌫ skip"), theme.danger)];
                    return (vec![], Some(block(line, BlockSpec { indent: 0, rows, prompt: prompt("⏎ send · esc") }, theme)));
                };
                let removed = view.change.removed.clone();
                let struck = (!removed.is_empty()).then(|| Decoration {
                    range: doc.buffer.lines_range(removed),
                    background: Some(theme.danger.opacity(0.14)),
                    strike: Some(theme.danger),
                    color: Some(theme.muted_foreground),
                });
                let lead = view.change.added.first().map_or(0, |l| l.len() - l.trim_start().len());
                let mut rows: Vec<Row> = view.change.added.iter().map(|l| Row::Code(l.replace('\t', "    "))).collect();
                rows.extend(keys(if step.complete { "⇥ accept · ⌫ skip · ⏎ redirect" } else { "…" }));
                (struck.into_iter().collect(), Some(block(view.anchor, BlockSpec { indent: lead, rows, prompt: prompt("⏎ send · esc") }, theme)))
            }
            Item::Note(Note { at, text, .. }) => {
                let Some(lines) = protocol::locate(doc.text(), at, located.near.clone()) else { return (vec![], None) };
                let marked = Decoration::background(doc.buffer.lines_range(lines.clone()), theme.info.opacity(0.12));
                let mut rows = vec![Row::Prose(text.clone())];
                rows.extend(keys("⇥ next · ⏎ ask"));
                (vec![marked], Some(block(lines.end - 1, BlockSpec { indent: indent_of(lines.end - 1), rows, prompt: prompt("⏎ send · esc") }, theme)))
            }
            _ => (vec![], None),
        }
    }

    /// A story asked from the tour panel shows in the panel: the output it's
    /// about, the prompt, what's happening, and answers not pinned to code.
    pub(super) fn panel(&self, ask: Option<Entity<TextField>>, theme: &Theme) -> Option<AnyElement> {
        if !self.in_panel {
            return None;
        }
        let story = self.story.as_ref();
        let current = story.and_then(|s| Some((s.current()?, s.current_item()?)));
        let (note, hint) = match &current {
            Some((id, item @ Item::Note(note))) if self.in_panel_item(*id, item) => (Some(note.text.clone()), format!("⇥ next · ⏎ ask{}", progress(story))),
            // The current item is in the code; so is a question about it.
            Some(_) => return None,
            None if story.and_then(Story::writing_about).is_some() => return None,
            None => (None, waiting_hint(story)),
        };
        let context = self.output.iter().flat_map(|o| o.lines().skip(1)).take(ghostwrite_project::tour::PAGE_ROWS).map(|l| div().whitespace_nowrap().overflow_hidden().text_color(theme.muted_foreground).child(l.to_string()));
        let row = |child: AnyElement| div().flex().items_center().gap_2().child(div().flex_none().text_xs().text_color(theme.warning).child("✦")).child(child);
        Some(
            div()
                .flex_none()
                .flex()
                .flex_col()
                .gap_1()
                .px_3()
                .py_1()
                .border_t_1()
                .border_color(theme.border)
                .font_family(theme.mono_font.clone())
                .text_size(theme.mono_size)
                .children(context)
                .children(note.map(|text| div().pl_4().text_color(theme.foreground).child(text)))
                .children(ask.is_none().then(|| row(div().text_xs().text_color(theme.muted_foreground).child(hint).into_any_element())))
                .children(ask.map(|field| {
                    let hint = if story.is_some() { "⏎ send · esc" } else { "⏎ ask · esc" };
                    row(div().flex_1().flex().items_center().child(div().flex_1().min_w_0().overflow_hidden().child(field)).child(div().flex_none().ml_3().text_xs().text_color(theme.muted_foreground).child(hint)).into_any_element())
                }))
                .into_any_element(),
        )
    }
}

/// What's happening while there's nothing to review: asking, waiting, failed.
fn waiting_hint(story: Option<&Story>) -> String {
    match story {
        None => "⏎ ask · esc".to_string(),
        Some(s) => match (&s.error, &s.looking) {
            (Some(e), _) => format!("{e} · esc"),
            (None, Some(what)) => format!("{what}…"),
            (None, None) => "…".into(),
        },
    }
}

/// `  2/5` (and `+` while more is coming).
fn progress(story: Option<&Story>) -> String {
    story.and_then(Story::progress).map_or(String::new(), |(i, n)| format!("  {}/{n}{}", i + 1, if story.is_some_and(Story::is_writing) { "+" } else { "" }))
}

/// A block under `line` that builds `spec` once it knows its sizes.
fn block(line: usize, spec: BlockSpec, theme: &Theme) -> Block {
    let theme = theme.clone();
    Block { line, build: Box::new(move |sizes| build(spec, &theme, sizes)) }
}

fn build(spec: BlockSpec, theme: &Theme, sizes: BlockSizes) -> AnyElement {
    let columns = |indent: usize| (((f32::from(sizes.width) - 48.) / f32::from(sizes.char_width.max(px(1.)))) as usize).saturating_sub(indent).max(24);
    let indent = sizes.char_width * spec.indent as f32;
    let line = |text: String, color: Hsla| div().h(sizes.line_height).flex().items_center().whitespace_nowrap().text_color(color).child(if text.is_empty() { " ".to_string() } else { text });
    let mut rows: Vec<AnyElement> = vec![];
    for row in spec.rows {
        match row {
            // Code carries its own indentation.
            Row::Code(code) => rows.extend(protocol::wrap_code(&code, columns(0)).into_iter().map(|l| line(l, theme.muted_foreground).into_any_element())),
            Row::Prose(text) => rows.extend(protocol::wrap(&text, columns(spec.indent)).into_iter().map(|l| line(l, theme.foreground).pl(indent).into_any_element())),
            Row::Hint(hint, color) => rows.push(
                div()
                    .h(sizes.line_height)
                    .pl(indent)
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().text_color(theme.warning).text_xs().child("✦"))
                    .child(div().text_xs().text_color(color).child(hint))
                    .into_any_element(),
            ),
        }
    }
    if let Some((field, hint)) = spec.prompt {
        rows.push(
            div()
                .w_full()
                .h(sizes.line_height)
                .pl(indent)
                .flex()
                .items_center()
                .gap_1()
                .child(div().flex_none().text_color(theme.warning).child("✦"))
                .child(div().flex_1().min_w_0().overflow_hidden().child(field))
                .child(div().flex_none().ml_3().text_xs().text_color(theme.muted_foreground).child(hint))
                .into_any_element(),
        );
    }
    div()
        .w(sizes.width)
        .pr(px(16.))
        .flex()
        .flex_col()
        .font_family(theme.mono_font.clone())
        .text_size(theme.mono_size)
        .line_height(sizes.line_height)
        .children(rows)
        .into_any_element()
}
