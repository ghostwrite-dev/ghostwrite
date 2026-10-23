//! Gruvbox, light and dark, following the system appearance.

use gpui_kit::App;
use gpui_kit::component::{Theme, ThemeRegistry};

const GRUVBOX: &str = include_str!("../assets/gruvbox.json");

pub fn install(cx: &mut App) {
    ThemeRegistry::global_mut(cx).load_themes_from_str(GRUVBOX).expect("the bundled gruvbox theme parses");
    let themes = ThemeRegistry::global(cx).themes();
    let (light, dark) = (themes["Gruvbox Light"].clone(), themes["Gruvbox Dark"].clone());
    // Make sure the global theme exists before swapping its light and dark halves.
    Theme::sync_system_appearance(None, cx);
    let theme = Theme::global_mut(cx);
    theme.light_theme = light;
    theme.dark_theme = dark;
    Theme::sync_system_appearance(None, cx);
}
