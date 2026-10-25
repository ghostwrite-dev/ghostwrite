//! Gruvbox, light and dark, following the system appearance. The bundled
//! theme file has more than Ghostwrite uses; this picks out the colors the
//! UI draws with and the syntax styles.

use std::collections::HashMap;

use gpui::{App, FontWeight, Global, Hsla, Pixels, Rems, Rgba, SharedString, Window, WindowAppearance, px, rems};
use serde_json::Value;

const GRUVBOX: &str = include_str!("../../../assets/gruvbox.json");
/// How far the window zooms in and out, a level at a time, as browsers do.
const ZOOMS: [f32; 13] = [0.5, 0.67, 0.75, 0.8, 0.9, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0, 2.5, 3.0];
/// A rem at 100%: what [`pt`] sizes are relative to.
const REM: f32 = 16.;

#[derive(Clone, Copy, Debug)]
pub struct SyntaxStyle {
    pub color: Option<Hsla>,
    pub italic: bool,
    pub bold: bool,
}

#[derive(Clone, Debug)]
pub struct Theme {
    pub background: Hsla,
    pub foreground: Hsla,
    pub muted: Hsla,
    pub muted_foreground: Hsla,
    pub border: Hsla,
    /// The row under the keyboard (tree, menus).
    pub accent: Hsla,
    pub accent_foreground: Hsla,
    pub selection: Hsla,
    pub sidebar: Hsla,
    pub sidebar_foreground: Hsla,
    pub status_bar: Hsla,
    pub popover: Hsla,
    pub tab_bar: Hsla,
    pub tab_active: Hsla,
    pub tab_foreground: Hsla,
    pub tab_active_foreground: Hsla,
    pub active_line: Hsla,
    pub line_number: Hsla,
    pub active_line_number: Hsla,
    pub danger: Hsla,
    pub warning: Hsla,
    pub success: Hsla,
    pub info: Hsla,
    pub hint: Hsla,
    /// `ui.cursor.primary.{normal,insert,select}` from helix's gruvbox, and the char inside.
    pub cursor_normal: Hsla,
    pub cursor_insert: Hsla,
    pub cursor_select: Hsla,
    pub cursor_text: Hsla,
    pub syntax: HashMap<String, SyntaxStyle>,
    pub mono_font: SharedString,
    /// The code text's size, zoomed.
    pub mono_size: Pixels,
    pub ui_font: SharedString,
    /// How far the window is zoomed: 1 is 100%.
    pub zoom: f32,
}

impl Global for Theme {}

/// Both halves, to switch between when the appearance changes.
struct Themes {
    light: Theme,
    dark: Theme,
}

impl Global for Themes {}

pub fn theme(cx: &App) -> &Theme {
    cx.global::<Theme>()
}

/// Load the themes and pick the one for `appearance`.
pub fn install(appearance: WindowAppearance, cx: &mut App) {
    let file: Value = serde_json::from_str(GRUVBOX).expect("the bundled theme parses");
    let named = |name: &str| file["themes"].as_array().and_then(|all| all.iter().find(|t| t["name"] == name)).expect("both gruvbox themes are bundled");
    let themes = Themes { light: parse(named("Gruvbox Light")), dark: parse(named("Gruvbox Dark")) };
    cx.set_global(themes);
    follow(appearance, cx);
}

/// Switch to the light or dark half.
pub fn follow(appearance: WindowAppearance, cx: &mut App) {
    let dark = matches!(appearance, WindowAppearance::Dark | WindowAppearance::VibrantDark);
    let themes = cx.global::<Themes>();
    let theme = if dark { themes.dark.clone() } else { themes.light.clone() };
    cx.set_global(theme);
}

/// A size in points at 100%, which zooms with the window: everything but
/// hairlines is sized this way (or from the code text's size).
pub fn pt(points: f32) -> Rems {
    rems(points / REM)
}

/// Zoom the whole window a level in (`by` 1) or out (-1), or back to 100% (0).
pub fn zoom(by: i32, cx: &mut App) {
    let now = ZOOMS.iter().position(|&z| z >= theme(cx).zoom - 0.001).unwrap_or(ZOOMS.len() - 1);
    let zoom = match by {
        0 => 1.0,
        _ => ZOOMS[now.saturating_add_signed(by as isize).min(ZOOMS.len() - 1)],
    };
    let themes = cx.global_mut::<Themes>();
    themes.light.zoom_to(zoom);
    themes.dark.zoom_to(zoom);
    cx.global_mut::<Theme>().zoom_to(zoom);
    cx.refresh_windows();
}

/// Follow the window's appearance from now on.
pub fn observe(window: &mut Window) {
    window
        .observe_window_appearance(|window, cx| {
            follow(window.appearance(), cx);
            window.refresh();
        })
        .detach();
}

