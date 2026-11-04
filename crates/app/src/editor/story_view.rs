//! Drawing a story: the current item folded out under its lines (a step's
//! removed lines struck through and its new lines as ghost text, or a note
//! under the lines it's about), its keys on a faint line below, and the
//! prompt when it's open. What's decided in the file tree shows there.
//!
//! The block is described as rows of text; the code view builds it with the
//! sizes it measures as it lays out, so wrapping always fits this frame.

use ghostwrite_ai::placement;
use ghostwrite_ai::protocol::{self, FileOp, Note};
use ghostwrite_ai::{Item, Story, lines};
use gpui::{AnyElement, Entity, Hsla, div, prelude::*, px};

use super::element::{Block, BlockSizes, Decoration};
use super::story::{Located, StoryState};
use super::tree::Annex;
use super::{Project, Tab};
use crate::theme::{Theme, pt};
use crate::ui::text_field::TextField;

/// A command the story proposes, as the terminal shows it: its note goes above
/// the prompt, the command is grey at the cursor, and the keys go under it.
pub(super) struct ProposedCommand {
    pub note: Option<Block>,
    pub command: String,
    pub keys: Block,
}

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
        let (note, keys) = match &item {
            Item::Step(step) if self.proposal(project, id, &item).is_some() => match step.op {
                Some(FileOp::Remove) => (Some(step.say.clone()), "⇥ move it to the Trash · ⏎ redirect".to_string()),
                Some(FileOp::Rename(_)) => (Some(step.say.clone()), "⇥ rename it · ⏎ redirect".to_string()),
                _ => {
                    let first = step.new.first().map_or(String::new(), |l| format!(" It starts with: {}", l.trim()));
                    (Some(format!("{}{first}", step.say)), "⇥ create it and write · ⏎ redirect".to_string())
                }
            },
            _ => return None,
        };
        Some(Annex { note, keys, asking })
    }

    /// Whether Claude is working and nothing is shown yet, so the dots animate.
    pub(super) fn thinking(&self) -> bool {
        self.story.as_ref().is_some_and(|s| s.error.is_none() && s.current().is_none())
    }

    /// The command the story is at, not yet run, for the terminal to show.
    pub(super) fn proposed_command(&self, ask: Option<Entity<TextField>>, theme: &Theme) -> Option<ProposedCommand> {
        let story = self.story.as_ref()?;
        let Item::Run(run) = story.current_item()? else { return None };
        let spec = |rows, prompt| BlockSpec { indent: 0, rows, prompt };
        let note = (!run.say.is_empty()).then(|| block(0, spec(vec![Row::Prose(run.say.clone())], None), theme));
        let rows = if ask.is_some() {
            vec![]
        } else {
            vec![Row::Hint(format!("{} · ⏎ redirect{}", run.tab_does(), progress(Some(story))), theme.muted_foreground)]
        };
        let keys = block(0, spec(rows, ask.map(|field| (field, "⏎ send · esc"))), theme);
        Some(ProposedCommand { note, command: run.typed().to_string(), keys })
    }

    /// What the story draws in `tab`: marks on its lines, and the block that
    /// folds out under them. `ask` is the prompt's field while it's open. A
    /// command shows only in the terminal, where it runs.
    pub(super) fn overlay(&self, project: &Project, tab: &Tab, ask: Option<Entity<TextField>>, theme: &Theme) -> (Vec<Decoration>, Option<Block>) {
        if self.annex(project, ask.is_some()).is_some() {
            return (vec![], None);
        }
        let doc = &tab.doc;
        let story = self.story.as_ref();
        let prompt = |hint| ask.clone().map(|field| (field, hint));
        let indent_of = |line: usize| {
            let text = doc.buffer.line(line.min(doc.buffer.last_line()));
            let lead = &text[..text.len() - text.trim_start_matches([' ', '\t']).len()];
            lead.len() + lead.matches('\t').count() * 3
        };
        let here = |located: &Located| located.tab.and_then(|ix| project.tabs.get(ix)).is_some_and(|t| t.doc.id == doc.id);

        // Asking: the prompt, under what it's about. Waiting for the reply isn't about
        // any line yet: the model thinks in the corner (`Editor::render_thinking`).
        let current = story.and_then(|s| Some((s.current()?, s.current_item()?)));
        let Some((id, item)) = current else {
            if ask.is_none() {
                return (vec![], None);
            }
            let Some(place) = self.waiting_spot(project) else { return (vec![], None) };
            let located = self.in_tab(project, place);
            if !here(&located) {
                return (vec![], None);
            }
            let rows = vec![];
            let hint = if story.is_some() { "⏎ send · esc" } else { "⏎ ask · esc" };
            let spec = BlockSpec { indent: indent_of(located.near.start), rows, prompt: prompt(hint) };
            return (vec![], Some(block(located.near.end.saturating_sub(1), spec, theme)));
        };

        // In this tab.
        let Some(located) = self.locate(project, id).filter(here) else { return (vec![], None) };
        // An item in another file than the one asked about says which.
        let file = located.path.as_ref().map_or(String::new(), |p| format!("{} · ", project.display(p)));
        let progress = progress(story);
        let keys = |what: &str| (ask.is_none()).then(|| Row::Hint(format!("{file}{what}{progress}"), theme.muted_foreground));
        match &item {
            Item::Step(step) => {
                let Some(view) = placement::view(doc.text(), step, located.near.clone()) else {
                    let line = located.near.end.saturating_sub(1).min(doc.buffer.last_line());
                    let rows = vec![Row::Hint(format!("{file}can't place this one · ⏎ redirect"), theme.danger)];
                    return (vec![], Some(block(line, BlockSpec { indent: 0, rows, prompt: prompt("⏎ send · esc") }, theme)));
                };
                let removed = view.change.removed.clone();
                let struck = (!removed.is_empty()).then(|| Decoration {
                    range: doc.buffer.lines_range(removed),
                    background: Some(theme.danger.opacity(0.14)),
                    strike: Some(theme.danger),
                    color: Some(theme.muted_foreground),
                    underline: None,
                });
                let lead = view.change.added.first().map_or(0, |l| l.len() - l.trim_start().len());
                // Why, then the change.
                let mut rows: Vec<Row> = (!step.say.is_empty()).then(|| Row::Prose(step.say.clone())).into_iter().collect();
                rows.extend(view.change.added.iter().map(|l| Row::Code(l.replace('\t', "    "))));
                rows.extend(keys(if step.complete { "⇥ accept · ⏎ redirect" } else { "…" }));
                (struck.into_iter().collect(), Some(block(view.anchor, BlockSpec { indent: lead, rows, prompt: prompt("⏎ send · esc") }, theme)))
            }
            Item::Note(Note { at, text, line, .. }) => {
                let Some(lines) = lines::find(doc.text(), at, located.near.clone(), line.is_some()).map(|f| f.lines) else { return (vec![], None) };
                let marked = Decoration::background(doc.buffer.lines_range(lines.clone()), theme.info.opacity(0.12));
                let mut rows = vec![Row::Prose(text.clone())];
                rows.extend(keys("⇥ next · ⏎ ask"));
                (vec![marked], Some(block(lines.end - 1, BlockSpec { indent: indent_of(lines.end - 1), rows, prompt: prompt("⏎ send · esc") }, theme)))
            }
            Item::Run(_) => (vec![], None),
        }
    }
}

