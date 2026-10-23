//! The start page: recent projects, most recently opened first.

use std::path::{Path, PathBuf};

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::editor::OpenProject;

const MAX_RECENT: usize = 20;

/// A file in Ghostwrite's Application Support folder; tests get a scratch one.
pub fn support_file(name: &str) -> Option<PathBuf> {
    if cfg!(test) {
        return Some(std::env::temp_dir().join(format!("ghostwrite-{name}-{}", std::process::id())));
    }
    Some(std::env::home_dir()?.join("Library/Application Support/Ghostwrite").join(name))
}

/// A project's open files, in tab order, with their cursors (line, column),
/// and which tab was active.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Session {
    pub files: Vec<(PathBuf, u32, u32)>,
    pub active: usize,
}

pub fn load_session(root: &Path) -> Option<Session> {
    let text = std::fs::read_to_string(support_file("sessions.json")?).ok()?;
    let all: serde_json::Value = serde_json::from_str(&text).ok()?;
    let entry = all.get(root.to_string_lossy().as_ref())?;
    let files = entry["files"]
        .as_array()?
        .iter()
        .filter_map(|f| Some((PathBuf::from(f["path"].as_str()?), f["line"].as_u64()? as u32, f["column"].as_u64()? as u32)))
        .collect();
    Some(Session { files, active: entry["active"].as_u64().unwrap_or(0) as usize })
}

pub fn save_session(root: &Path, session: &Session) {
    let Some(path) = support_file("sessions.json") else { return };
    let mut all: serde_json::Value =
        std::fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_else(|| serde_json::json!({}));
    let files: Vec<serde_json::Value> = session
        .files
        .iter()
        .map(|(p, line, column)| serde_json::json!({ "path": p, "line": line, "column": column }))
        .collect();
    all[root.to_string_lossy().as_ref()] = serde_json::json!({ "files": files, "active": session.active });
    let saved = path.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|()| std::fs::write(&path, all.to_string()));
    if let Err(e) = saved {
        eprintln!("couldn't save the session to {}: {e}", path.display());
    }
}

fn store_path() -> Option<PathBuf> {
    support_file("projects")
}

pub fn recent() -> Vec<PathBuf> {
    let Some(text) = store_path().and_then(|p| std::fs::read_to_string(p).ok()) else { return vec![] };
    text.lines().filter(|l| !l.is_empty()).map(PathBuf::from).collect()
}

fn save(projects: &[PathBuf]) {
    let Some(path) = store_path() else { return };
    let text: String = projects.iter().map(|p| format!("{}\n", p.display())).collect();
    let saved = path.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|()| std::fs::write(&path, text));
    if let Err(e) = saved {
        eprintln!("couldn't save recent projects to {}: {e}", path.display());
    }
}

/// Move `project` to the top of the recent list.
pub fn touch(project: &Path) {
    let mut projects = recent();
    projects.retain(|p| p != project);
    projects.insert(0, project.to_path_buf());
    projects.truncate(MAX_RECENT);
    save(&projects);
}

fn forget(project: &Path) {
    let mut projects = recent();
    projects.retain(|p| p != project);
    save(&projects);
}

/// The project a path belongs to: the enclosing git repository, else the
/// folder itself, else the file's folder.
pub fn root_for(path: &Path) -> PathBuf {
    let path = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let dir = if path.is_dir() { path.clone() } else { path.parent().map_or(path.clone(), Path::to_path_buf) };
    dir.ancestors().find(|d| d.join(".git").exists()).map_or(dir.clone(), Path::to_path_buf)
}

pub enum ProjectsEvent {
    Chosen(PathBuf),
}

pub struct ProjectsPage {
    projects: Vec<PathBuf>,
    focus: FocusHandle,
}

impl EventEmitter<ProjectsEvent> for ProjectsPage {}

impl ProjectsPage {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        Self { projects: recent(), focus }
    }

    fn choose(&mut self, project: PathBuf, cx: &mut Context<Self>) {
        if project.is_dir() {
            cx.emit(ProjectsEvent::Chosen(project));
        } else {
            forget(&project);
            self.projects = recent();
            cx.notify();
        }
    }

    fn render_project(&self, ix: usize, project: &Path, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let name = project.file_name().map_or_else(|| project.display().to_string(), |n| n.to_string_lossy().into_owned());
        let home = std::env::home_dir().unwrap_or_default();
        let shown = match project.strip_prefix(&home) {
            Ok(rest) => format!("~/{}", rest.display()),
            Err(_) => project.display().to_string(),
        };
        let missing = !project.is_dir();
        let (open, remove) = (project.to_path_buf(), project.to_path_buf());
        div()
            .id(("project", ix))
            .flex()
            .items_center()
            .justify_between()
            .px_3()
            .py_2()
            .rounded_md()
            .cursor_pointer()
            .hover(|s| s.bg(theme.muted))
            .when(missing, |d| d.opacity(0.5))
            .on_click(cx.listener(move |this, _, _, cx| this.choose(open.clone(), cx)))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .child(div().text_color(theme.foreground).child(name))
                    .child(div().text_xs().text_color(theme.muted_foreground).child(shown)),
            )
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
                        forget(&remove);
                        this.projects = recent();
                        cx.notify();
                    })),
            )
    }
}

impl Render for ProjectsPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        window.set_window_title("Ghostwrite");
        let projects = self.projects.clone();
        let mut rows = Vec::with_capacity(projects.len());
        for (ix, p) in projects.iter().enumerate() {
            rows.push(self.render_project(ix, p, cx).into_any_element());
        }
        let theme = cx.theme();
        div()
            .track_focus(&self.focus)
            .key_context("Projects")
            .size_full()
            .flex()
            .justify_center()
            .bg(theme.background)
            .text_color(theme.foreground)
            .child(
                div()
                    .w(px(520.))
                    .max_w_full()
                    .px_4()
                    .pt(px(96.))
                    .flex()
                    .flex_col()
                    .gap_4()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(div().text_2xl().font_weight(FontWeight::SEMIBOLD).child("Projects"))
                            .child(
                                Button::new("open-folder")
                                    .primary()
                                    .label("Open Folder…")
                                    .on_click(|_, window, cx| window.dispatch_action(Box::new(OpenProject), cx)),
                            ),
                    )
                    .child(if rows.is_empty() {
                        div()
                            .text_color(theme.muted_foreground)
                            .child("No recent projects. Open a folder to start.")
                            .into_any_element()
                    } else {
                        div().id("recent").flex().flex_col().gap_1().overflow_y_scroll().children(rows).into_any_element()
                    }),
            )
    }
}
