//! The editor's side of plugins: `:` commands they add, telling them what happens to
//! files, carrying out what they answer, and `:plugin`. They're asked off the
//! UI thread, so a slow one holds up nothing, and answer with what they'd like done
//! ([`Action`]): an edit of text that's changed since they were shown it isn't made.

use std::ops::Range;
use std::path::Path;

use ghostwrite_plugin::manifest::Manifest;
use ghostwrite_plugin::{Action, Context as Shown, Event, Plugin, Reply, install};
use gpui::{AppContext as _, Context};

use super::commands::ManagePlugins;
use super::project::Project;
use super::tabs::Tab;
use super::{DocId, Editor};

/// Something that happened to a file, for the plugins that want to hear of it.
pub struct Heard {
    event: Event,
    doc: DocId,
    version: u64,
    shown: Shown,
}

/// What a plugin is shown of `tab`.
fn shown(root: &Path, tab: &Tab) -> Shown {
    let selection = tab.view.selection;
    Shown {
        root: root.display().to_string(),
        path: tab.doc.path.as_ref().map(|path| path.display().to_string()),
        language: tab.doc.language.map(|language| language.name.to_string()),
        text: tab.doc.text().to_string(),
        cursor: tab.cursor(),
        selection: selection.from()..selection.to(),
    }
}

impl Project {
    /// `event` happened to tab `ix`, if any plugin wants to know.
    pub(super) fn tell_plugins(&mut self, ix: usize, event: Event) {
        if ghostwrite_plugin::hearing(event).is_empty() {
            return;
        }
        let Some(tab) = self.tabs.get(ix).filter(|tab| !tab.doc.read_only()) else { return };
        let heard = Heard { event, doc: tab.doc.id, version: tab.doc.buffer.version(), shown: shown(&self.root, tab) };
        self.heard.push(heard);
    }
}

/// `edits` done to `tab`, as one undo step, if they're all sound; they're all about
/// the text as it is, whatever order they come in.
pub(super) fn apply_edits(tab: &mut Tab, mut edits: Vec<(Range<usize>, String)>) -> Result<(), String> {
    if tab.doc.read_only() {
        return Err("this file is read-only".into());
    }
    let text = tab.doc.text();
    edits.sort_by_key(|(range, _)| range.start);
    let mut reached = 0;
    for (range, _) in &edits {
        if range.start > range.end || range.end > text.len() || !text.is_char_boundary(range.start) || !text.is_char_boundary(range.end) {
            return Err(format!("its edit of {}..{} isn't in the text", range.start, range.end));
        }
        if range.start < reached {
            return Err("its edits overlap".into());
        }
        reached = range.end;
    }
    edits.retain(|(range, new)| text[range.clone()] != **new);
    if edits.is_empty() {
        return Ok(());
    }
    // Where a place in the text ends up: inside an edit, after what it put there.
    let moved = |at: usize| {
        let mut shift = 0isize;
        for (range, new) in &edits {
            if at < range.start || (at == range.start && range.is_empty()) {
                break;
            }
            if at < range.end {
                return (range.start as isize + shift) as usize + new.len();
            }
            shift += new.len() as isize - range.len() as isize;
        }
        (at as isize + shift) as usize
    };
    let selection = tab.view.selection;
    let after = ghostwrite_core::Range::new(moved(selection.anchor), moved(selection.head));
    tab.doc.buffer.begin(selection);
    for (range, new) in edits.iter().rev() {
        tab.doc.buffer.edit(range.clone(), new);
    }
    tab.doc.buffer.commit(after);
    tab.view.selection = after;
    tab.sync();
    Ok(())
}

impl Editor {
    /// `:name args`, which a plugin adds, about the file in view.
    pub(super) fn run_plugin_command(&mut self, name: &str, args: Vec<String>, cx: &mut Context<Self>) {
        let Some(plugin) = ghostwrite_plugin::with_command(name) else { return };
        let root = self.project.root.clone();
        let about = self.tab().map(|tab| (tab.doc.id, tab.doc.buffer.version(), shown(&root, tab)));
        let (file, context) = match about {
            Some((doc, version, shown)) => (Some((doc, version)), shown),
            None => (None, Shown { root: root.display().to_string(), ..Shown::default() }),
        };
        let name = name.to_string();
        let asked = cx.background_spawn(async move { asked_of(plugin, move |plugin| plugin.run(&name, &args, &context)) });
        cx.spawn(async move |this, cx| {
            let reply = asked.await;
            this.update(cx, |this, cx| this.plugin_answered(plugin.name(), file, reply, cx)).ok();
        })
        .detach();
    }

    /// Tell the plugins that want to hear what's happened since they last were.
    pub(super) fn tell_plugins_what_happened(&mut self, cx: &mut Context<Self>) {
        for Heard { event, doc, version, shown } in std::mem::take(&mut self.project.heard) {
            for plugin in ghostwrite_plugin::hearing(event) {
                let shown = shown.clone();
                let asked = cx.background_spawn(async move { asked_of(plugin, move |plugin| plugin.on_event(event, &shown)) });
                cx.spawn(async move |this, cx| {
                    let reply = asked.await;
                    this.update(cx, |this, cx| this.plugin_answered(plugin.name(), Some((doc, version)), reply, cx)).ok();
                })
                .detach();
            }
        }
    }

