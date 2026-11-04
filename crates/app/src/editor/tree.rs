//! Tree mode: the file tree takes the keys (`space e`), and the panel that
//! draws it, with whatever Claude proposes doing to a file shown in place.

use std::path::{Path, PathBuf};

use ghostwrite_core::Mode;
use ghostwrite_core::modal::TreeCommand;
use ghostwrite_project::fs;
use ghostwrite_view::story::Waiting;
use gpui::{
    AnyElement, Context, FontWeight, MouseButton, MouseDownEvent, PromptLevel, ScrollStrategy, SharedString, UniformListScrollHandle, Window, div, prelude::*,
    uniform_list,
};

use super::{Editor, PromptKind};
use crate::theme::{pt, theme};
use crate::ui::pane::{Edge, Resizable};

/// The tree's side of the window: shown or not, how wide, and where it's scrolled.
pub(super) struct TreePane {
    pub(super) shown: bool,
    pub(super) width: Resizable,
    scroll: UniformListScrollHandle,
}

/// How wide it starts, and can be dragged.
const WIDTH: f32 = 240.;
pub(super) const WIDTHS: std::ops::RangeInclusive<f32> = 140.0..=640.0;

impl TreePane {
    pub(super) fn new() -> Self {
        Self { shown: true, width: Resizable::new("tree-width", Edge::Right, Some(WIDTH)), scroll: UniformListScrollHandle::new() }
    }

    fn width(&self) -> f32 {
        self.width.size.unwrap_or(WIDTH)
    }
}

const ROW_HEIGHT: f32 = 24.;
const INDENT: f32 = 14.;

/// A line of the tree panel.
#[derive(Clone)]
enum Line {
    Row(usize),
    /// A proposed new file or folder, where it would go.
    Ghost {
        depth: usize,
        name: String,
    },
    /// Under a proposal: what Claude said about it, the keys, or the prompt to ask.
    Note {
        depth: usize,
        text: String,
    },
    Keys {
        depth: usize,
        text: String,
    },
    Ask {
        depth: usize,
    },
}

/// What shows under a proposal in the tree (the story decides).
pub(super) struct Annex {
    pub note: Option<String>,
    pub keys: String,
    pub asking: bool,
}

impl Editor {
    pub(super) fn enter_tree(&mut self, cx: &mut Context<Self>) {
        self.leave_insert();
        // From the terminal: back to what it was over first. A review is still up
        // in the tree, and leaving the tree goes back to it.
        self.modal.leave(Mode::Terminal);
        self.stop_reading();
        self.stop_selecting();
        self.modal.to_tree();
        self.tree_pane.shown = true;
        self.project.tree.set_active(self.project.active_path());
        if let Some(path) = self.project.active_path() {
            self.project.tree.reveal(&path);
        }
        self.scroll_tree_to_cursor(ScrollStrategy::Center);
        cx.notify();
    }

    /// ⌘B, `:tree`: as ⌘J for the terminal, into the tree (showing it), or
    /// out of it, hiding it. During a review it only shows or hides.
    pub(super) fn toggle_tree(&mut self, cx: &mut Context<Self>) {
        if self.modal.mode != Mode::Tree && !self.modal.mode.reviewing() {
            return self.enter_tree(cx);
        }
        self.tree_pane.shown = !self.tree_pane.shown;
        if !self.tree_pane.shown {
            self.modal.leave_tree();
        }
        cx.notify();
    }

