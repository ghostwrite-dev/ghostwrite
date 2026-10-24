//! The window's content: the projects page, or a project's editor.

use std::path::PathBuf;

use ghostwrite_project::{root_for, store};
use gpui::{AppContext as _, Context, Entity, PathPromptOptions, PromptLevel, Subscription, Window, div, prelude::*};

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

impl Workspace {
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
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content = match &self.screen {
            Screen::Projects(page) => page.clone().into_any_element(),
            Screen::Project(editor) => editor.clone().into_any_element(),
        };
        div().key_context("Workspace").on_action(cx.listener(Self::show_projects)).on_action(cx.listener(Self::open_folder)).size_full().child(content)
    }
}
