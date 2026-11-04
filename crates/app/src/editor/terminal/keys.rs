//! The keys and the mouse as a program in the terminal is told them: as
//! xterm sends them, or by kitty's keyboard protocol if it asked for that.

use ghostwrite_terminal::{Button, Held, Modes};
use gpui::{Keystroke, Modifiers, MouseButton};

/// What a key sends to the program in the terminal: as xterm sends it, or by kitty's
/// keyboard protocol if the program asked for that. `None` for keys that aren't the
/// terminal's (anything with cmd).
pub(super) fn key_bytes(keystroke: &Keystroke, modes: &Modes) -> Option<Vec<u8>> {
    if keystroke.modifiers.platform || keystroke.modifiers.function {
        return None;
    }
    if modes.keyboard != 0 {
        return kitty_key(keystroke, modes.keyboard);
    }
    legacy_key(keystroke, modes.app_cursor)
}

/// What a key sends to the shell, as xterm sends it; `None` for keys that
/// aren't the shell's (anything with cmd). Option types what the layout
/// says it types (`|`, `{` …), except with the keys that move and delete.
fn legacy_key(keystroke: &Keystroke, app_cursor: bool) -> Option<Vec<u8>> {
    let m = &keystroke.modifiers;
    if m.platform || m.function {
        return None;
    }
    // xterm's modifier parameter: 1, plus shift 1, alt 2, ctrl 4.
    let modifier = 1 + m.shift as u8 + 2 * m.alt as u8 + 4 * m.control as u8;
    let cursor = |c: char| match modifier {
        1 if app_cursor => format!("\x1bO{c}"),
        1 => format!("\x1b[{c}"),
        n => format!("\x1b[1;{n}{c}"),
    };
    let tilde = |code: u8| if modifier == 1 { format!("\x1b[{code}~") } else { format!("\x1b[{code};{modifier}~") };
    let named = match keystroke.key.as_str() {
        "enter" => Some("\r".to_string()),
        "tab" if m.shift => Some("\x1b[Z".into()),
        "tab" => Some("\t".into()),
        "escape" => Some("\x1b".into()),
        "backspace" if m.alt => Some("\x1b\x7f".into()),
        "backspace" if m.control => Some("\x08".into()),
        "backspace" => Some("\x7f".into()),
        "up" => Some(cursor('A')),
        "down" => Some(cursor('B')),
        "right" => Some(cursor('C')),
        "left" => Some(cursor('D')),
        "home" => Some(cursor('H')),
        "end" => Some(cursor('F')),
        "delete" => Some(tilde(3)),
        "pageup" => Some(tilde(5)),
        "pagedown" => Some(tilde(6)),
        "insert" => Some(tilde(2)),
        "f1" | "f2" | "f3" | "f4" => {
            let c = ['P', 'Q', 'R', 'S'][keystroke.key[1..].parse::<usize>().ok()? - 1];
            Some(if modifier == 1 { format!("\x1bO{c}") } else { format!("\x1b[1;{modifier}{c}") })
        }
        f if function_key(f).is_some() => function_key(f).map(tilde),
        _ => None,
    };
    if let Some(named) = named {
        return Some(named.into_bytes());
    }
    let mut chars = keystroke.key.chars();
    let single = chars.next().filter(|_| chars.next().is_none());
    if m.control {
        let control = match single? {
            c @ 'a'..='z' => c as u8 - b'a' + 1,
            '@' | ' ' | '2' => 0,
            '[' | '3' => 0x1b,
            '\\' | '4' => 0x1c,
            ']' | '5' => 0x1d,
            '^' | '6' => 0x1e,
            '_' | '-' | '7' => 0x1f,
            '?' | '8' => 0x7f,
            _ => return None,
        };
        return Some(if m.alt { vec![0x1b, control] } else { vec![control] });
    }
    let typed = keystroke.key_char.clone().or_else(|| single.map(|c| if m.shift { c.to_ascii_uppercase() } else { c }).map(String::from))?;
    (!typed.is_empty()).then(|| typed.into_bytes())
}