    /// Do what a plugin answered about `file` (the document, and its version as it
    /// was shown), or say why it can't be done. Whatever it says is marked as its own.
    pub(super) fn plugin_answered(&mut self, name: &str, file: Option<(DocId, u64)>, reply: Reply, cx: &mut Context<Self>) {
        let whose = |said: &str| if name.is_empty() { said.to_string() } else { format!("{name}: {said}") };
        let actions = match reply {
            Ok(actions) => actions,
            Err(why) => {
                self.project.say(whose(&why));
                return cx.notify();
            }
        };
        let mut edits = vec![];
        for action in actions {
            match action {
                Action::Say(said) => self.project.say(whose(&said)),
                Action::Edit { range, text } => edits.push((range, text)),
                Action::Open(path) => {
                    let root = self.project.root.clone();
                    let path = ghostwrite_project::clean(&root.join(path));
                    if ghostwrite_project::within(&root, &path) {
                        self.open_paths(vec![path], cx);
                    } else {
                        self.project.say(whose(&format!("{} isn't in this project", path.display())));
                    }
                }
            }
        }
        if !edits.is_empty()
            && let Err(why) = self.make_plugin_edits(file, edits)
        {
            self.project.say(whose(&why));
        }
        cx.notify();
    }

    fn make_plugin_edits(&mut self, file: Option<(DocId, u64)>, edits: Vec<(Range<usize>, String)>) -> Result<(), String> {
        let (doc, version) = file.ok_or("there's no file to edit")?;
        // Typing goes on as its own step, not as part of this one.
        if self.tab().is_some_and(|tab| tab.doc.id == doc) {
            self.leave_insert();
        }
        let tab = self.project.tabs.by_id_mut(doc).ok_or("the file has been closed")?;
        if tab.doc.buffer.version() != version {
            return Err("the file changed while it was working, so its edit wasn't made".into());
        }
        apply_edits(tab, edits)
    }

    /// `:plugin install`, `update` or `remove`, off the UI thread (a clone takes a while).
    pub(super) fn manage_plugins(&mut self, what: ManagePlugins, cx: &mut Context<Self>) -> Result<(), String> {
        let folder = self.plugins_at.clone().ok_or("there's nowhere to keep plugins here")?;
        // A folder is about where you are. Otherwise it's a repository.
        let what = match what {
            ManagePlugins::Install(source) => {
                let local = ghostwrite_project::resolve(&source, &self.project.root);
                ManagePlugins::Install(if local.is_dir() { local.display().to_string() } else { source })
            }
            other => other,
        };
        let doing = match &what {
            ManagePlugins::Install(source) => format!("installing {source}…"),
            ManagePlugins::Update(_) => "updating plugins…".to_string(),
            ManagePlugins::Remove(_) => String::new(),
        };
        if !doing.is_empty() {
            self.project.say(doing);
        }
        let work = cx.background_spawn(async move { managed(what, &folder) });
        cx.spawn(async move |this, cx| {
            let said = work.await;
            this.update(cx, |this, cx| {
                this.project.say(said);
                cx.notify();
            })
            .ok();
        })
        .detach();
        Ok(())
    }
}

/// Ask `plugin`, so that a panic in one built in can't take the app with it.
fn asked_of(plugin: &'static dyn Plugin, ask: impl FnOnce(&dyn Plugin) -> Reply) -> Reply {
    ghostwrite_project::contained("the plugin", || ask(plugin)).unwrap_or_else(Err)
}

/// What's put in place goes live; what to say of it.
fn managed(what: ManagePlugins, folder: &Path) -> String {
    let adds = |manifest: &Manifest| {
        let later = if manifest.has_rules() { ". Its rules for the model apply from the next start" } else { "" };
        format!("{} ({}){later}", manifest.name, manifest.describe())
    };
    let updated = |name: &str| match install::update(name, folder) {
        Ok(manifest) => {
            let said = format!("updated {}", adds(&manifest));
            ghostwrite_plugin::remove(name);
            ghostwrite_plugin::add(Box::new(manifest));
            said
        }
        Err(why) => format!("{name}: {why}"),
    };
    match what {
        ManagePlugins::Install(source) => match install::install(&source, folder) {
            Ok(manifest) => {
                let said = format!("installed {}", adds(&manifest));
                ghostwrite_plugin::add(Box::new(manifest));
                said
            }
            Err(why) => format!("couldn't install {source}: {why}"),
        },
        ManagePlugins::Update(Some(name)) => updated(&name),
        ManagePlugins::Update(None) => {
            let names = install::updatable(folder);
            if names.is_empty() {
                "there are no installed plugins to update".to_string()
            } else {
                names.iter().map(|name| updated(name)).collect::<Vec<_>>().join(" · ")
            }
        }
        ManagePlugins::Remove(name) => match install::remove(&name, folder) {
            Ok(()) => {
                ghostwrite_plugin::remove(&name);
                format!("removed {name}")
            }
            Err(why) => why,
        },
    }
}