    pub(super) fn tree_command(&mut self, command: TreeCommand, window: &mut Window, cx: &mut Context<Self>) {
        match command {
            TreeCommand::Down(n) => self.project.tree.move_by(n as isize),
            TreeCommand::Up(n) => self.project.tree.move_by(-(n as isize)),
            TreeCommand::First => self.project.tree.move_to(0),
            TreeCommand::Last => self.project.tree.move_to(usize::MAX),
            // A file is previewed: it opens, and the keys stay in the tree.
            TreeCommand::Expand => {
                if let Some(path) = self.project.tree.expand() {
                    self.open_paths(vec![path], cx);
                }
            }
            TreeCommand::Collapse => self.project.tree.collapse(),
            TreeCommand::Open => match self.project.tree.selected().cloned() {
                Some(row) if row.is_dir => self.project.tree.toggle(self.project.tree.cursor()),
                Some(row) => {
                    self.modal.leave_tree();
                    self.open_paths(vec![row.path], cx);
                }
                None => {}
            },
            TreeCommand::Leave => {}
            TreeCommand::Add => {
                let dir = self.project.tree.target_dir();
                self.open_prompt(PromptKind::TreeAdd(dir), "", window, cx);
            }
            TreeCommand::Rename => {
                if let Some(row) = self.project.tree.selected().cloned() {
                    self.open_prompt(PromptKind::TreeRename(row.path), &row.name, window, cx);
                }
            }
            TreeCommand::Delete => self.tree_delete(window, cx),
        }
        self.scroll_tree_to_cursor(ScrollStrategy::Top);
    }

    fn scroll_tree_to_cursor(&self, strategy: ScrollStrategy) {
        let cursor = self.project.tree.cursor();
        if let Some(ix) = self.tree_lines(None).iter().position(|l| matches!(l, Line::Row(i) if *i == cursor)) {
            self.tree_pane.scroll.scroll_to_item(ix, strategy);
        }
    }

    pub(super) fn tree_add(&mut self, dir: &Path, name: &str, cx: &mut Context<Self>) {
        match fs::create(dir, name) {
            Ok(path) => {
                self.project.tree.reveal(&path);
                if path.is_file() {
                    self.modal.leave_tree();
                    self.open_paths(vec![path], cx);
                }
            }
            Err(e) => self.project.say(e),
        }
    }

    pub(super) fn tree_rename(&mut self, from: &Path, name: &str) {
        match fs::rename(from, name) {
            Ok(to) => self.project.path_moved(from, &to),
            Err(e) => self.project.say(e),
        }
    }