fn color(hex: &str) -> Option<Hsla> {
    let hex = hex.strip_prefix('#')?;
    let value = u32::from_str_radix(hex, 16).ok()?;
    let rgba = match hex.len() {
        6 => Rgba { r: ((value >> 16) & 0xff) as f32 / 255., g: ((value >> 8) & 0xff) as f32 / 255., b: (value & 0xff) as f32 / 255., a: 1. },
        8 => Rgba {
            r: ((value >> 24) & 0xff) as f32 / 255.,
            g: ((value >> 16) & 0xff) as f32 / 255.,
            b: ((value >> 8) & 0xff) as f32 / 255.,
            a: (value & 0xff) as f32 / 255.,
        },
        _ => return None,
    };
    Some(rgba.into())
}

fn parse(theme: &Value) -> Theme {
    let dark = theme["mode"] == "dark";
    let colors = &theme["colors"];
    let highlight = &theme["highlight"];
    let get = |key: &str| colors[key].as_str().or_else(|| highlight[key].as_str()).and_then(color);
    let pick = |keys: &[&str]| keys.iter().find_map(|k| get(k)).unwrap_or(gpui::black());
    let syntax = highlight["syntax"]
        .as_object()
        .map(|styles| {
            styles
                .iter()
                .map(|(name, style)| {
                    let parsed = SyntaxStyle {
                        color: style["color"].as_str().and_then(color),
                        italic: style["font_style"] == "italic",
                        bold: style["font_weight"].as_u64().is_some_and(|w| w >= 600),
                    };
                    (name.clone(), parsed)
                })
                .collect()
        })
        .unwrap_or_default();
    // Helix's gruvbox cursors (fg3, blue1, orange1 on bg1).
    let (fg3, blue1, orange1, bg1) = if dark { (0xbdae93, 0x83a598, 0xfe8019, 0x3c3836) } else { (0x665c54, 0x076678, 0xaf3a03, 0xebdbb2) };
    Theme {
        background: pick(&["editor.background", "background"]),
        foreground: pick(&["editor.foreground", "foreground"]),
        muted: pick(&["muted.background"]),
        muted_foreground: pick(&["muted.foreground"]),
        border: pick(&["border"]),
        accent: pick(&["list.active.background", "accent.background"]),
        accent_foreground: pick(&["accent.foreground", "foreground"]),
        selection: pick(&["selection.background"]),
        sidebar: pick(&["sidebar.background"]),
        sidebar_foreground: pick(&["sidebar.foreground"]),
        status_bar: pick(&["status_bar.background", "title_bar.background"]),
        popover: pick(&["popover.background"]),
        tab_bar: pick(&["tab_bar.background"]),
        tab_active: pick(&["tab.active.background"]),
        tab_foreground: pick(&["tab.foreground"]),
        tab_active_foreground: pick(&["tab.active.foreground"]),
        active_line: pick(&["editor.active_line.background"]),
        line_number: pick(&["editor.line_number"]),
        active_line_number: pick(&["editor.active_line_number"]),
        danger: pick(&["error.border", "base.red.light"]),
        warning: pick(&["warning.border", "base.yellow.light"]),
        success: pick(&["created", "base.green.light"]),
        info: pick(&["info.border", "base.blue.light"]),
        hint: pick(&["hint.border", "base.cyan.light"]),
        cursor_normal: gpui::rgb(fg3).into(),
        cursor_insert: gpui::rgb(blue1).into(),
        cursor_select: gpui::rgb(orange1).into(),
        cursor_text: gpui::rgb(bg1).into(),
        syntax,
        mono_font: "Menlo".into(),
        mono_size: px(crate::config::get().font_size),
        ui_font: ".SystemUIFont".into(),
        zoom: 1.0,
    }
}

/// Our scope names that the theme spells differently.
const ALIASES: &[(&str, &str)] = &[
    ("text.title", "title"),
    ("text.emphasis", "emphasis"),
    ("text.strong", "emphasis.strong"),
    ("text.uri", "link_uri"),
    ("text.reference", "link_text"),
    ("escape", "string.escape"),
    ("function.macro", "function"),
    ("constant.builtin", "constant"),
    ("variable.builtin", "variable.special"),
    ("type.builtin", "type"),
];

impl Theme {
    fn zoom_to(&mut self, zoom: f32) {
        self.zoom = zoom;
        self.mono_size = px(crate::config::get().font_size * zoom);
    }

    /// The size of a rem, which [`pt`] sizes and gpui's text and spacing follow.
    pub fn rem_size(&self) -> Pixels {
        px(REM * self.zoom)
    }

    /// The style for a highlight scope: the most specific name the theme has.
    pub fn syntax_style(&self, scope: &str) -> Option<SyntaxStyle> {
        let scope = ALIASES.iter().find(|(ours, _)| *ours == scope).map_or(scope, |(_, theirs)| theirs);
        let mut name = scope;
        loop {
            if let Some(style) = self.syntax.get(name) {
                return Some(*style);
            }
            name = &name[..name.rfind('.')?];
        }
    }

    pub fn weight(style: &SyntaxStyle) -> FontWeight {
        if style.bold { FontWeight::BOLD } else { FontWeight::NORMAL }
    }
}
