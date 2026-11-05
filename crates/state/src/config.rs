//! User settings, read once at launch from `~/.config/ghostwrite/config.json`.
//! A missing or broken file, or a missing key, means the default; keys
//! that can't be bound are said at launch, and keep theirs.

use std::path::PathBuf;
use std::sync::OnceLock;

use ghostwrite_core::{Keymap, PluginView, ViewKind};

/// A key of yours: the view it's in (none: the code's), the keys, and what they're bound to.
pub type YourKey = (Option<String>, String, String);

pub struct Config {
    /// Files bigger than this aren't opened, so a stray log or binary can't hang the editor.
    pub max_file_size: u64,
    pub tab_width: usize,
    /// The code text's size, in points, before zooming.
    pub font_size: f32,
    /// Lines wrap at this many columns, or sooner if the window is narrower.
    pub wrap_column: usize,
    /// Lines kept between the cursor and the edge when scrolling to it.
    pub margin_lines: f32,
    /// The Claude model asked through the API or Claude Code.
    pub model: String,
    /// A local model to offer, asked at first unless another's been chosen with
    /// `:model`: Hugging Face's `user/repo:quantization` or a `.gguf` file.
    pub local_model: Option<String>,
    /// Bytes of the file around the marked lines sent to the model.
    pub context_bytes: usize,
    /// Bytes of uncommitted changes sent to the model.
    pub max_diff: usize,
    pub max_recent_projects: usize,
    /// Format a file with its language server when it's saved (`:w`, ⌘S).
    pub format_on_save: bool,
    /// Whether a view lights up when a story moves to it.
    pub animations: bool,
    /// The debug adapter `:debug` runs: the program and its arguments.
    pub debugger: Vec<String>,
    /// Your own keys in normal and select mode: a `:` command (`"ctrl-s":
    /// ":w"`, `"space w": ":wa"`), a command by name (`"x":
    /// "extend_line_below"`), a plugin's view by its name (`"space p": "focus_view
    /// pages"`), or `"no_op"` to unbind a key. Said flat (no view), they're the code's,
    /// and every view's under its own; under a view's name, that view's alone:
    /// `"files": { "r": "rename_file" }`, `"terminal"`, `"debugger"`, or a plugin's
    /// view's, `"goals"`. See [`Config::keymap`].
    pub keys: Vec<YourKey>,
    /// What's wrong with your keys that can be said without the plugins, if anything is.
    pub problems: Vec<String>,
    /// The theme for light and dark appearance, by name, from `themes/` beside the config.
    pub theme_light: Option<String>,
    pub theme_dark: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            max_file_size: 20 * 1024 * 1024,
            tab_width: 4,
            font_size: 13.,
            wrap_column: 100,
            margin_lines: 3.,
            model: "claude-opus-5-5".into(),
            local_model: None,
            context_bytes: 200_000,
            max_diff: 12_000,
            max_recent_projects: 20,
            format_on_save: true,
            animations: true,
            debugger: if cfg!(target_os = "macos") { vec!["/usr/bin/xcrun".into(), "lldb-dap".into()] } else { vec!["lldb-dap".into()] },
            keys: vec![],
            problems: vec![],
            theme_light: None,
            theme_dark: None,
        }
    }
}

pub fn path() -> Option<PathBuf> {
    Some(ghostwrite_project::platform::config_dir()?.join("config.json"))
}

/// The settings, loaded on first use.
pub fn get() -> &'static Config {
    static CONFIG: OnceLock<Config> = OnceLock::new();
    CONFIG.get_or_init(load)
}

fn load() -> Config {
    match path().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok()) {
        Some(json) => from_json(&json),
        None => Config::default(),
    }
}

