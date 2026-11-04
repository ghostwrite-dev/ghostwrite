//! Helix's `:` commands: parsing, Tab completion, history, and running them.
//! What's typed is parsed in [`ghostwrite_view::commands`].

use ghostwrite_project::{fs, resolve};
use gpui::{Context, Window};

pub use ghostwrite_view::commands::*;

use super::{Editor, EditorEvent};

impl Editor {
    /// `:api-key`: keep the key safe and ask Claude with it from now on.
    fn api_key(&mut self, key: Option<String>) -> Result<(), String> {
        use ghostwrite_ai::credentials;
        match key.as_deref() {
            None if credentials::saved().is_some() => self.project.say("there's a key saved; `:api-key clear` takes it out"),
            None => self.project.say("`:api-key ‹key›` keeps it where your system keeps secrets: from console.anthropic.com"),
            Some("clear") => {
                credentials::forget();
                self.project.say("the key is gone, and won't be used next time");
            }
            Some(key) => {
                credentials::save(key)?;
                let model = ghostwrite_ai::Backend::Api { key: key.trim().to_string(), model: crate::config::get().model.clone() };
                self.choose_model(model)?;
            }
        }
        Ok(())
    }

    /// ↑ or ↓ in the `:` line.
    pub(super) fn history_step(&mut self, back: bool, cx: &mut Context<Self>) {
        let Some(line) = self.history.step(back) else { return };
        if let Some(prompt) = &self.prompt {
            prompt.field.update(cx, |f, cx| f.set_text(line, cx));
        }
    }

    /// While a model's name is typed: what it could be.
    pub(super) fn suggest_model(&mut self, line: &str, cx: &mut Context<Self>) {
        if let Some(typed) = line.trim_start().strip_prefix("model ") {
            let names = ghostwrite_ai::models::names();
            let hits = ghostwrite_ai::models::matching(&names, typed.trim());
            self.project.status = Some(if hits.is_empty() { format!("no model {typed}") } else { hits.join("  ") }.into());
            cx.notify();
        }
    }

    pub(super) fn complete_command(&mut self, line: &str, cx: &mut Context<Self>) {
        let (completed, candidates) = complete(line, &self.project.root);
        if completed != line
            && let Some(prompt) = &self.prompt
        {
            prompt.field.update(cx, |f, cx| f.set_text(completed, cx));
        }
        self.project.status = (!candidates.is_empty()).then(|| candidates.join("  ").into());
    }

