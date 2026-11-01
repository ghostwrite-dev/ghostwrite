//! User settings, read once at launch from `~/.config/ghostwrite/config.json`.
//! A missing or broken file, or a missing key, means the default.

use std::path::PathBuf;
use std::sync::OnceLock;

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
    /// The Claude model asked.
    pub model: String,
    /// A local model to offer, asked at first unless another's been chosen with
    /// `:model`: Hugging Face's `user/repo:quantization` or a `.gguf` file.
    pub local_model: Option<String>,
    /// Bytes of the file around the marked lines sent to Claude.
    pub context_bytes: usize,
    /// Bytes of uncommitted changes sent to Claude.
    pub max_diff: usize,
    pub max_recent_projects: usize,
    /// Format a file with its language server when it's saved (`:w`, ⌘S).
    pub format_on_save: bool,
    /// Whether a panel lights up when a story moves to it.
    pub animations: bool,
    /// The debug adapter `:debug` runs: the program and its arguments.
    pub debugger: Vec<String>,
    /// Your own keys in normal and select mode: `"ctrl-s": ":w"`, `"space w": ":wa"`.
    pub keys: Vec<(String, String)>,
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
    let mut config = Config::default();
    let Some(json) = path().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok()) else {
        return config;
    };
    if let Some(animations) = json.get("animations").and_then(|v| v.as_bool()) {
        config.animations = animations;
    }
    if let Some(debugger) = json.get("debugger").and_then(|v| v.as_array()) {
        config.debugger = debugger.iter().filter_map(|word| Some(word.as_str()?.to_string())).collect();
    }
    if let Some(keys) = json.get("keys").and_then(|v| v.as_object()) {
        config.keys = keys.iter().filter_map(|(key, command)| Some((key.clone(), command.as_str()?.trim_start_matches(':').to_string()))).collect();
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
