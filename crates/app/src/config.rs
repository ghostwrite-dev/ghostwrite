//! User settings, read once at launch from `~/.config/ghostwrite/config.json`.
//! A missing or broken file, or a missing key, means the default.

use std::path::PathBuf;
use std::sync::OnceLock;

pub struct Config {
    /// Files bigger than this aren't opened, so a stray log or binary can't hang the editor.
    pub max_file_size: u64,
    pub tab_width: usize,
    /// Lines kept between the cursor and the edge when scrolling to it.
    pub margin_lines: f32,
    pub model: String,
    /// Bytes of the file around the marked lines sent to Claude.
    pub context_bytes: usize,
    /// Bytes of uncommitted changes sent to Claude.
    pub max_diff: usize,
    pub max_recent_projects: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            max_file_size: 20 * 1024 * 1024,
            tab_width: 4,
            margin_lines: 3.,
            model: "claude-opus-5-5".into(),
            context_bytes: 200_000,
            max_diff: 12_000,
            max_recent_projects: 20,
        }
    }
}

pub fn path() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("HOME")?).join(".config/ghostwrite/config.json"))
}

/// The settings, loaded on first use.
pub fn get() -> &'static Config {
    static CONFIG: OnceLock<Config> = OnceLock::new();
    CONFIG.get_or_init(load)
}

fn load() -> Config {
    let mut config = Config::default();
    let Some(json) = path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
    else {
        return config;
    };
    if let Some(mb) = json.get("max_file_size_mb").and_then(|v| v.as_u64()) {
        config.max_file_size = mb * 1024 * 1024;
    }
    let usize_of = |key: &str| json.get(key).and_then(|v| v.as_u64()).map(|n| n as usize);
    if let Some(n) = usize_of("tab_width").filter(|&n| n > 0) {
        config.tab_width = n;
    }
    if let Some(n) = json.get("margin_lines").and_then(|v| v.as_f64()) {
        config.margin_lines = n as f32;
    }
    if let Some(model) = json.get("model").and_then(|v| v.as_str()) {
        config.model = model.to_string();
    }
    if let Some(n) = usize_of("context_bytes") {
        config.context_bytes = n;
    }
    if let Some(n) = usize_of("max_diff") {
        config.max_diff = n;
    }
    if let Some(n) = usize_of("max_recent_projects") {
        config.max_recent_projects = n;
    }
    config
}
