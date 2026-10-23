//! The window's content: the projects page, or a project with its file tree
//! beside the tabbed editor.

use std::path::PathBuf;

use gpui_kit::component::resizable::{ResizablePanelEvent, ResizableState, h_resizable, resizable_panel};
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::editor::{EditorEvent, FocusFileTree, OpenProject, ShowProjects, TextEditor, ToggleFileTree};
use crate::projects::{self, ProjectsEvent, ProjectsPage};
use crate::tree::{FileTree, FileTreeEvent};

const TREE_WIDTH: f32 = 240.;

/// The file tree's width from last time, so a resize sticks across launches.
fn saved_tree_width() -> Pixels {
    let saved = projects::support_file("tree-width").and_then(|p| std::fs::read_to_string(p).ok());
    px(saved.and_then(|w| w.trim().parse().ok()).unwrap_or(TREE_WIDTH))
}

fn save_tree_width(width: Pixels) {
    let Some(path) = projects::support_file("tree-width") else { return };
    let saved = path.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|()| std::fs::write(&path, f32::from(width).to_string()));
    if let Err(e) = saved {
        eprintln!("couldn't save the file tree width to {}: {e}", path.display());
    }
}

pub struct Project {
    tree: Entity<FileTree>,
    editor: Entity<TextEditor>,
    show_tree: bool,
    layout: Entity<ResizableState>,
    _subscriptions: Vec<Subscription>,
}

enum Screen {
    Projects { page: Entity<ProjectsPage>, _subscription: Subscription },
    Project(Project),
}

pub struct Workspace {
    screen: Screen,
}

impl Workspace {
    pub fn new(paths: Vec<PathBuf>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let screen = Self::projects_screen(window, cx);
        let mut this = Self { screen };
        this.open_paths(paths, window, cx);
        this
    }

    fn projects_screen(window: &mut Window, cx: &mut Context<Self>) -> Screen {
        let page = cx.new(|cx| ProjectsPage::new(window, cx));
        let sub = cx.subscribe_in(&page, window, |this, _, event, window, cx| match event {
            ProjectsEvent::Chosen(root) => this.open_project(root.clone(), vec![], window, cx),
        });
        Screen::Projects { page, _subscription: sub }
    }

    /// Paths from the command line or Finder: files open in the current
    /// project, or in the project they belong to when none is open.
    pub fn open_paths(&mut self, paths: Vec<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(first) = paths.first() else { return };
        match &self.screen {
            Screen::Project(project) => {
                let files = paths.into_iter().filter(|p| !p.is_dir()).collect();
                project.editor.update(cx, |editor, cx| editor.open_paths(files, window, cx));
            }
            Screen::Projects { .. } => {
                let root = projects::root_for(first);
                let files = paths.into_iter().filter(|p| !p.is_dir()).collect();
                self.open_project(root, files, window, cx);
            }
        }
    }

