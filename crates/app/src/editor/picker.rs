//! Pickers, as in helix: `space f` the project's files, `space b` the open
//! ones, `space s` the file's symbols, `space .` the server's code actions.
//! Typing narrows them down, fuzzily (`edmod` finds `editor/mod.rs`);
//! ↑ ↓ choose, Enter takes it, Esc closes.

use std::path::PathBuf;

use gpui::{AnyElement, AppContext as _, Context, Window, div, prelude::*};
use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher};

use super::prompt::PromptKind;
use super::{DocId, Editor};
use crate::theme::{pt, theme};

/// Rows shown at once.
const ROWS: usize = 14;
/// Files listed at most (a huge folder still opens).
const MAX_FILES: usize = 200_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PickerKind {
    Files,
    Buffers,
    Symbols,
    Actions,
}

/// What picking a row does.
#[derive(Clone)]
pub(super) enum Pick {
    /// Open a file, by its path in the project.
    File(String),
    /// Go to an open file.
    Tab(DocId),
    /// Go to a place in an open file.
    At(DocId, usize),
    /// Carry out a code action.
    Action(Box<ghostwrite_lsp::CodeAction>),
}

pub(super) struct Picker {
    kind: PickerKind,
    /// What can be picked: how it reads, and what picking it does.
    items: Vec<(String, Pick)>,
    /// The items matching what's typed, best first.
    shown: Vec<usize>,
    selected: usize,
    typed: String,
    /// The project's files are still being listed.
    listing: bool,
}

impl Picker {
    /// Whether the project's files are still being listed.
    pub(super) fn listing(&self) -> bool {
        self.listing
    }

    fn filter(&mut self) {
        let pattern = Pattern::parse(&self.typed, CaseMatching::Smart, Normalization::Smart);
        let mut matcher = Matcher::new(Config::DEFAULT.match_paths());
        let found = pattern.match_list(self.items.iter().enumerate().map(|(i, (label, _))| Labeled(i, label)), &mut matcher);
        self.shown = found.into_iter().map(|(Labeled(i, _), _)| i).collect();
        self.selected = 0;
    }
}

/// An item's label, and where it is in the list.
struct Labeled<'a>(usize, &'a str);

impl AsRef<str> for Labeled<'_> {
    fn as_ref(&self) -> &str {
        self.1
    }
}

impl Editor {
    /// `space f`, `space b`.
    pub(super) fn open_picker(&mut self, kind: PickerKind, window: &mut Window, cx: &mut Context<Self>) {
        let items = match kind {
            PickerKind::Buffers => self
                .project
                .tabs
                .iter()
                .map(|t| (t.doc.path.as_deref().map_or_else(|| t.doc.name().to_string(), |p| self.project.display(p)), Pick::Tab(t.doc.id)))
                .collect(),
            _ => vec![],
        };
        self.show_picker(kind, items, window, cx);
    }

    /// A picker of `items`, of `kind` (the project's files are listed meanwhile).
    pub(super) fn show_picker(&mut self, kind: PickerKind, items: Vec<(String, Pick)>, window: &mut Window, cx: &mut Context<Self>) {
        let root = self.project.root.clone();
        let mut picker = Picker { kind, items, shown: vec![], selected: 0, typed: String::new(), listing: kind == PickerKind::Files };
        picker.filter();
        self.picker = Some(picker);
        self.open_prompt(PromptKind::Pick, "", window, cx);
        if kind == PickerKind::Files {
            let listing = cx.background_spawn(async move { ghostwrite_project::files::files(&root, &root).into_iter().take(MAX_FILES).collect::<Vec<_>>() });
            cx.spawn(async move |this, cx| {
                let files = listing.await;
                this.update(cx, |this, cx| {
                    let Some(picker) = this.picker.as_mut().filter(|p| p.kind == PickerKind::Files) else { return };
                    picker.items = files.into_iter().map(|f| (f.clone(), Pick::File(f))).collect();
                    picker.listing = false;
                    picker.filter();
                    cx.notify();
                })
                .ok();
            })
            .detach();
        }
    }

    pub(super) fn picker_typed(&mut self, typed: &str, cx: &mut Context<Self>) {
        if let Some(picker) = self.picker.as_mut() {
            picker.typed = typed.to_string();
            picker.filter();
            cx.notify();
        }
    }