    /// Run a `:` command. `Err` carries the message for the status line.
    pub(super) fn run_command(&mut self, line: &str, window: &mut Window, cx: &mut Context<Self>) -> Result<(), String> {
        let unsaved = |name: &str| format!("{name} has unsaved changes (add ! to discard them)");
        match parse(line)? {
            Command::ApiKey(key) => self.api_key(key)?,
            Command::UndoStory => self.undo_story(cx)?,
            Command::Timings => self.project.say(ghostwrite_ai::timing::summary()),
            Command::Clear => self.clear_context(cx),
            Command::Break => self.toggle_breakpoint(cx)?,
            Command::Debug(line) => self.start_debugging(&line, cx)?,
            Command::DebuggerGo(how) => self.debugger_go(how, cx)?,
            Command::StopDebugging => self.stop_debugging(cx)?,
            Command::Plugins => self.project.say(crate::plugins::list()),
            Command::FromPlugin { name, args } => self.run_plugin_command(&name, args, cx),
            Command::ManagePlugins(what) => self.manage_plugins(what, cx)?,
            Command::Split => self.split_right(cx)?,
            Command::Only => self.close_split(cx),
            Command::Tutor => {
                let ix = self.project.add(None, include_str!("../../../../assets/tutor.md").to_string());
                self.activate(ix, cx);
            }
            Command::Update => self.check_for_update(true, cx),
            Command::Replace { pattern, with, all } => self.start_replace(&pattern, with, all, window, cx)?,
            Command::Open(args) => {
                let paths: Vec<_> = args.iter().map(|a| resolve(a, &self.project.root)).collect();
                if let Some(dir) = paths.iter().find(|p| p.is_dir()) {
                    return Err(format!("{} is a folder", dir.display()));
                }
                self.open_paths(paths, cx);
            }
            Command::Write { path, force, quit, changed_only } => {
                let path = match path {
                    Some(p) => resolve(&p, &self.project.root),
                    None => self.current()?.doc.path.clone().ok_or("no file name yet: use :write <path>")?,
                };
                let ix = self.project.tabs.active_index();
                let doc = &self.current()?.doc;
                let needed = !(changed_only && doc.path.as_ref() == Some(&path) && !doc.dirty);
                if needed && !self.project.write(ix, path, force) {
                    return Err(self.project.status.take().map(|s| s.to_string()).unwrap_or_default());
                }
                if quit {
                    cx.emit(EditorEvent::CloseWindow { force: false });
                }
            }
            Command::WriteAll { quit } if quit => crate::workspace::write_all_and_quit(cx),
            Command::WriteAll { .. } => {
                self.project.write_all();
                if let Some(tab) = self.project.tabs.iter().find(|t| t.doc.dirty) {
                    return Err(format!("couldn't write {}", tab.doc.name()));
                }
                self.project.status = Some("wrote all files".into());
            }
            Command::Quit { force } => cx.emit(EditorEvent::CloseWindow { force }),
            Command::QuitAll { force } => crate::workspace::quit(force, cx),
            Command::Remove { path, force } => {
                let target = match path {
                    Some(p) => resolve(&p, &self.project.root),
                    None => self.current()?.doc.path.clone().ok_or("this buffer has no file to remove")?,
                };
                if !target.exists() {
                    return Err(format!("{} doesn't exist", target.display()));
                }
                let root = &self.project.root;
                if ghostwrite_project::same_file(&target, root) || !target.starts_with(root) {
                    return Err(format!("{} isn't in the project: :rm only removes the project's files", target.display()));
                }
                if target.is_dir() && !force {
                    return Err(format!("{} is a folder (:rm! moves it and everything in it to the Trash)", ghostwrite_project::display(&target, root)));
                }
                let affected = |t: &super::Tab| t.doc.path.as_deref().is_some_and(|p| p.starts_with(&target));
                if let Some(tab) = self.project.tabs.iter().find(|t| affected(t) && t.doc.dirty).filter(|_| !force) {
                    return Err(format!("{} has unsaved changes (use :rm! to remove it anyway)", tab.doc.name()));
                }
                fs::trash(&target)?;
                for tab in self.project.tabs.iter_mut().filter(|t| affected(t)) {
                    tab.doc.dirty = false;
                }
                self.project.path_deleted(&target);
                self.tab_changed(cx);
                self.project.status = Some(format!("moved {} to the Trash", ghostwrite_project::display(&target, &self.project.root)).into());
            }
            Command::Reload { force } => {
                let doc = &self.current()?.doc;
                if doc.dirty && !force {
                    return Err(unsaved(&doc.name()));
                }
                let path = doc.path.clone().ok_or("nothing to reload: this buffer has no file")?;
                let (text, _) = ghostwrite_project::fs::read_text(&path).map_err(|e| format!("couldn't reload: {e}"))?;
                self.leave_insert();
                if let Some(tab) = self.tab_mut() {
                    tab.reload(text);
                }
            }
            Command::Goto(line) => {
                let tab = self.tab_mut().ok_or("no file open")?;
                let line = line.saturating_sub(1).min(tab.doc.buffer.last_line());
                let at = tab.doc.buffer.line_start(line);
                tab.select(ghostwrite_core::Range::point(at), super::Autoscroll::Center(line..line + 1));
            }
            Command::Grep(pattern) => self.start_search(&pattern, window, cx)?,
            Command::Tree => self.toggle_tree(cx),
            Command::Terminal => self.enter_terminal(cx),
            Command::Preview => self.preview(cx)?,
            Command::Format => self.format(|_, _| {}, cx),
            Command::Model(None) => {
                let asking = self.backend.as_ref().map_or("nothing", |b| b.name()).to_string();
                let memory = ghostwrite_model::memory::total().map_or(String::new(), |bytes| {
                    format!(". This Mac has {} GB; local models add about their file's size: Qwen 14B Q3 about 9 GB, 1.5B about 1.5 GB", bytes >> 30)
                });
                self.project.status = Some(format!("asking {asking}. There's {}{memory}", ghostwrite_ai::models::names().join("  ")).into());
            }
            Command::Model(Some(typed)) => self.choose_model(ghostwrite_ai::models::find(&typed)?)?,
            Command::Project(path) => {
                let path = path.map(|p| resolve(&p, &self.project.root));
                if let Some(p) = path.as_ref().filter(|p| !p.is_dir()) {
                    return Err(format!("{} isn't a folder", p.display()));
                }
                cx.emit(EditorEvent::OpenProject(path));
            }
            Command::New => self.new_buffer(cx),
            Command::TabLeft => self.nudge_tab(false, 1, cx),
            Command::TabRight => self.nudge_tab(true, 1, cx),
            Command::BufferNext => self.cycle_tab(true, cx),
            Command::BufferPrev => self.cycle_tab(false, cx),
            Command::BufferClose { force } => {
                let doc = &self.current()?.doc;
                if doc.dirty && !force {
                    return Err(unsaved(&doc.name()));
                }
                self.close_active(cx);
            }
        }
        Ok(())
    }
}