    /// `d`: move the row to the Trash, after asking.
    fn tree_delete(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(row) = self.project.tree.selected().cloned() else { return };
        let message = format!("Move {} to the Trash?", row.name);
        let detail = format!("The {} can be restored from the Trash.", if row.is_dir { "folder" } else { "file" });
        let answer = window.prompt(PromptLevel::Warning, &message, Some(&detail), &["Move to Trash", "Cancel"], cx);
        cx.spawn_in(window, async move |this, cx| {
            if answer.await != Ok(0) {
                return;
            }
            this.update(cx, |this, cx| {
                match fs::trash(&row.path) {
                    Ok(()) => {
                        this.project.path_deleted(&row.path);
                        this.tab_changed(cx);
                    }
                    Err(e) => this.project.say(e),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The panel's lines: the rows, with a proposal's ghost row and what goes under it.
    fn tree_lines(&self, annex: Option<&Annex>) -> Vec<Line> {
        let rows = self.project.tree.rows();
        let ghost = self.project.tree.new_entry();
        let proposed = self.project.tree.proposal().and_then(|p| p.row_path());
        let cols = |depth: usize| ((self.tree_pane.width() - (8. + INDENT * depth as f32 + 16.) - 8.) / 7.).max(20.) as usize;
        let under = |depth: usize| -> Vec<Line> {
            let Some(annex) = annex else { return vec![] };
            if annex.asking {
                return vec![Line::Ask { depth }];
            }
            let mut lines: Vec<Line> = annex
                .note
                .as_deref()
                .map(|note| ghostwrite_ai::protocol::wrap(note, cols(depth)).into_iter().map(|text| Line::Note { depth, text }).collect())
                .unwrap_or_default();
            lines.push(Line::Keys { depth, text: annex.keys.clone() });
            lines
        };
        let mut out = vec![];
        for (ix, row) in rows.iter().enumerate() {
            if let Some(g) = ghost.as_ref().filter(|g| g.before == ix) {
                out.push(Line::Ghost { depth: g.depth, name: g.name.clone() });
                out.extend(under(g.depth));
            }
            out.push(Line::Row(ix));
            if proposed == Some(row.path.as_path()) {
                out.extend(under(row.depth));
            }
        }
        if let Some(g) = ghost.filter(|g| g.before >= rows.len()) {
            out.push(Line::Ghost { depth: g.depth, name: g.name.clone() });
            out.extend(under(g.depth));
        }
        out
    }

    fn click_row(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus);
        let Some(row) = self.project.tree.rows().get(ix).cloned() else { return };
        if row.is_dir {
            self.project.tree.toggle(ix);
        } else {
            self.project.tree.move_to(ix);
            self.modal.leave_tree();
            self.stop_selecting();
            self.open_paths(vec![row.path], cx);
        }
        cx.notify();
    }

    pub(super) fn render_tree(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = theme(cx).clone();
        let focused = self.modal.mode == Mode::Tree;
        let annex = self.story.as_ref().and_then(|s| s.annex(&self.project, self.asking()));
        let lines = self.tree_lines(annex.as_ref());
        let ask_field = annex.as_ref().filter(|a| a.asking).and(self.prompt.as_ref()).map(|p| p.field.clone());
        let name: SharedString = self.project.root.file_name().map(|n| n.to_string_lossy().to_uppercase()).unwrap_or_default().into();
        // Naming a new or renamed entry: a prompt at the top.
        let naming = self.prompt.as_ref().and_then(|p| {
            let label = match &p.kind {
                PromptKind::TreeAdd(dir) => {
                    let rel = dir.strip_prefix(&self.project.root).unwrap_or(dir).display().to_string();
                    if rel.is_empty() { "New file or folder/".to_string() } else { format!("New in {rel}/") }
                }
                PromptKind::TreeRename(_) => "Rename to".to_string(),
                _ => return None,
            };
            Some(
                div()
                    .px_2()
                    .py_1()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .border_b_1()
                    .border_color(theme.border)
                    .text_sm()
                    .child(div().text_xs().text_color(theme.muted_foreground).child(label))
                    .child(div().px_1().rounded_sm().bg(theme.background).child(p.field.clone())),
            )
        });
        let rows: Vec<(PathBuf, String, usize, bool, bool)> =
            self.project.tree.rows().iter().map(|r| (r.path.clone(), r.name.clone(), r.depth, r.is_dir, r.ignored)).collect();
        let (cursor, active) = (self.project.tree.cursor(), self.project.tree.active().map(Path::to_path_buf));
        let proposal = self.project.tree.proposal().cloned();
        // Files a story is waiting in, marked on the deepest row that shows
        // them: the file, or the folded folder it's in.
        let mut waiting: std::collections::HashMap<PathBuf, Waiting> = std::collections::HashMap::new();
        let shown: std::collections::HashSet<&Path> = self.project.tree.rows().iter().map(|r| r.path.as_path()).collect();
        for (path, how) in self.story.as_ref().map(|s| s.files(&self.project)).unwrap_or_default() {
            if let Some(row) = path.ancestors().find(|a| shown.contains(a)) {
                let mark = waiting.entry(row.to_path_buf()).or_insert(how);
                *mark = (*mark).min(how);
            }
        }
        let expanded: Vec<bool> = rows.iter().map(|(p, ..)| self.project.tree.is_expanded(p)).collect();
        // Where the story's step is, for the eye to be led there.
        let (editor, story_mark) = (cx.entity(), self.step_mark());
        let marked = move |row: gpui::Stateful<gpui::Div>| match story_mark {
            Some(mark) => row.relative().child(super::attention::marker(editor.clone(), mark)),
            None => row,
        };
        let list = uniform_list(
            "tree",
            lines.len(),
            cx.processor(move |_, range: std::ops::Range<usize>, _, cx| {
                range
                    .filter_map(|i| {
                        let line = lines.get(i)?.clone();
                        let pad = |depth: usize| pt(8. + INDENT * depth as f32 + 16.);
                        let base = div().id(i).h(pt(ROW_HEIGHT)).w_full().flex().items_center().whitespace_nowrap().overflow_hidden();
                        Some(match line {
                            Line::Keys { depth, text } => marked(base.pl(pad(depth)).text_xs().text_color(theme.muted_foreground).child(text)),
                            Line::Note { depth, text } => base.pl(pad(depth)).text_sm().child(text),
                            Line::Ask { depth } => marked(base)
                                .pl(pad(depth))
                                .pr_2()
                                .gap_1()
                                .text_sm()
                                .child(div().flex_none().text_xs().text_color(theme.warning).child("✦"))
                                .children(ask_field.clone().map(|f| div().flex_1().min_w_0().overflow_hidden().child(f)))
                                .child(div().flex_none().text_xs().text_color(theme.muted_foreground).child("⏎ send · esc")),
                            Line::Ghost { depth, name } => base
                                .pl(pt(8. + INDENT * depth as f32))
                                .gap_1()
                                .text_sm()
                                .bg(theme.warning.opacity(0.12))
                                .text_color(theme.muted_foreground)
                                .child(div().w(pt(12.)).flex_none().text_color(theme.warning).child("+"))
                                .child(div().italic().child(name)),
                            Line::Row(ix) => {
                                let (path, name, depth, is_dir, ignored) = rows[ix].clone();
                                let arrow = match (is_dir, expanded[ix]) {
                                    (true, true) => "▾",
                                    (true, false) => "▸",
                                    _ => "",
                                };
                                let doomed = matches!(&proposal, Some(ghostwrite_project::tree::Proposal::Remove(p)) if *p == path);
                                let renamed_to = match &proposal {
                                    Some(ghostwrite_project::tree::Proposal::Rename(p, to)) if *p == path => Some(to.clone()),
                                    _ => None,
                                };
                                let is_active = active.as_ref() == Some(&path);
                                base.pl(pt(8. + INDENT * depth as f32))
                                    .pr_2()
                                    .gap_1()
                                    .text_sm()
                                    .cursor_pointer()
                                    .map(|d| match (focused && ix == cursor, is_active) {
                                        (true, _) => d.bg(theme.accent).text_color(theme.accent_foreground),
                                        (false, true) => d.bg(theme.muted),
                                        (false, false) => d.hover(|s| s.bg(theme.muted)).when(ignored, |d| d.text_color(theme.muted_foreground)),
                                    })
                                    .child(div().w(pt(12.)).flex_none().text_color(theme.muted_foreground).child(arrow))
                                    .map(|d| match (doomed, renamed_to) {
                                        (true, _) => d.bg(theme.danger.opacity(0.15)).child(div().line_through().text_color(theme.danger).child(name)),
                                        (false, Some(to)) => d
                                            .bg(theme.warning.opacity(0.12))
                                            .child(div().line_through().text_color(theme.muted_foreground).child(name))
                                            .child(div().text_color(theme.warning).child(format!("→ {to}"))),
                                        (false, None) => d.child(name),
                                    })
                                    .children(waiting.get(&path).map(|how| match how {
                                        Waiting::Here => div().flex_none().text_xs().text_color(theme.warning).child("✦"),
                                        Waiting::Later => div().flex_none().text_xs().text_color(theme.muted_foreground).child("·"),
                                    }))
                                    .on_click(cx.listener(move |this, _, window, cx| this.click_row(ix, window, cx)))
                            }
                        })
                    })
                    .collect()
            }),
        )
        .track_scroll(self.tree_pane.scroll.clone())
        .flex_1();
        div()
            .relative()
            .flex_none()
            .w(pt(self.tree_pane.width()))
            .h_full()
            .flex()
            .flex_col()
            .bg(theme.sidebar)
            .text_color(theme.sidebar_foreground)
            .border_r_1()
            .border_color(theme.border)
            .child(
                div()
                    .px_3()
                    .py_2()
                    .flex()
                    .justify_between()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.muted_foreground)
                    .child(name)
                    .when(focused, |d| d.child(div().text_color(theme.warning).child("TREE"))),
            )
            .children(naming)
            .child(list)
            // While it has the keys, what they do to the row.
            .when(focused, |d| {
                d.child(
                    div()
                        .px_3()
                        .py_1()
                        .border_t_1()
                        .border_color(theme.border)
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child("a add · r rename · d trash · esc back"),
                )
            })
            .child(self.tree_pane.width.handle("tree-edge").on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _, cx| {
                    let width = this.tree_pane.width();
                    this.tree_pane.width.grab(event.position, width);
                    cx.stop_propagation();
                }),
            ))
            .into_any_element()
    }
}
