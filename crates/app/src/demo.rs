//! The demo the README's GIF is recorded from (`scripts/record-demo.sh`): a
//! scripted model, and keys played into the window as if typed.
//!
//! `GHOSTWRITE_DEMO` names a folder with `replies/` (the model's replies, in
//! the order of their names) and `keys`, a line per step:
//!
//! ```text
//! wait 800              wait this many milliseconds
//! keys space a          press these keys, one after another
//! type make it pass     type this text
//! quit                  quit
//! ```

use std::path::PathBuf;
use std::time::Duration;

use ghostwrite_ai::Backend;
use gpui::{AnyWindowHandle, App, AsyncApp, Keystroke};

/// Between keys pressed one after another, and between letters typed.
const PRESS: Duration = Duration::from_millis(380);
const LETTER: Duration = Duration::from_millis(55);

fn folder() -> Option<PathBuf> {
    std::env::var_os("GHOSTWRITE_DEMO").map(PathBuf::from)
}

/// The model the demo asks, if there's a demo.
pub fn backend() -> Option<Backend> {
    let mut replies: Vec<_> = std::fs::read_dir(folder()?.join("replies")).ok()?.filter_map(|entry| Some(entry.ok()?.path())).collect();
    replies.sort();
    Some(Backend::Demo(replies.iter().filter_map(|path| std::fs::read_to_string(path).ok()).collect()))
}

enum Step {
    Wait(Duration),
    Keys(Vec<Keystroke>),
    Type(String),
    Quit,
}

fn steps() -> Option<Vec<Step>> {
    let script = std::fs::read_to_string(folder()?.join("keys")).ok()?;
    let step = |line: &str| {
        let (word, rest) = line.split_once(' ').unwrap_or((line, ""));
        Some(match word {
            "wait" => Step::Wait(Duration::from_millis(rest.trim().parse().ok()?)),
            "keys" => Step::Keys(rest.split_whitespace().map(Keystroke::parse).collect::<Result<_, _>>().ok()?),
            "type" => Step::Type(rest.to_string()),
            "quit" => Step::Quit,
            _ => return None,
        })
    };
    let lines = script.lines().map(str::trim).filter(|line| !line.is_empty() && !line.starts_with('#'));
    lines.map(|line| step(line).or_else(|| panic!("the demo can't do `{line}`"))).collect()
}

/// Play the keys into `window`, and say where it is for the recording to crop to.
pub fn play(window: AnyWindowHandle, cx: &mut App) {
    let Some(steps) = steps() else { return };
    window
        .update(cx, |_, window, _| {
            let bounds = window.bounds();
            eprintln!(
                "demo window {} {} {} {}",
                f32::from(bounds.origin.x),
                f32::from(bounds.origin.y),
                f32::from(bounds.size.width),
                f32::from(bounds.size.height)
            );
        })
        .ok();
    cx.spawn(async move |cx: &mut AsyncApp| {
        let press = |keystroke: Keystroke, cx: &mut AsyncApp| {
            cx.update(|cx| window.update(cx, |_, window, cx| window.dispatch_keystroke(keystroke, cx))).ok();
        };
        for step in steps {
            match step {
                Step::Wait(time) => cx.background_executor().timer(time).await,
                Step::Keys(keys) => {
                    for keystroke in keys {
                        press(keystroke, cx);
                        cx.background_executor().timer(PRESS).await;
                    }
                }
                Step::Type(text) => {
                    // As gpui's tests type: each letter a keystroke of its own.
                    for letter in text.chars() {
                        if let Ok(keystroke) = Keystroke::parse(&letter.to_string()) {
                            press(keystroke, cx);
                        }
                        cx.background_executor().timer(LETTER).await;
                    }
                }
                Step::Quit => {
                    cx.update(|cx| cx.quit()).ok();
                }
            }
        }
    })
    .detach();
}