    fn open_project(&mut self, root: PathBuf, files: Vec<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        projects::touch(&root);
        let tree = cx.new(|cx| FileTree::new(root.clone(), cx));
        // Bring back the files open last time, then any asked for now.
        let session = projects::load_session(&root);
        let editor = cx.new(|cx| {
            let mut editor = TextEditor::new(vec![], root.clone(), window, cx);
            if let Some(session) = &session {
                editor.restore(session, window, cx);
            }
            editor.open_paths(files, window, cx);
            editor
        });
        let layout = cx.new(|_| ResizableState::default());
        let subscriptions = vec![
            cx.subscribe(&layout, |_, layout, _: &ResizablePanelEvent, cx| {
                if let Some(&width) = layout.read(cx).sizes().first() {
                    save_tree_width(width);
                }
            }),
            cx.subscribe_in(&tree, window, |this, tree, event, window, cx| {
                let Screen::Project(project) = &this.screen else { return };
                match event {
                    FileTreeEvent::Open { path, focus } => {
                        project.editor.update(cx, |editor, cx| editor.open_paths(vec![path.clone()], window, cx));
                        if !focus {
                            tree.update(cx, |tree, cx| tree.focus(window, cx));
                        }
                    }
                    FileTreeEvent::Dismiss => project.editor.update(cx, |editor, cx| editor.focus_active(window, cx)),
                    FileTreeEvent::Review(key) => project.editor.update(cx, |editor, cx| editor.review_command(*key, window, cx)),
                    FileTreeEvent::Renamed { from, to } => {
                        project.editor.update(cx, |editor, cx| editor.path_moved(from, to, cx));
                    }
                    FileTreeEvent::Deleted(path) => {
                        project.editor.update(cx, |editor, cx| editor.path_deleted(path, window, cx));
                    }
                    FileTreeEvent::Error(message) => {
                        project.editor.update(cx, |editor, cx| editor.set_status(message.clone(), cx));
                    }
                }
            }),
            cx.subscribe_in(&editor, window, |this, _, event, window, cx| match event {
                EditorEvent::FocusTree => this.focus_tree(&FocusFileTree, window, cx),
                EditorEvent::OpenProject(None) => this.show_projects(&ShowProjects, window, cx),
                EditorEvent::FilesChanged => {
                    if let Screen::Project(project) = &this.screen {
                        project.tree.update(cx, |tree, cx| {
                            tree.rebuild();
                            cx.notify();
                        });
                    }
                }
                EditorEvent::ProposeCreate(path) => {
                    if let Screen::Project(project) = &mut this.screen {
                        project.tree.update(cx, |tree, cx| tree.propose(path.clone(), cx));
                        // Deciding on a new file happens in the tree.
                        if path.is_some() {
                            project.show_tree = true;
                            project.tree.update(cx, |tree, cx| tree.focus(window, cx));
                        }
                    }
                }
                EditorEvent::TreeNote(note) => {
                    if let Screen::Project(project) = &this.screen {
                        project.tree.update(cx, |tree, cx| tree.set_note(note.clone(), cx));
                    }
                }
                EditorEvent::AskInTree(input) => {
                    if let Screen::Project(project) = &mut this.screen {
                        project.tree.update(cx, |tree, cx| {
                            tree.set_asking(input.clone(), cx);
                            if input.is_none() {
                                tree.focus(window, cx);
                            }
                        });
                    }
                }
                EditorEvent::OpenProject(Some(root)) => {
                    let root = root.clone();
                    this.leave_project(move |this, window, cx| this.open_project(root, vec![], window, cx), window, cx);
                }
            }),
            cx.observe(&editor, |this, editor, cx| {
                if let Screen::Project(project) = &this.screen {
                    let active = editor.read(cx).active_path();
                    project.tree.update(cx, |tree, cx| tree.set_active(active, cx));
                }
            }),
            cx.observe_window_activation(window, |this, window, cx| {
                if window.is_window_active()
                    && let Screen::Project(project) = &this.screen
                {
                    project.tree.update(cx, |tree, cx| {
                        tree.rebuild();
                        cx.notify();
                    });
                }
            }),
        ];
        // With nothing to edit yet, start in the file tree.
        if editor.read(cx).active_path().is_none() {
            tree.update(cx, |tree, cx| tree.focus(window, cx));
        }
        self.screen = Screen::Project(Project { tree, editor, show_tree: true, layout, _subscriptions: subscriptions });
        cx.notify();
    }

