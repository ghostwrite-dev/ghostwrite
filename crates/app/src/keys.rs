//! gpui keystrokes as the modal editor's keys.

use ghostwrite_core::Key;
use gpui::Keystroke;

/// `None` for keys the modal editor never sees (anything with cmd).
pub fn key_of(ks: &Keystroke) -> Option<Key> {
    let m = &ks.modifiers;
    if m.platform || m.function {
        return None;
    }
    let single = |s: &str| {
        let mut chars = s.chars();
        chars.next().filter(|_| chars.next().is_none())
    };
    if m.control {
        return single(&ks.key).map(Key::Ctrl);
    }
    if m.alt {
        return single(&ks.key).map(Key::Alt);
    }
    let named = match ks.key.as_str() {
        "escape" => Some(Key::Escape),
        "enter" => Some(Key::Enter),
        "tab" if m.shift => Some(Key::BackTab),
        "tab" => Some(Key::Tab),
        "backspace" => Some(Key::Backspace),
        "delete" => Some(Key::Delete),
        "left" => Some(Key::Left),
        "right" => Some(Key::Right),
        "up" => Some(Key::Up),
        "down" => Some(Key::Down),
        "home" => Some(Key::Home),
        "end" => Some(Key::End),
        "pageup" => Some(Key::PageUp),
        "pagedown" => Some(Key::PageDown),
        "space" => Some(Key::Char(' ')),
        _ => None,
    };
    if named.is_some() {
        return named;
    }
    // What was typed; else a one-character key name (some sources send no `key_char`).
    let typed = ks.key_char.as_deref().map_or_else(|| single(&ks.key).map(|c| if m.shift { c.to_ascii_uppercase() } else { c }), single);
    typed.filter(|c| !c.is_control()).map(Key::Char)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn translates_keystrokes() {
        let k = |s: &str| key_of(&Keystroke::parse(s).unwrap());
        assert_eq!(k("j"), Some(Key::Char('j')));
        assert_eq!(k("shift-g"), Some(Key::Char('G')));
        assert_eq!(k("ctrl-d"), Some(Key::Ctrl('d')));
        assert_eq!(k("alt-;"), Some(Key::Alt(';')));
        assert_eq!(k("escape"), Some(Key::Escape));
        assert_eq!(k("shift-tab"), Some(Key::BackTab));
        assert_eq!(k("space"), Some(Key::Char(' ')));
        assert_eq!(k("cmd-s"), None);
    }
}