/// The number a function key from F5 sends (`CSI 15 ~`).
fn function_key(name: &str) -> Option<u8> {
    Some(match name {
        "f5" => 15,
        "f6" => 17,
        "f7" => 18,
        "f8" => 19,
        "f9" => 20,
        "f10" => 21,
        "f11" => 23,
        "f12" => 24,
        _ => return None,
    })
}

/// A key by kitty's keyboard protocol, with the program's `flags` (see [`Modes::keyboard`]):
/// keys told apart from what they used to share (Esc, ctrl-i and Tab), modifiers on any
/// key, and with flag 8 every key as `CSI code ; modifiers u`. Keys that move keep the
/// forms they had; only presses are sent.
fn kitty_key(keystroke: &Keystroke, flags: u8) -> Option<Vec<u8>> {
    let m = &keystroke.modifiers;
    let every = flags & 8 != 0;
    let modifiers = 1 + m.shift as u8 + 2 * m.alt as u8 + 4 * m.control as u8;
    let with = |n: u8| if n == 1 { String::new() } else { format!(";{n}") };
    let csi_u = |code: u32, shifted: Option<char>, text: Option<&str>| {
        let shifted = shifted.filter(|_| flags & 4 != 0).map_or(String::new(), |c| format!(":{}", c as u32));
        let text = text.filter(|t| flags & 16 != 0 && !t.is_empty()).map(|t| t.chars().map(|c| (c as u32).to_string()).collect::<Vec<_>>().join(":"));
        match text {
            Some(text) => format!("\x1b[{code}{shifted};{modifiers};{text}u"),
            None => format!("\x1b[{code}{shifted}{}u", with(modifiers)),
        }
        .into_bytes()
    };
    // Keys that move, and the function keys: as they always were, but always as CSI.
    let at = |c: char| if modifiers == 1 { format!("\x1b[{c}") } else { format!("\x1b[1;{modifiers}{c}") }.into_bytes();
    let tilde = |n: u8| format!("\x1b[{n}{}~", with(modifiers)).into_bytes();
    let key = keystroke.key.as_str();
    match key {
        "up" => return Some(at('A')),
        "down" => return Some(at('B')),
        "right" => return Some(at('C')),
        "left" => return Some(at('D')),
        "home" => return Some(at('H')),
        "end" => return Some(at('F')),
        "f1" => return Some(at('P')),
        "f2" => return Some(at('Q')),
        "f3" => return Some(tilde(13)),
        "f4" => return Some(at('S')),
        "insert" => return Some(tilde(2)),
        "delete" => return Some(tilde(3)),
        "pageup" => return Some(tilde(5)),
        "pagedown" => return Some(tilde(6)),
        f if function_key(f).is_some() => return function_key(f).map(tilde),
        _ => {}
    }
    // Enter, Tab and Backspace stay as they were unless modified (so a shell left in this
    // mode by a program that crashed still runs `reset`); Esc never does.
    let legacy = match key {
        "enter" => Some((13, "\r")),
        "tab" => Some((9, "\t")),
        "backspace" => Some((127, "\x7f")),
        "escape" => Some((27, "\x1b")),
        _ => None,
    };
    if let Some((code, bytes)) = legacy {
        let plain = modifiers == 1 && code != 27;
        return Some(if plain && !every { bytes.as_bytes().to_vec() } else { csi_u(code, None, None) });
    }
    let base = if key == "space" { ' ' } else { single_char(key)? };
    let typed = keystroke.key_char.clone().unwrap_or_else(|| if m.shift { base.to_uppercase().collect() } else { base.to_string() });
    // Option typing what the layout says (`|` on option-7): that's text, as before.
    if m.alt && !m.control && keystroke.key_char.as_deref().is_some_and(|t| t != key && !t.eq_ignore_ascii_case(key)) && !every {
        return Some(typed.into_bytes());
    }
    // Text, alone or shifted, is text; with ctrl or alt, or every key asked for, it's CSI u.
    if !every && !m.control && !m.alt {
        return Some(typed.into_bytes());
    }
    let shifted = m.shift.then(|| typed.chars().next()).flatten().filter(|c| *c != base);
    let text = (!m.control).then_some(typed.as_str());
    Some(csi_u(base.to_lowercase().next().unwrap_or(base) as u32, shifted, text))
}