    /// Leave the current project, asking first if it has unsaved changes.
    fn leave_project(&mut self, then: impl FnOnce(&mut Self, &mut Window, &mut Context<Self>) + 'static, window: &mut Window, cx: &mut Context<Self>) {
        if let Screen::Project(project) = &self.screen {
            let editor = project.editor.read(cx);
            crate::projects::save_session(editor.root(), &editor.session(cx));
        }
        let dirty = matches!(&self.screen, Screen::Project(p) if p.editor.read(cx).is_dirty());
        if !dirty {
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
        if matches!(self.screen, Screen::Projects { .. }) {
            return;
        }
        self.leave_project(
            |this, window, cx| {
                this.screen = Self::projects_screen(window, cx);
                cx.notify();
            },
            window,
            cx,
        );
    }

    fn open_project_dialog(&mut self, _: &OpenProject, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Open Project".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(mut paths))) = paths.await else { return };
            let Some(root) = paths.pop() else { return };
            this.update_in(cx, |this, window, cx| {
                this.leave_project(move |this, window, cx| this.open_project(root, vec![], window, cx), window, cx)
            })
            .ok();
        })
        .detach();
    }

    /// Move the keyboard into the file tree, or back to the editor.
    fn focus_tree(&mut self, _: &FocusFileTree, window: &mut Window, cx: &mut Context<Self>) {
        let Screen::Project(project) = &mut self.screen else { return };
        if project.show_tree && project.tree.read(cx).is_focused(window) {
            project.editor.update(cx, |editor, cx| editor.focus_active(window, cx));
            return;
        }
        project.show_tree = true;
        project.tree.update(cx, |tree, cx| tree.focus(window, cx));
        cx.notify();
    }

    fn toggle_tree(&mut self, _: &ToggleFileTree, _: &mut Window, cx: &mut Context<Self>) {
        if let Screen::Project(project) = &mut self.screen {
            project.show_tree = !project.show_tree;
            cx.notify();
        }
    }
}

impl Render for Workspace {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content = match &self.screen {
            Screen::Projects { page, .. } => page.clone().into_any_element(),
            Screen::Project(project) => h_resizable("workspace")
                .with_state(&project.layout)
                .child(
                    resizable_panel()
                        .size(saved_tree_width())
                        .size_range(px(140.)..px(640.))
                        .flex_none()
                        .visible(project.show_tree)
                        .child(project.tree.clone()),
                )
                .child(resizable_panel().child(project.editor.clone()))
                .into_any_element(),
        };
        div()
            .key_context("Workspace")
            .on_action(cx.listener(Self::show_projects))
            .on_action(cx.listener(Self::open_project_dialog))
            .on_action(cx.listener(Self::toggle_tree))
            .on_action(cx.listener(Self::focus_tree))
            .size_full()
            .child(content)
    }
}

#[cfg(test)]
mod tests {
    use gpui_kit::{AppContext as _, TestAppContext, WindowOptions};

    use super::{Screen, Workspace};
    
