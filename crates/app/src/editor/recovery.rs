//! Unsaved changes survive a crash: every couple of seconds what's changed
//! since is copied beside Ghostwrite's other state, and when the project is
//! next opened whatever's still there comes back, unsaved, for you to keep or
//! undo. A copy goes when its file is saved or its tab closed, and all of them
//! when you say to leave without saving.

use std::collections::HashMap;
use std::path::PathBuf;

use ghostwrite_project::store;
use gpui::{AppContext as _, Context, Task};
use serde_json::{Value, json};

use super::Editor;

const EVERY: std::time::Duration = std::time::Duration::from_secs(2);

/// The copies kept, by name, and the buffer version each is of.
#[derive(Default)]
pub(super) struct Recovery {
    kept: HashMap<String, u64>,
    _task: Option<Task<()>>,
}

fn folder(root: &std::path::Path) -> String {
    format!("{}/recovery", store::project_dir(root))
}

/// A stable name for a file's copy; one with no file goes by its tab.
fn name_of(path: Option<&PathBuf>, id: u64) -> String {
    let hash = |text: &str| text.bytes().fold(0xcbf29ce484222325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100000001b3));
    match path {
        Some(path) => format!("{:016x}.json", hash(&path.to_string_lossy())),
        None => format!("untitled-{id}.json"),
    }
}

impl Editor {
    /// Start copying: there may already be copies, from before a crash, and
    /// they stay until it's seen what became of them.
    pub(super) fn start_recovery(&mut self, cx: &mut Context<Self>) {
        let existing = store::listed(&folder(&self.project.root));
        self.recovery.kept = existing.into_iter().map(|name| (name, u64::MAX)).collect();
        self.recovery._task = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(EVERY).await;
                let Ok(copying) = this.update(cx, |this, cx| this.copy_changes(cx)) else { break };
                copying.await;
            }
        }));
    }

    /// Write the copies that are behind, and remove those no longer wanted.
    fn copy_changes(&mut self, cx: &mut Context<Self>) -> Task<()> {
        let dir = folder(&self.project.root);
        let mut wanted: HashMap<String, u64> = HashMap::new();
        let mut writes: Vec<(String, String)> = vec![];
        for tab in self.project.tabs.iter().filter(|tab| tab.doc.dirty && tab.doc.view_of.is_none()) {
            let (doc, version) = (&tab.doc, tab.doc.buffer.version());
            let name = name_of(doc.path.as_ref(), doc.id as u64);
            if self.recovery.kept.get(&name) != Some(&version) {
                let copy = json!({ "path": doc.path, "text": doc.buffer.text() });
                writes.push((format!("{dir}/{name}"), copy.to_string()));
            }
            wanted.insert(name, version);
        }
        let gone: Vec<String> = self.recovery.kept.keys().filter(|name| !wanted.contains_key(*name)).map(|name| format!("{dir}/{name}")).collect();
        self.recovery.kept = wanted;
        if writes.is_empty() && gone.is_empty() {
            return Task::ready(());
        }
        cx.background_spawn(async move {
            let done = ghostwrite_project::contained("keeping unsaved changes", || {
                for (name, text) in &writes {
                    store::write(name, text);
                }
                for name in &gone {
                    store::remove(name);
                }
            });
            if let Err(why) = done {
                eprintln!("{why}");
            }
        })
    }

    /// Bring back what a crash left: the files with unsaved changes as they were,
    /// each as a change to what's on disk.
    pub fn recover(&mut self, cx: &mut Context<Self>) {
        let dir = folder(&self.project.root);
        let mut recovered = 0;
        for name in store::listed(&dir) {
            let Some(copy) = store::read(&format!("{dir}/{name}")).and_then(|text| serde_json::from_str::<Value>(&text).ok()) else { continue };
            let Some(text) = copy["text"].as_str() else { continue };
            let path = copy["path"].as_str().map(PathBuf::from).filter(|path| path.is_file());
            let open = path.as_ref().and_then(|path| self.project.open(path));
            let ix = open.unwrap_or_else(|| self.project.add(None, String::new()));
            let Some(tab) = self.project.tabs.get_mut(ix) else { continue };
            if tab.doc.buffer.text() == text {
                continue;
            }
            let selection = tab.view.selection;
            tab.view.selection = tab.doc.buffer.replace_all(text, selection);
            tab.sync();
            recovered += 1;
        }
        if recovered > 0 {
            self.project.say(format!("brought back unsaved changes in {recovered} file{} after a crash; u undoes them", if recovered == 1 { "" } else { "s" }));
            self.tab_changed(cx);
        }
    }

    /// You said to leave without saving: no copies are left to bring back.
    pub fn forget_unsaved(&mut self) {
        let dir = folder(&self.project.root);
        for name in store::listed(&dir) {
            store::remove(&format!("{dir}/{name}"));
        }
        self.recovery.kept.clear();
        self.recovery._task = None;
    }
}
