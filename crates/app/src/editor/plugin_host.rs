//! The editor's side of plugins: `:` commands they add, telling them what happens to
//! files, carrying out what they answer, and `:plugin`. They're asked off the
//! UI thread, so a slow one holds up nothing, and answer with what they'd like done
//! ([`Action`]): an edit of text that's changed since they were shown it isn't made.

use std::ops::Range;
use std::path::Path;

use ghostwrite_plugin::manifest::Manifest;
use ghostwrite_plugin::{Action, Context as Shown, Plugin, Reply, install};
pub(super) use ghostwrite_state::plugin_host::apply_edits;
use ghostwrite_state::plugin_host::{Heard, shown};
use gpui::{AppContext as _, Context};

use super::commands::ManagePlugins;
use super::{DocId, Editor};

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

    /// `:plugin install`, `link`, `update` or `remove`, off the UI thread (a download or
    /// a clone takes a while). Installing is in two steps: what's fetched is checked and
    /// said what it can do, and only `:plugin install!` puts it in place.
    pub(super) fn manage_plugins(&mut self, what: ManagePlugins, cx: &mut Context<Self>) -> Result<(), String> {
        let folder = self.plugins_at.clone().ok_or("there's nowhere to keep plugins here")?;
        // A folder is about where you are. Otherwise it's a repository.
        let here = |source: String| {
            let local = ghostwrite_project::resolve(&source, &self.project.root);
            if local.is_dir() { local.display().to_string() } else { source }
        };
        let what = match what {
            ManagePlugins::Install { source, confirmed } => ManagePlugins::Install { source: here(source), confirmed },
            ManagePlugins::Link(source) => ManagePlugins::Link(here(source)),
            other => other,
        };
        // What was fetched for this, if it's to be installed now; anything else fetched is let go.
        let staged = match (&what, self.staged_plugin.take()) {
            (ManagePlugins::Install { source, confirmed: true }, Some((fetched, staged))) if *source == fetched => Some(staged),
            _ => None,
        };
        let doing = match &what {
            ManagePlugins::Install { source, .. } if staged.is_none() => format!("fetching {source}…"),
            ManagePlugins::Update(_) => "updating plugins…".to_string(),
            _ => String::new(),
        };
        if !doing.is_empty() {
            self.project.say(doing);
        }
        let work = cx.background_spawn(async move { managed(what, staged, &folder) });
        cx.spawn(async move |this, cx| {
            let done = work.await;
            this.update(cx, |this, cx| {
                match done {
                    Managed::Said(said) => this.project.say(said),
                    Managed::Staged(source, staged) => {
                        let manifest = &staged.manifest;
                        let unchecked = staged.unchecked.as_ref().map_or_else(String::new, |why| format!(". {why}"));
                        let clashes: String = this.clashes_if_added(manifest).into_iter().map(|clash| format!(". {clash}")).collect();
                        this.project.say(format!("{} ({}){unchecked}{clashes}. :plugin install! {source} to install it", manifest.name, manifest.describe()));
                        this.staged_plugin = Some((source, *staged));
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        Ok(())
    }

    /// The views of `manifest` whose keys would be taken, were it installed (in place of
    /// one of its name, if there's one), as they'd be said.
    pub(super) fn clashes_if_added(&self, manifest: &Manifest) -> Vec<String> {
        let mut views: Vec<_> = ghostwrite_plugin::views().into_iter().filter(|view| view.id.plugin != manifest.name).collect();
        views.extend_from_slice(manifest.views());
        let (keymap, _) = super::keymap_for(&views);
        keymap.clashes().into_iter().filter(|clash| clash.view.id.plugin == manifest.name).map(|clash| clash.said()).collect()
    }

    /// Look at the linked plugins every second, and read again those that have changed.
    pub(super) fn watch_linked_plugins(&mut self, cx: &mut Context<Self>) {
        self._watching_links = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(std::time::Duration::from_secs(1)).await;
                if this.update(cx, |this, cx| this.reload_linked_plugins(cx)).is_err() {
                    break;
                }
            }
        }));
    }

    /// Read again the linked plugins that have changed since they were last looked at.
    pub(super) fn reload_linked_plugins(&mut self, cx: &mut Context<Self>) {
        let Some(folder) = &self.plugins_at else { return };
        let changed = LINKS.lock().unwrap_or_else(|e| e.into_inner()).changed(folder);
        for (name, read) in changed {
            match read.and_then(|manifest| manifest.check().map(|()| manifest)) {
                Ok(manifest) => {
                    ghostwrite_plugin::remove(&name);
                    ghostwrite_plugin::add(Box::new(manifest));
                    self.project.say(format!("{name} changed, and was read again"));
                }
                Err(why) => self.project.say(format!("{name} changed, and can't be read: {why}")),
            }
            cx.notify();
        }
    }

    /// When a file opens that no plugin handles, say which plugin in the catalog would,
    /// once for each kind of file. It's looked up off the UI thread, and kept for a day.
    pub(super) fn suggest_plugin(&mut self, cx: &mut Context<Self>) {
        let Some((url, kept)) = self.catalog.clone() else { return };
        let Some(tab) = self.tab().filter(|tab| tab.doc.language.is_none()) else { return };
        let Some(extension) = tab.doc.path.as_ref().and_then(|path| path.extension()).map(|e| e.to_string_lossy().to_lowercase()) else { return };
        if !SUGGESTED.lock().unwrap_or_else(|e| e.into_inner()).insert(extension.clone()) {
            return;
        }
        let looking = cx.background_spawn(async move {
            let entries = ghostwrite_plugin::catalog::read(&url, &kept).unwrap_or_default();
            ghostwrite_plugin::catalog::for_extension(&entries, &extension).map(|entry| entry.suggestion())
        });
        cx.spawn(async move |this, cx| {
            if let Some(said) = looking.await {
                this.update(cx, |this, cx| {
                    this.project.say(said);
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }
}

/// The linked plugins, as they were last looked at.
static LINKS: std::sync::Mutex<install::Watch> = std::sync::Mutex::new(install::Watch::new());
/// The kinds of file a plugin's been suggested for, this time the editor's run.
static SUGGESTED: std::sync::LazyLock<std::sync::Mutex<std::collections::HashSet<String>>> = std::sync::LazyLock::new(Default::default);

/// What came of `:plugin`: what to say, or a plugin fetched, to be said what it can do.
enum Managed {
    Said(String),
    Staged(String, Box<install::Staged>),
}

/// Ask `plugin`, so that a panic in one built in can't take the app with it.
fn asked_of(plugin: &'static dyn Plugin, ask: impl FnOnce(&dyn Plugin) -> Reply) -> Reply {
    ghostwrite_project::contained("the plugin", || ask(plugin)).unwrap_or_else(Err)
}

/// What's put in place goes live; what to say of it.
fn managed(what: ManagePlugins, staged: Option<install::Staged>, folder: &Path) -> Managed {
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
    let added = |installed: Result<Manifest, String>, how: &str, source: &str| match installed {
        Ok(manifest) => {
            let said = format!("{how} {}", adds(&manifest));
            ghostwrite_plugin::add(Box::new(manifest));
            said
        }
        Err(why) => format!("couldn't {} {source}: {why}", if how == "linked" { "link" } else { "install" }),
    };
    Managed::Said(match what {
        ManagePlugins::Install { source, confirmed: false } => match install::fetch(&source, folder) {
            Ok(staged) => return Managed::Staged(source, Box::new(staged)),
            Err(why) => format!("couldn't install {source}: {why}"),
        },
        ManagePlugins::Install { source, confirmed: true } => {
            let installed = match staged {
                Some(staged) => staged.install(folder),
                None => install::install(&source, folder),
            };
            added(installed, "installed", &source)
        }
        ManagePlugins::Link(source) => {
            let linked = install::link(Path::new(&source), folder);
            // Looked at now, so that a change from now on is seen.
            LINKS.lock().unwrap_or_else(|e| e.into_inner()).changed(folder);
            added(linked, "linked", &source)
        }
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
    })
}