    #[gpui_kit::test]
    fn file_opens_its_project_and_tree_opens_tabs(cx: &mut TestAppContext) {
        let root = std::env::temp_dir().join(format!("ghostwrite-ws-{}", std::process::id()));
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
        std::fs::write(root.join("README.md"), "hi\n").unwrap();

        let (_, workspace) = cx.update(|cx| {
            gpui_kit::init(cx);
            crate::editor::bind_keys(cx);
            crate::tree::bind_keys(cx);
            let paths = vec![root.join("src/main.rs")];
            gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
                cx.new(|cx| Workspace::new(paths, window, cx))
            })
            .unwrap()
        });
        cx.run_until_parked();

        let (tree, editor) = workspace.read_with(cx, |ws, _| match &ws.screen {
            Screen::Project(p) => (p.tree.clone(), p.editor.clone()),
            Screen::Projects { .. } => panic!("expected the file's project to open"),
        });
        // The active file's folder is expanded: src/, main.rs, README.md.
        let readme = tree.read_with(cx, |t, _| t.rows.iter().position(|r| r.name == "README.md").unwrap());
        tree.update(cx, |t, cx| t.click(readme, cx));
        cx.run_until_parked();
        let active = editor.read_with(cx, |e, _| e.active_path());
        assert_eq!(active, Some(root.join("README.md")));
        std::fs::remove_dir_all(root).ok();
    }

    #[gpui_kit::test]
    fn keyboard_drives_tree_and_tabs(cx: &mut TestAppContext) {
        let root = std::env::temp_dir().join(format!("ghostwrite-kb-{}", std::process::id()));
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
        std::fs::write(root.join("README.md"), "hi\n").unwrap();

        let (window, workspace) = cx.update(|cx| {
            gpui_kit::init(cx);
            crate::theme::install(cx);
            crate::editor::bind_keys(cx);
            crate::tree::bind_keys(cx);
            let paths = vec![root.join("src/main.rs")];
            gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
                cx.new(|cx| Workspace::new(paths, window, cx))
            })
            .unwrap()
        });
        cx.run_until_parked();
        let name = cx.update(|cx| gpui_kit::component::Theme::global(cx).theme_name().clone());
        assert!(name.starts_with("Gruvbox"), "{name}");

        let (tree, editor) = workspace.read_with(cx, |ws, _| match &ws.screen {
            Screen::Project(p) => (p.tree.clone(), p.editor.clone()),
            Screen::Projects { .. } => panic!("expected a project"),
        });
        let tree_focused = |cx: &mut TestAppContext| {
            cx.update_window(window, |_, window, cx| tree.read(cx).is_focused(window)).unwrap()
        };
        let keys = |keys: &str, cx: &mut TestAppContext| {
            cx.simulate_keystrokes(window, keys);
            cx.run_until_parked();
        };

        // Rows: src/, main.rs (active), README.md.
        keys("space e", cx);
        assert!(tree_focused(cx));
        keys("j enter", cx);
        assert!(!tree_focused(cx));
        assert_eq!(editor.read_with(cx, |e, _| e.active_path()), Some(root.join("README.md")));

        keys("g p", cx);
        assert_eq!(editor.read_with(cx, |e, _| e.active_path()), Some(root.join("src/main.rs")));
        keys("g n", cx);
        assert_eq!(editor.read_with(cx, |e, _| e.active_path()), Some(root.join("README.md")));

        // `l` on a file previews it and keeps the keyboard in the tree; `h` climbs to the folder.
        keys("space e k l", cx);
        assert!(tree_focused(cx));
        assert_eq!(editor.read_with(cx, |e, _| e.active_path()), Some(root.join("src/main.rs")));
        keys("h h", cx);
        assert_eq!(tree.read_with(cx, |t, _| t.rows.len()), 2);
        keys("escape", cx);
        assert!(!tree_focused(cx));
        std::fs::remove_dir_all(root).ok();
    }

    #[gpui_kit::test]
    fn tree_width_is_resizable_and_remembered(cx: &mut TestAppContext) {
        let root = std::env::temp_dir().join(format!("ghostwrite-rs-{}", std::process::id()));
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::write(root.join("a.txt"), "a\n").unwrap();
        let (window, workspace) = cx.update(|cx| {
            gpui_kit::init(cx);
            crate::editor::bind_keys(cx);
            crate::tree::bind_keys(cx);
            let paths = vec![root.join("a.txt")];
            gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
                cx.new(|cx| Workspace::new(paths, window, cx))
            })
            .unwrap()
        });
        cx.run_until_parked();
        let layout = workspace.read_with(cx, |ws, _| match &ws.screen {
            Screen::Project(p) => p.layout.clone(),
            Screen::Projects { .. } => panic!("expected a project"),
        });
        cx.update_window(window, |_, window, cx| layout.update(cx, |l, cx| l.resize_panel(0, gpui_kit::px(320.), window, cx)))
            .unwrap();
        cx.run_until_parked();
        assert_eq!(layout.read_with(cx, |l, _| l.sizes()[0]), gpui_kit::px(320.));
        assert_eq!(super::saved_tree_width(), gpui_kit::px(320.));
        std::fs::remove_dir_all(root).ok();
    }

    #[gpui_kit::test]
    fn tree_adds_renames_and_tabs_follow(cx: &mut TestAppContext) {
        let root = std::env::temp_dir().join(format!("ghostwrite-ops-ws-{}", std::process::id()));
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::write(root.join("a.txt"), "a\n").unwrap();
        let (window, workspace) = cx.update(|cx| {
            gpui_kit::init(cx);
            crate::editor::bind_keys(cx);
            crate::tree::bind_keys(cx);
            let paths = vec![root.join("a.txt")];
            gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
                cx.new(|cx| Workspace::new(paths, window, cx))
            })
            .unwrap()
        });
        cx.run_until_parked();
        let editor = workspace.read_with(cx, |ws, _| match &ws.screen {
            Screen::Project(p) => p.editor.clone(),
            Screen::Projects { .. } => panic!("expected a project"),
        });
        let keys = |keys: &str, cx: &mut TestAppContext| {
            cx.simulate_keystrokes(window, keys);
            cx.run_until_parked();
        };

        // `a` in the tree: a new file under a new folder opens in a tab.
        keys("space e a", cx);
        cx.simulate_input(window, "notes/todo.md");
        keys("enter", cx);
        assert!(root.join("notes/todo.md").is_file());
        assert_eq!(editor.read_with(cx, |e, _| e.active_path()), Some(root.join("notes/todo.md")));

        // `r` on the open file renames it, and its tab follows.
        keys("space e r cmd-a", cx);
        cx.simulate_input(window, "done.md");
        keys("enter", cx);
        assert!(root.join("notes/done.md").is_file() && !root.join("notes/todo.md").exists());
        assert_eq!(editor.read_with(cx, |e, _| e.active_path()), Some(root.join("notes/done.md")));
        std::fs::remove_dir_all(root).ok();
    }

    #[gpui_kit::test]
    fn project_without_files_starts_empty(cx: &mut TestAppContext) {
        let root = std::env::temp_dir().join(format!("ghostwrite-empty-{}", std::process::id()));
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::write(root.join("a.txt"), "a\n").unwrap();
        let (window, workspace) = cx.update(|cx| {
            gpui_kit::init(cx);
            crate::editor::bind_keys(cx);
            crate::tree::bind_keys(cx);
            gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
                cx.new(|cx| Workspace::new(vec![root.clone()], window, cx))
            })
            .unwrap()
        });
        cx.run_until_parked();
        let (tree, editor) = workspace.read_with(cx, |ws, _| match &ws.screen {
            Screen::Project(p) => (p.tree.clone(), p.editor.clone()),
            Screen::Projects { .. } => panic!("expected a project"),
        });
        assert_eq!(editor.read_with(cx, |e, _| e.tab_count()), 0);
        assert!(cx.update_window(window, |_, window, cx| tree.read(cx).is_focused(window)).unwrap());

        // Escape leaves the tree for the empty editor, where `:` still works.
        cx.simulate_keystrokes(window, "escape :");
        cx.run_until_parked();
        cx.simulate_input(window, "e a.txt");
        cx.simulate_keystrokes(window, "enter");
        cx.run_until_parked();
        assert_eq!(editor.read_with(cx, |e, _| e.active_path()), Some(root.join("a.txt")));

        // Closing the last tab goes back to the empty state, not an untitled buffer.
        cx.simulate_keystrokes(window, ":");
        cx.simulate_input(window, "bc");
        cx.simulate_keystrokes(window, "enter");
        cx.run_until_parked();
        assert_eq!(editor.read_with(cx, |e, _| e.tab_count()), 0);
        std::fs::remove_dir_all(root).ok();
    }

    #[gpui_kit::test]
    fn a_proposed_file_shows_in_the_tree(cx: &mut TestAppContext) {
        let root = std::env::temp_dir().join(format!("ghostwrite-ghost-{}", std::process::id()));
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
        let (_, workspace) = cx.update(|cx| {
            gpui_kit::init(cx);
            crate::editor::bind_keys(cx);
            crate::tree::bind_keys(cx);
            gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
                cx.new(|cx| Workspace::new(vec![root.join("src/main.rs")], window, cx))
            })
            .unwrap()
        });
        cx.run_until_parked();
        let (tree, editor) = workspace.read_with(cx, |ws, _| match &ws.screen {
            Screen::Project(p) => (p.tree.clone(), p.editor.clone()),
            Screen::Projects { .. } => panic!("expected a project"),
        });
        editor.update(cx, |_, cx| cx.emit(crate::editor::EditorEvent::ProposeCreate(Some(crate::tree::Proposal::Create(root.join("src/util.rs"))))));
        cx.run_until_parked();
        let ghost = tree.read_with(cx, |t, _| t.ghost_row());
        let src = tree.read_with(cx, |t, _| t.rows.iter().position(|r| r.name == "src").unwrap());
        assert_eq!(ghost, Some((src + 1, 1, "util.rs".to_string())), "a faint row inside src/");
        editor.update(cx, |_, cx| cx.emit(crate::editor::EditorEvent::ProposeCreate(None)));
        cx.run_until_parked();
        assert_eq!(tree.read_with(cx, |t, _| t.ghost_row()), None);
        std::fs::remove_dir_all(root).ok();
    }

    #[gpui_kit::test]
    fn a_proposed_file_is_decided_in_the_tree(cx: &mut TestAppContext) {
        let root = std::env::temp_dir().join(format!("ghostwrite-create-{}", std::process::id()));
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
        let (window, workspace) = cx.update(|cx| {
            gpui_kit::init(cx);
            crate::editor::bind_keys(cx);
            crate::tree::bind_keys(cx);
            gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
                cx.new(|cx| Workspace::new(vec![root.join("src/main.rs")], window, cx))
            })
            .unwrap()
        });
        cx.run_until_parked();
        let (tree, editor) = workspace.read_with(cx, |ws, _| match &ws.screen {
            Screen::Project(p) => (p.tree.clone(), p.editor.clone()),
            Screen::Projects { .. } => panic!("expected a project"),
        });
        let story = "<create>\nsrc/util.rs\n</create>\n<step>\n<file>\nsrc/util.rs\n</file>\n<old>\n</old>\n<new>\npub fn one() -> u32 { 1 }\n</new>\n</step>\n<step>\n<file>\nsrc/main.rs\n</file>\n<old>\nfn main() {}\n</old>\n<new>\nfn main() { util::one(); }\n</new>\n</step>\n";
        editor.update(cx, |e, _| e.set_reply_for_test(story));
        let keys = |keys: &str, cx: &mut TestAppContext| {
            cx.simulate_keystrokes(window, keys);
            cx.run_until_parked();
        };
        keys("space i", cx);
        cx.simulate_input(window, "add util");
        keys("enter", cx);
        let tree_focused = |cx: &mut TestAppContext| cx.update_window(window, |_, window, cx| tree.read(cx).is_focused(window)).unwrap();
        assert!(tree_focused(cx), "the proposal takes the keyboard to the tree");
        assert!(tree.read_with(cx, |t, _| t.ghost_row().is_some()));
        // Enter asks about it right there; the tree's own keys type into the prompt.
        keys("enter", cx);
        keys("a d", cx);
        assert_eq!(tree.read_with(cx, |t, cx| t.asking_text(cx)), Some("ad".into()));
        assert!(root.join("src").is_dir() && !root.join("src/util.rs").exists());
        // Esc goes back to deciding it.
        keys("escape", cx);
        assert_eq!(tree.read_with(cx, |t, cx| t.asking_text(cx)), None);
        assert!(tree_focused(cx));
        assert!(tree.read_with(cx, |t, _| t.ghost_row().is_some()), "still proposed");
        // Enter in the prompt sends it.
        keys("enter", cx);
        cx.simulate_input(window, "why");
        keys("enter", cx);
        assert_eq!(editor.read_with(cx, |e, _| e.followups_for_test()), ["why"]);
        assert!(tree_focused(cx), "the answer comes back to the tree");
        assert!(tree.read_with(cx, |t, _| t.ghost_row().is_some()), "proposed again");
        // Tab in the tree creates it, and the story goes on in the new file.
        keys("tab", cx);
        assert!(root.join("src/util.rs").is_file());
        assert!(!tree_focused(cx), "back in the editor");
        assert_eq!(editor.read_with(cx, |e, _| e.active_path()), Some(root.join("src/util.rs")));
        assert_eq!(tree.read_with(cx, |t, _| t.ghost_row()), None);
        // Accepting its step moves on to the next step's file, which writes it.
        keys("tab", cx);
        assert_eq!(editor.read_with(cx, |e, _| e.active_path()), Some(root.join("src/main.rs")));
        assert_eq!(std::fs::read_to_string(root.join("src/util.rs")).unwrap(), "pub fn one() -> u32 { 1 }\n");
        std::fs::remove_dir_all(root).ok();
    }

    #[gpui_kit::test]
    fn a_question_about_a_proposed_file_is_answered_in_the_tree(cx: &mut TestAppContext) {
        let root = std::env::temp_dir().join(format!("ghostwrite-ask-tree-{}", std::process::id()));
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
        let (window, workspace) = cx.update(|cx| {
            gpui_kit::init(cx);
            crate::editor::bind_keys(cx);
            crate::tree::bind_keys(cx);
            gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
                cx.new(|cx| Workspace::new(vec![root.join("src/main.rs")], window, cx))
            })
            .unwrap()
        });
        cx.run_until_parked();
        let (tree, editor) = workspace.read_with(cx, |ws, _| match &ws.screen {
            Screen::Project(p) => (p.tree.clone(), p.editor.clone()),
            Screen::Projects { .. } => panic!("expected a project"),
        });
        let story = "<create>\nsrc/util.rs\n</create>\n";
        let answer = "<answer>\n<note>\n<at>\nfn main() {}\n</at>\nutil.rs holds the helpers main uses.\n</note>\n</answer>\n";
        editor.update(cx, |e, _| e.set_rounds_for_test(&[story, answer]));
        let keys = |keys: &str, cx: &mut TestAppContext| {
            cx.simulate_keystrokes(window, keys);
            cx.run_until_parked();
        };
        let tree_focused = |cx: &mut TestAppContext| cx.update_window(window, |_, window, cx| tree.read(cx).is_focused(window)).unwrap();
        keys("space i", cx);
        cx.simulate_input(window, "add util");
        keys("enter", cx);
        keys("enter", cx);
        cx.simulate_input(window, "why this name");
        keys("enter", cx);
        assert_eq!(tree.read_with(cx, |t, _| t.note_for_test().map(str::to_string)), Some("util.rs holds the helpers main uses.".into()));
        assert!(tree.read_with(cx, |t, _| t.ghost_row().is_some()), "shown under the proposal");
        assert!(tree_focused(cx), "and read there");
        // Tab reads on: back to deciding the file.
        keys("tab", cx);
        assert_eq!(tree.read_with(cx, |t, _| t.note_for_test().map(str::to_string)), None);
        assert!(tree.read_with(cx, |t, _| t.ghost_row().is_some()));
        keys("tab", cx);
        assert!(root.join("src/util.rs").is_file());
        std::fs::remove_dir_all(root).ok();
    }

    #[gpui_kit::test]
    fn reopening_a_project_brings_back_its_files_and_cursors(cx: &mut TestAppContext) {
        let root = std::env::temp_dir().join(format!("ghostwrite-session-{}", std::process::id()));
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::write(root.join("a.txt"), "a\n").unwrap();
        std::fs::write(root.join("b.txt"), "one\ntwo\nthree\n").unwrap();
        let (window, workspace) = cx.update(|cx| {
            gpui_kit::init(cx);
            crate::editor::bind_keys(cx);
            crate::tree::bind_keys(cx);
            gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
                cx.new(|cx| Workspace::new(vec![root.join("a.txt"), root.join("b.txt")], window, cx))
            })
            .unwrap()
        });
        cx.run_until_parked();
        cx.simulate_keystrokes(window, "j j l");
        cx.run_until_parked();
        // Leave for the projects page, then come back.
        cx.update_window(window, |_, window, cx| {
            workspace.update(cx, |ws, cx| ws.show_projects(&crate::editor::ShowProjects, window, cx));
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(window, |_, window, cx| workspace.update(cx, |ws, cx| ws.open_project(root.clone(), vec![], window, cx))).unwrap();
        cx.run_until_parked();
        let editor = workspace.read_with(cx, |ws, _| match &ws.screen {
            Screen::Project(p) => p.editor.clone(),
            Screen::Projects { .. } => panic!("expected a project"),
        });
        assert_eq!(editor.read_with(cx, |e, cx| e.session(cx).files.len()), 2, "both tabs are back");
        assert_eq!(editor.read_with(cx, |e, _| e.active_path()), Some(root.join("b.txt")), "with b active");
        let pos = editor.read_with(cx, |e, cx| e.session(cx).files[1].clone());
        assert_eq!((pos.1, pos.2), (2, 1), "and its cursor where it was");
        std::fs::remove_dir_all(root).ok();
    }
}