    pub(super) fn picker_move(&mut self, down: bool, cx: &mut Context<Self>) {
        if let Some(picker) = self.picker.as_mut().filter(|p| !p.shown.is_empty()) {
            let n = picker.shown.len();
            picker.selected = if down { (picker.selected + 1) % n } else { (picker.selected + n - 1) % n };
            cx.notify();
        }
    }

    /// Enter: open what's chosen.
    pub(super) fn picker_open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let chosen = self.picker.take().and_then(|p| p.shown.get(p.selected).map(|&i| p.items[i].clone()));
        self.close_prompt(window, cx);
        match chosen.map(|(_, pick)| pick) {
            Some(Pick::Tab(doc)) => {
                if let Some(ix) = self.project.tabs.index_of_id(doc) {
                    self.activate(ix, cx);
                }
            }
            Some(Pick::At(doc, at)) => {
                if let Some(ix) = self.project.tabs.index_of_id(doc) {
                    self.activate(ix, cx);
                }
                if let Some(tab) = self.tab_mut() {
                    let line = tab.doc.buffer.line_of(at);
                    tab.select(ghostwrite_core::Range::point(at), super::Autoscroll::Center(line..line + 1));
                }
            }
            Some(Pick::File(path)) => self.open_paths(vec![self.project.root.join(PathBuf::from(path))], cx),
            Some(Pick::Action(action)) => self.run_code_action(*action, cx),
            None => {}
        }
    }

    pub(super) fn close_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.picker = None;
        self.close_prompt(window, cx);
    }

    /// The picker, over the code: what's typed, and what matches.
    pub(super) fn render_picker(&self, cx: &Context<Self>) -> Option<AnyElement> {
        let picker = self.picker.as_ref()?;
        let field = self.prompt.as_ref().filter(|p| p.kind == PromptKind::Pick)?.field.clone();
        let theme = theme(cx);
        let first = picker.selected.saturating_sub(ROWS - 1);
        let rows = picker.shown.iter().enumerate().skip(first).take(ROWS).map(|(n, &i)| {
            let label = &picker.items[i].0;
            // A path's name first, then its folder; anything else as it reads (its detail after a tab).
            let (name, dir) = match picker.kind {
                PickerKind::Files | PickerKind::Buffers => label.rsplit_once('/').map_or((label.as_str(), ""), |(d, n)| (n, d)),
                _ => label.split_once('\t').unwrap_or((label.as_str(), "")),
            };
            div()
                .px_3()
                .flex()
                .gap_2()
                .when(n == picker.selected, |d| d.bg(theme.accent).text_color(theme.accent_foreground))
                .child(div().whitespace_nowrap().child(name.to_string()))
                .child(
                    div()
                        .whitespace_nowrap()
                        .overflow_hidden()
                        .text_color(if n == picker.selected { theme.accent_foreground } else { theme.muted_foreground })
                        .child(dir.to_string()),
                )
        });
        let what = match picker.kind {
            PickerKind::Files => "files",
            PickerKind::Buffers => "open files",
            PickerKind::Symbols => "symbols",
            PickerKind::Actions => "actions",
        };
        let count = if picker.listing() { format!("listing {what}…") } else { format!("{} of {} {what}", picker.shown.len(), picker.items.len()) };
        Some(
            div()
                .absolute()
                .inset_0()
                .occlude()
                .flex()
                .justify_center()
                .pt(pt(60.))
                .child(
                    div()
                        .w(pt(560.))
                        .max_w_full()
                        .h_auto()
                        .flex()
                        .flex_col()
                        .rounded_lg()
                        .border_1()
                        .border_color(theme.border)
                        .bg(theme.popover)
                        .shadow_lg()
                        .font_family(theme.mono_font.clone())
                        .text_size(theme.mono_size)
                        .child(
                            div()
                                .px_3()
                                .py_2()
                                .border_b_1()
                                .border_color(theme.border)
                                .flex()
                                .gap_2()
                                .child(div().flex_1().child(field))
                                .child(div().text_color(theme.muted_foreground).child(count)),
                        )
                        .child(div().py_1().children(rows)),
                )
                .into_any_element(),
        )
    }
}