/// The settings `json` gives.
fn from_json(json: &serde_json::Value) -> Config {
    let mut config = Config::default();
    if let Some(animations) = json.get("animations").and_then(|v| v.as_bool()) {
        config.animations = animations;
    }
    if let Some(debugger) = json.get("debugger").and_then(|v| v.as_array()) {
        config.debugger = debugger.iter().filter_map(|word| Some(word.as_str()?.to_string())).collect();
    }
    if let Some(keys) = json.get("keys").and_then(|v| v.as_object()) {
        (config.keys, config.problems) = yours(keys);
    }
    config.theme_light = json.get("theme_light").and_then(|v| v.as_str()).map(str::to_string);
    config.theme_dark = json.get("theme_dark").and_then(|v| v.as_str()).map(str::to_string);
    if let Some(mb) = json.get("max_file_size_mb").and_then(|v| v.as_u64()) {
        config.max_file_size = mb * 1024 * 1024;
    }
    let usize_of = |key: &str| json.get(key).and_then(|v| v.as_u64()).map(|n| n as usize);
    if let Some(n) = usize_of("tab_width").filter(|&n| n > 0) {
        config.tab_width = n;
    }
    if let Some(n) = usize_of("wrap_column").filter(|&n| n > 0) {
        config.wrap_column = n;
    }
    if let Some(n) = json.get("font_size").and_then(|v| v.as_f64()).filter(|&n| (6. ..=48.).contains(&n)) {
        config.font_size = n as f32;
    }
    if let Some(n) = json.get("margin_lines").and_then(|v| v.as_f64()) {
        config.margin_lines = n as f32;
    }
    let string_of = |key: &str| json.get(key).and_then(|v| v.as_str()).filter(|s| !s.trim().is_empty()).map(str::to_string);
    if let Some(model) = string_of("model") {
        config.model = model;
    }
    config.local_model = string_of("local_model");
    if let Some(n) = usize_of("context_bytes") {
        config.context_bytes = n;
    }
    if let Some(on) = json.get("format_on_save").and_then(|v| v.as_bool()) {
        config.format_on_save = on;
    }
    if let Some(n) = usize_of("max_diff") {
        config.max_diff = n;
    }
    if let Some(n) = usize_of("max_recent_projects") {
        config.max_recent_projects = n;
    }
    config
}

impl Config {
    /// The keymap: the editor's keys, the keys plugins' `views` declare where they're
    /// free, and yours over them all. And what's wrong with yours: a view under whose
    /// name they are is one of the editor's or one of those.
    pub fn keymap(&self, views: &[PluginView]) -> (Keymap, Vec<String>) {
        let mut keymap = Keymap::default().with_views(views);
        let mut problems = self.problems.clone();
        let mut names: Vec<Option<&str>> = vec![None];
        for (view, ..) in &self.keys {
            if !names.contains(&view.as_deref()) {
                names.push(view.as_deref());
            }
        }
        for name in names {
            let view = match name {
                None => ViewKind::Code,
                Some(name) => match keymap.view_named(name) {
                    Some(view) => view,
                    None => {
                        let mut views: Vec<&str> = ViewKind::BUILT_IN.iter().map(|view| view.name()).collect();
                        views.extend(keymap.views().iter().map(|view| view.id.name).filter(|name| *name != "code"));
                        views.dedup();
                        problems.push(format!("\"{name}\": a command, or a view ({}) and its keys", views.join(", ")));
                        continue;
                    }
                },
            };
            let theirs: Vec<(String, String)> =
                self.keys.iter().filter(|(of, ..)| of.as_deref() == name).map(|(_, keys, to)| (keys.clone(), to.clone())).collect();
            let wrong;
            (keymap, wrong) = keymap.with_yours_in(view, &theirs);
            problems.extend(wrong.into_iter().map(|wrong| match name {
                None => wrong,
                Some(name) => format!("{name}: {wrong}"),
            }));
        }
        (keymap, problems)
    }
}

/// Your keys, by view: those said flat are the code's, and those under a view's name
/// (`"files": { … }`) that view's. And what isn't either.
fn yours(keys: &serde_json::Map<String, serde_json::Value>) -> (Vec<YourKey>, Vec<String>) {
    let (mut found, mut wrong) = (vec![], vec![]);
    for (key, to) in keys {
        match to {
            serde_json::Value::String(to) => found.push((None, key.clone(), to.clone())),
            serde_json::Value::Object(theirs) => {
                found.extend(theirs.iter().filter_map(|(keys, to)| Some((Some(key.clone()), keys.clone(), to.as_str()?.to_string()))));
            }
            _ => wrong.push(format!("\"{key}\": a command, or a view and its keys")),
        }
    }
    (found, wrong)
}

#[cfg(test)]
mod tests {
    use ghostwrite_core::{Key, Modal, PluginView, Shows, ViewId};