/// The one char a key name is, if it is one.
fn single_char(key: &str) -> Option<char> {
    let mut chars = key.chars();
    chars.next().filter(|_| chars.next().is_none())
}

/// gpui's button, as a program in the terminal is told it.
pub(super) fn button(button: MouseButton) -> Button {
    match button {
        MouseButton::Right => Button::Right,
        MouseButton::Middle => Button::Middle,
        _ => Button::Left,
    }
}

/// The keys held, as a program in the terminal is told them with the mouse.
pub(super) fn held(modifiers: &Modifiers) -> Held {
    Held { shift: modifiers.shift, alt: modifiers.alt, ctrl: modifiers.control }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_reach_the_shell_as_xterm_sends_them() {
        let k = |s: &str| legacy_key(&Keystroke::parse(s).unwrap(), false);
        assert_eq!(k("enter"), Some(b"\r".to_vec()));
        assert_eq!(k("ctrl-c"), Some(vec![3]));
        assert_eq!(k("up"), Some(b"\x1b[A".to_vec()));
        assert_eq!(legacy_key(&Keystroke::parse("up").unwrap(), true), Some(b"\x1bOA".to_vec()), "application cursor keys");
        assert_eq!(k("ctrl-left"), Some(b"\x1b[1;5D".to_vec()));
        assert_eq!(k("shift-tab"), Some(b"\x1b[Z".to_vec()));
        assert_eq!(k("alt-backspace"), Some(b"\x1b\x7f".to_vec()));
        assert_eq!(k("shift-a"), Some(b"A".to_vec()));
        assert_eq!(k("cmd-v"), None, "the app's");
        let option_7 = Keystroke { key_char: Some("|".into()), ..Keystroke::parse("alt-7").unwrap() };
        assert_eq!(legacy_key(&option_7, false), Some(b"|".to_vec()), "option types what the layout says");
        assert_eq!(k("f1"), Some(b"\x1bOP".to_vec()));
        assert_eq!(k("f5"), Some(b"\x1b[15~".to_vec()));
    }

    #[test]
    fn keys_by_kittys_protocol_when_its_asked_for() {
        let k = |s: &str, flags: u8| kitty_key(&Keystroke::parse(s).unwrap(), flags).map(|b| String::from_utf8(b).unwrap());
        // Telling keys apart (flag 1).
        assert_eq!(k("ctrl-x", 1).as_deref(), Some("\x1b[120;5u"));
        assert_eq!(k("ctrl-s", 1).as_deref(), Some("\x1b[115;5u"));
        assert_eq!(k("escape", 1).as_deref(), Some("\x1b[27u"), "Esc isn't the start of a sequence any more");
        assert_eq!(k("alt-a", 1).as_deref(), Some("\x1b[97;3u"));
        assert_eq!(k("ctrl-shift-a", 1).as_deref(), Some("\x1b[97;6u"));
        assert_eq!(k("a", 1).as_deref(), Some("a"), "text is text");
        assert_eq!(k("shift-a", 1).as_deref(), Some("A"));
        assert_eq!(k("enter", 1).as_deref(), Some("\r"), "so a shell left like this still runs `reset`");
        assert_eq!(k("shift-enter", 1).as_deref(), Some("\x1b[13;2u"));
        assert_eq!(k("shift-tab", 1).as_deref(), Some("\x1b[9;2u"));
        assert_eq!(k("ctrl-up", 1).as_deref(), Some("\x1b[1;5A"), "keys that move keep their forms");
        assert_eq!(k("up", 1).as_deref(), Some("\x1b[A"));
        // Every key (flag 8), shifted keys (4) and text (16).
        assert_eq!(k("a", 8).as_deref(), Some("\x1b[97u"));
        assert_eq!(k("enter", 8).as_deref(), Some("\x1b[13u"));
        assert_eq!(k("shift-a", 8 | 4).as_deref(), Some("\x1b[97:65;2u"));
        assert_eq!(k("a", 8 | 16).as_deref(), Some("\x1b[97;1;97u"));
    }
}