/// What's happening while there's nothing to review: asking, waiting, failed.
pub(super) fn waiting_hint(story: Option<&Story>) -> String {
    match story {
        None => "⏎ ask · esc".to_string(),
        Some(s) => match (&s.error, &s.looking) {
            (Some(e), _) => format!("{e} · esc"),
            (None, Some(what)) => format!("{what}{}", dots()),
            (None, None) => format!("thinking{}", dots()),
        },
    }
}

/// `.`, `..`, `...` in turn, so waiting shows it's alive.
fn dots() -> &'static str {
    let ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis());
    [".", "..", "..."][(ms / 400 % 3) as usize]
}

/// `  2/5` (and `+` while more is coming).
pub(super) fn progress(story: Option<&Story>) -> String {
    story.and_then(Story::progress).map_or(String::new(), |(i, n)| format!("  {}/{n}{}", i + 1, if story.is_some_and(Story::is_writing) { "+" } else { "" }))
}

/// A block under `line` that builds `spec` once it knows its sizes.
fn block(line: usize, spec: BlockSpec, theme: &Theme) -> Block {
    let theme = theme.clone();
    Block { line, build: Box::new(move |sizes, _, _| build(spec, &theme, sizes)) }
}

fn build(spec: BlockSpec, theme: &Theme, sizes: BlockSizes) -> AnyElement {
    let columns =
        |indent: usize| (((f32::from(sizes.width) - 48. * theme.zoom) / f32::from(sizes.char_width.max(px(1.)))) as usize).saturating_sub(indent).max(24);
    let indent = sizes.char_width * spec.indent as f32;
    let line = |text: String, color: Hsla| {
        div().h(sizes.line_height).flex().items_center().whitespace_nowrap().text_color(color).child(if text.is_empty() { " ".to_string() } else { text })
    };
    let mut rows: Vec<AnyElement> = vec![];
    for row in spec.rows {
        match row {
            // Code carries its own indentation.
            Row::Code(code) => rows.extend(protocol::wrap_code(&code, columns(0)).into_iter().map(|l| line(l, theme.muted_foreground).into_any_element())),
            Row::Prose(text) => {
                let note = div().ml(indent).pl_2().border_l_2().border_color(theme.info).bg(theme.info.opacity(0.10)).flex().flex_col();
                let note = note.children(protocol::wrap(&text, columns(spec.indent + 2)).into_iter().map(|l| line(l, theme.foreground)));
                rows.push(note.into_any_element())
            }
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
                .min_h(sizes.line_height)
                .pl(indent)
                .flex()
                .items_start()
                .gap_1()
                .child(div().flex_none().text_color(theme.warning).child("✦"))
                .child(div().flex_1().min_w_0().child(field))
                .child(div().flex_none().h(sizes.line_height).flex().items_center().ml_3().text_xs().text_color(theme.muted_foreground).child(hint))
                .into_any_element(),
        );
    }
    div()
        .w(sizes.width)
        .pr(pt(16.))
        .flex()
        .flex_col()
        .font_family(theme.mono_font.clone())
        .text_size(theme.mono_size)
        .line_height(sizes.line_height)
        .children(rows)
        .into_any_element()
}