    #[test]
    fn your_keys_are_bound_and_those_that_cant_be_are_said() {
        let json = serde_json::json!({ "keys": { "ctrl-s": ":w", "Z": "extend_line_below", "x": "no_op", "q": "extend_lin_below" } });
        let (modal, problems) = modal_of(&json, &[]);
        assert_eq!(modal.command_for(Key::Char('Z')), Some(ghostwrite_core::Command::ExtendLineBelow));
        assert!(!modal.binds(Key::Char('x')), "unbound");
        assert!(modal.binds(Key::Ctrl('s')));
        assert_eq!(modal.command_for(Key::Char('q')), Some(ghostwrite_core::Command::ReplayMacro), "as it was");
        assert_eq!(problems, ["\"q\": no command \"extend_lin_below\" (a : command starts with :, a plugin's view is focus_view and its name)"]);
    }

    #[test]
    fn keys_that_lead_on_to_others_cant_be_bound_and_that_is_said() {
        let json = serde_json::json!({ "keys": { "space w": ":wa", "space W": ":wa", "files": { "space w": "undo" } } });
        let (modal, problems) = modal_of(&json, &[]);
        assert_eq!(problems.len(), 2, "{problems:?}");
        assert!(
            problems[0].starts_with("\"space w\": those keys lead on to others (space w v, space w w, space w q, space w o), so they can't be bound"),
            "{problems:?}"
        );
        assert!(problems[1].starts_with("files: \"space w\": those keys lead on to others"), "{problems:?}");
        assert!(modal.binds(Key::Char(' ')), "space is as it was");
    }

    /// A plugin's view of pages, as the tests have it.
    const PAGES: PluginView = PluginView {
        id: ViewId { plugin: "typeset", name: "pages", shows: Shows::Pages },
        title: "Pages",
        place: ghostwrite_core::ViewPlace::Tab,
        key: Some("space v"),
    };

    /// A modal with the keys `json` says over the editor's and `views`', and what's wrong with them.
    fn modal_of(json: &serde_json::Value, views: &[PluginView]) -> (Modal, Vec<String>) {
        let config = super::from_json(json);
        let (keymap, problems) = config.keymap(views);
        let mut modal = Modal::default();
        modal.set_keymap(Box::leak(Box::new(keymap)));
        (modal, problems)
    }

    #[test]
    fn your_keys_can_be_a_views_own() {
        use ghostwrite_core::ViewKind;
        let json = serde_json::json!({ "keys": {
            "x": "extend_line_below",
            "files": { "R": "rename_file", "r": "no_op" },
            "terminal": { "Y": ":w" },
            "pages": { "J": "scroll_down", "q": "no_such" },
            "nowhere": { "a": "undo" },
            "space p": "focus_view pages",
        } });
        let (mut modal, problems) = modal_of(&json, &[PAGES]);
        modal.view = ViewKind::Files;
        assert_eq!(modal.command_for(Key::Char('R')), Some(ghostwrite_core::Command::RenameFile));
        assert_eq!(modal.command_for(Key::Char('r')), Some(ghostwrite_core::Command::NoOp), "hidden in Files");
        assert_eq!(modal.command_for(Key::Char('x')), Some(ghostwrite_core::Command::ExtendLineBelow), "the flat ones are every view's");
        modal.view = ViewKind::Code;
        assert_eq!(modal.command_for(Key::Char('R')), Some(ghostwrite_core::Command::ReplaceWithYanked), "and a view's are its own");
        modal.view = ViewKind::Terminal;
        assert!(modal.binds(Key::Char('Y')));
        modal.view = PAGES.kind();
        assert_eq!(modal.command_for(Key::Char('J')), Some(ghostwrite_core::Command::ScrollDown), "a plugin's view's, by its name");
        assert_eq!(problems.len(), 2, "{problems:?}");
        assert!(problems.iter().any(|p| p.starts_with("\"nowhere\": a command, or a view (code, files, terminal, debugger, pages)")), "{problems:?}");
        assert!(problems.iter().any(|p| p.starts_with("pages: \"q\": no command")), "{problems:?}");
        let (_, problems) = modal_of(&json, &[]);
        assert!(problems.iter().any(|p| p.starts_with("\"pages\": a command, or a view")), "with no plugin that has it: {problems:?}");
        assert!(problems.iter().any(|p| p.starts_with("\"space p\": no plugin has a view \"pages\"")), "{problems:?}");
    }
}
