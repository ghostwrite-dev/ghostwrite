//! A window's content: the projects page, or a project's editor. Each
//! project is open in one window at most; opening it again goes to that one.

use std::path::{Path, PathBuf};

use ghostwrite_project::{root_for, store};
use gpui::{
    App, AppContext as _, Bounds, Context, Entity, PathPromptOptions, PromptLevel, Subscription, Window, WindowBounds, WindowHandle, WindowOptions, div, point,
    prelude::*, px, size,
};

use crate::actions::{OpenFolder, ShowProjects};
use crate::editor::{Editor, EditorEvent, Options};
use crate::projects_page::{ProjectsEvent, ProjectsPage};

enum Screen {
    Projects(Entity<ProjectsPage>),
    Project(Entity<Editor>),
}

pub struct Workspace {
    screen: Screen,
    _subscription: Subscription,
}

/// Open a window on `paths`' project (the projects page if there are none).
pub fn open_window(paths: Vec<PathBuf>, cx: &mut App) -> Option<WindowHandle<Workspace>> {
    // Each new window a little down and right of the last, so none hides another.
    let step = px(24. * cx.windows().len() as f32);
    let mut bounds = Bounds::centered(None, size(px(1100.), px(780.)), cx);
    bounds.origin += point(step, step);
    let options = WindowOptions { window_bounds: Some(WindowBounds::Windowed(bounds)), ..Default::default() };
    cx.open_window(options, |window, cx| {
        crate::theme::observe(window);
        let workspace = cx.new(|cx| Workspace::new(paths, window, cx));
        let closing = workspace.downgrade();
        window.on_window_should_close(cx, move |window, cx| closing.update(cx, |this, cx| this.may_close(window, cx)).unwrap_or(true));
        workspace
    })
    .ok()
}

/// Where Finder's (or the command line's) paths go: the window whose project
/// they're in, else a window showing the projects page, else a new one.
pub fn open_anywhere(paths: Vec<PathBuf>, cx: &mut App) {
    let Some(first) = paths.first() else { return };
    let windows = workspaces(cx);
    let holding = windows
        .iter()
        .filter_map(|&w| Some((w, w.read(cx).ok()?.root(cx)?)))
        .filter(|(_, root)| first.starts_with(root))
        .max_by_key(|(_, root)| root.as_os_str().len());
    let target = holding.map(|(w, _)| w).or_else(|| windows.iter().copied().find(|w| w.read(cx).is_ok_and(|ws| ws.root(cx).is_none())));
    match target {
        Some(window) => {
            window
                .update(cx, |workspace, window, cx| {
                    window.activate_window();
                    workspace.open_paths(paths, window, cx);
                })
                .ok();
        }
        None => {
            open_window(paths, cx);
        }
    }
}

fn workspaces(cx: &App) -> Vec<WindowHandle<Workspace>> {
    cx.windows().into_iter().filter_map(|w| w.downcast::<Workspace>()).collect()
}

/// The editors of every window's project.
fn editors(cx: &App) -> Vec<(WindowHandle<Workspace>, Entity<Editor>)> {
    let editor = |w: &WindowHandle<Workspace>| match &w.read(cx).ok()?.screen {
        Screen::Project(editor) => Some(editor.clone()),
        Screen::Projects(_) => None,
    };
    workspaces(cx).into_iter().filter_map(|w| Some((w, editor(&w)?))).collect()
}

/// ⌘Q and `:qa`: quit, after asking (in the first window with any) about
/// unsaved changes in every window, unless `force`d.
pub fn quit(force: bool, cx: &mut App) {
    let dirty: Vec<_> = editors(cx).into_iter().filter(|(_, e)| e.read(cx).is_dirty()).collect();
    let Some((window, _)) = dirty.first().filter(|_| !force).cloned() else { return cx.quit() };
    let files: usize = dirty.iter().map(|(_, e)| e.read(cx).dirty_count()).sum();
    window
        .update(cx, |_, window, cx| {
            window.activate_window();
            let message = format!("Quit with unsaved changes in {files} file{}?", if files == 1 { "" } else { "s" });
            let answer = window.prompt(PromptLevel::Warning, &message, None, &["Save All and Quit", "Quit Without Saving", "Cancel"], cx);
            cx.spawn(async move |_, cx| match answer.await {
                Ok(0) => cx.update(write_all_and_quit).ok(),
                Ok(1) => cx.update(|cx| cx.quit()).ok(),
                _ => None,
            })
            .detach();
        })
        .ok();
}

/// `:wqa`: write every window's files, and quit if they all were (a file
/// changed on disk isn't written over; its window says so).
pub fn write_all_and_quit(cx: &mut App) {
    let written = editors(cx).into_iter().all(|(_, editor)| {
        editor.update(cx, |e, cx| {
            let written = e.write_all();
            cx.notify();
            written
        })
    });
    if written {
        cx.quit();
    }
}

impl Workspace {
    /// The open project's folder.
    pub fn root(&self, cx: &App) -> Option<PathBuf> {
        match &self.screen {
            Screen::Project(editor) => Some(editor.read(cx).root().to_path_buf()),
            Screen::Projects(_) => None,
        }
    }

