//! The start page: recent projects, most recently opened first. The only
//! screen that isn't the modal editor: `j`/`k` move, Enter opens, `o` opens
//! a folder, and the mouse works as you'd expect.

use std::path::{Path, PathBuf};

use ghostwrite_project::store;
use gpui::{Context, EventEmitter, FocusHandle, Focusable, FontWeight, KeyDownEvent, MouseButton, Window, div, prelude::*};

use crate::actions::OpenFolder;
use crate::theme::{pt, theme};

/// Rows Ctrl+D / Ctrl+U move, like a half page in the editor.
const HALF_PAGE: usize = 5;

pub enum ProjectsEvent {
    Chosen(PathBuf),
}

pub struct ProjectsPage {
    projects: Vec<PathBuf>,
    selected: usize,
    focus: FocusHandle,
}

impl EventEmitter<ProjectsEvent> for ProjectsPage {}

impl Focusable for ProjectsPage {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus.clone()
    }
}

impl ProjectsPage {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus);
        Self { projects: store::recent_projects(), selected: 0, focus }
    }

    fn choose(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(project) = self.projects.get(ix).cloned() else { return };
        if project.is_dir() {
            cx.emit(ProjectsEvent::Chosen(project));
        } else {
            self.forget(&project, cx);
        }
    }

    fn forget(&mut self, project: &Path, cx: &mut Context<Self>) {
        store::forget_project(project);
        crate::actions::set_menus(cx);
        self.projects = store::recent_projects();
        self.selected = self.selected.min(self.projects.len().saturating_sub(1));
        cx.notify();
    }

    fn key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let last = self.projects.len().saturating_sub(1);
        if event.keystroke.modifiers.control {
            match event.keystroke.key.as_str() {
                "d" => self.selected = (self.selected + HALF_PAGE).min(last),
                "u" => self.selected = self.selected.saturating_sub(HALF_PAGE),
                _ => return,
            }
            cx.stop_propagation();
            cx.notify();
            return;
        }
        match event.keystroke.key.as_str() {
            "j" | "down" => self.selected = (self.selected + 1).min(self.projects.len().saturating_sub(1)),
            "k" | "up" => self.selected = self.selected.saturating_sub(1),
            "enter" => self.choose(self.selected, cx),
            "o" => window.dispatch_action(Box::new(OpenFolder), cx),
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }
}

impl Render for ProjectsPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        window.set_window_title("Ghostwrite");
        let theme = theme(cx).clone();
        let home = std::env::home_dir().unwrap_or_default();
        let rows: Vec<_> = self
            .projects
            .iter()
            .enumerate()
            .map(|(ix, project)| {
                let name = project.file_name().map_or_else(|| project.display().to_string(), |n| n.to_string_lossy().into_owned());
                let shown = match project.strip_prefix(&home) {
                    Ok(rest) => format!("~/{}", rest.display()),
                    Err(_) => project.display().to_string(),
                };
                let forget = project.clone();
                div()
                    .id(("project", ix))
                    .flex()
                    .items_center()
                    .justify_between()
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .cursor_pointer()
                    .when(ix == self.selected, |d| d.bg(theme.muted))
                    .hover(|s| s.bg(theme.muted))
                    .when(!project.is_dir(), |d| d.opacity(0.5))
                    .on_click(cx.listener(move |this, _, _, cx| this.choose(ix, cx)))
                    .child(div().flex().flex_col().child(div().child(name)).child(div().text_xs().text_color(theme.muted_foreground).child(shown)))
                    .child(
                        div()
                            .id(("forget", ix))
                            .px_2()
                            .rounded_sm()
                            .text_color(theme.muted_foreground)
                            .hover(|s| s.text_color(theme.foreground))
                            .child("×")
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.forget(&forget, cx);
                            })),
                    )
            })
            .collect();
        div()
            .track_focus(&self.focus)
            .key_context("Projects")
            .on_key_down(cx.listener(Self::key_down))
            .size_full()
            .flex()
            .justify_center()
            .bg(theme.background)
            .text_color(theme.foreground)
            .font_family(theme.ui_font.clone())
            .child(
                div()
                    .w(pt(520.))
                    .max_w_full()
                    .px_4()
                    .pt(pt(96.))
                    .flex()
                    .flex_col()
                    .gap_4()
                    .child(
                        div().flex().items_center().justify_between().child(div().text_2xl().font_weight(FontWeight::SEMIBOLD).child("Projects")).child(
                            div()
                                .id("open-folder")
                                .px_3()
                                .py_1()
                                .rounded_md()
                                .cursor_pointer()
                                .bg(theme.accent)
                                .text_color(theme.accent_foreground)
                                .hover(|s| s.opacity(0.85))
                                .child("Open Folder…  o")
                                .on_click(|_, window, cx| window.dispatch_action(Box::new(OpenFolder), cx)),
                        ),
                    )
                    .child(if rows.is_empty() {
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .text_color(theme.muted_foreground)
                            .child(div().text_color(theme.foreground).child("Welcome to Ghostwrite."))
                            .child("A modal editor where a model's changes come as small steps you accept, one at a time.")
                            .child("Open a folder to start. Then:")
                            .child(":tutor        learn the keys, in a few minutes")
                            .child(":api-key      give it a Claude API key, or use Claude Code if it's installed")
                            .child(":model        see which models can be asked, and choose one")
                            .child("space ?       see every key, at any time")
                            .into_any_element()
                    } else {
                        div().id("recent").flex().flex_col().gap_1().overflow_y_scroll().children(rows).into_any_element()
                    }),
            )
    }
}