    /// Another window with `root` open.
    fn elsewhere(root: &Path, window: &Window, cx: &App) -> Option<WindowHandle<Workspace>> {
        let this = window.window_handle().window_id();
        workspaces(cx).into_iter().filter(|w| w.window_id() != this).find(|w| w.read(cx).ok().and_then(|ws| ws.root(cx)).as_deref() == Some(root))
    }

    /// The window's close button: save where things were, and ask first
    /// about unsaved changes.
    fn may_close(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Screen::Project(editor) = &self.screen else { return true };
        editor.read(cx).save_session();
        if !editor.read(cx).is_dirty() {
            return true;
        }
        self.leave_project(|_, window, _| window.remove_window(), window, cx);
        false
    }

    pub fn new(paths: Vec<PathBuf>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (screen, subscription) = Self::projects_screen(window, cx);
        let mut this = Self { screen, _subscription: subscription };
        this.open_paths(paths, window, cx);
        this
    }

    fn projects_screen(window: &mut Window, cx: &mut Context<Self>) -> (Screen, Subscription) {
        let page = cx.new(|cx| ProjectsPage::new(window, cx));
        let subscription = cx.subscribe_in(&page, window, |this, _, event, window, cx| match event {
            ProjectsEvent::Chosen(root) => this.open_project(root.clone(), vec![], window, cx),
        });
        (Screen::Projects(page), subscription)
    }

    /// Paths from the command line or Finder: files open in the current
    /// project, or in the project they belong to when none is open.
    pub fn open_paths(&mut self, paths: Vec<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(first) = paths.first() else { return };
        let files: Vec<PathBuf> = paths.iter().filter(|p| !p.is_dir()).cloned().collect();
        match &self.screen {
            Screen::Project(editor) => editor.update(cx, |editor, cx| editor.open_paths(files, cx)),
            Screen::Projects(_) => {
                let root = root_for(first);
                self.open_project(root, files, window, cx);
            }
        }
    }

    fn open_project(&mut self, root: PathBuf, files: Vec<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        // Open in another window already: go there.
        if let Some(other) = Self::elsewhere(&root, window, cx) {
            cx.defer(move |cx| {
                other
                    .update(cx, |workspace, window, cx| {
                        window.activate_window();
                        workspace.open_paths(files, window, cx);
                    })
                    .ok();
            });
            return;
        }
        store::touch_project(&root);
        // Bring back the files open last time, then any asked for now.
        let session = store::load_session(&root);
        let editor = cx.new(|cx| {
            let mut editor = Editor::new(root.clone(), Options::default(), window, cx);
            if let Some(session) = &session {
                editor.restore(session, cx);
            }
            editor.open_paths(files, cx);
            editor
        });
        let subscription = cx.subscribe_in(&editor, window, |this, _, event, window, cx| match event {
            EditorEvent::OpenProject(None) => this.show_projects(&ShowProjects, window, cx),
            EditorEvent::OpenProject(Some(root)) => {
                let root = root.clone();
                this.leave_project(move |this, window, cx| this.open_project(root, vec![], window, cx), window, cx);
            }
            EditorEvent::CloseWindow { force } => {
                if let Screen::Project(editor) = &this.screen {
                    editor.read(cx).save_session();
                }
                if *force || this.may_close(window, cx) {
                    window.remove_window();
                }
            }
        });
        self.screen = Screen::Project(editor);
        self._subscription = subscription;
        cx.notify();
    }

    /// Leave the current project, asking first if it has unsaved changes.
    fn leave_project(&mut self, then: impl FnOnce(&mut Self, &mut Window, &mut Context<Self>) + 'static, window: &mut Window, cx: &mut Context<Self>) {
        let Screen::Project(editor) = &self.screen else { return then(self, window, cx) };
        editor.read(cx).save_session();
        if !editor.read(cx).is_dirty() {
            return then(self, window, cx);
        }
        let answer = window.prompt(
            PromptLevel::Warning,
            "Leave this project without saving?",
            Some("Unsaved changes in open tabs will be lost."),
            &["Leave", "Cancel"],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            if answer.await == Ok(0) {
                this.update_in(cx, |this, window, cx| then(this, window, cx)).ok();
            }
        })
        .detach();
    }

    fn show_projects(&mut self, _: &ShowProjects, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.screen, Screen::Projects(_)) {
            return;
        }
        self.leave_project(
            |this, window, cx| {
                let (screen, subscription) = Self::projects_screen(window, cx);
                this.screen = screen;
                this._subscription = subscription;
                cx.notify();
            },
            window,
            cx,
        );
    }

    fn open_folder(&mut self, _: &OpenFolder, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions { files: false, directories: true, multiple: false, prompt: Some("Open Project".into()) });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(mut paths))) = paths.await else { return };
            let Some(root) = paths.pop() else { return };
            this.update_in(cx, |this, window, cx| this.leave_project(move |this, window, cx| this.open_project(root, vec![], window, cx), window, cx)).ok();
        })
        .detach();
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Zooming sizes everything that's in rems.
        window.set_rem_size(crate::theme::theme(cx).rem_size());
        let content = match &self.screen {
            Screen::Projects(page) => page.clone().into_any_element(),
            Screen::Project(editor) => editor.clone().into_any_element(),
        };
        div().key_context("Workspace").on_action(cx.listener(Self::show_projects)).on_action(cx.listener(Self::open_folder)).size_full().child(content)
    }
}
