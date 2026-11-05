# Key reference

To move around, `space` and a letter goes to a view: `space e` Files, `space t` the terminal, `space x` the debugger, and a plugin's views by the keys they declare (`space v` Typst's pages, `space g` Lean's goals). The same key again goes back to the code. See [Views and panes](views.md).

This is every key, in the same sections and order as the `space ?` cheatsheet. The tables come from the keymap itself (`crates/core/src/modal/keymap.rs`), and the names in their last column from `command.rs` beside it.

The first seven sections are the code's keys. They work in every [view](../editing/modes.md#views), under that view's own layer. Files, Terminal, Pages and Beside the file are those layers. Reviewing is the keys of a story, Command line the `:` key and its commands, and Window the app's ⌘ keys.

- [Move](move.md)
- [Select](select.md)
- [Edit](edit.md)
- [Match (m)](match.md)
- [Search](search.md)
- [Space](space.md)
- [Views and panes](views.md)
- [Files](files.md)
- [Terminal](terminal.md)
- [Pages](pages.md)
- [Beside the file](aside.md)
- [Reviewing](reviewing.md)
- [Command line (:)](command-line.md)
- [Window](window.md)

## How keys are written

Keys pressed one after another are written apart: `space f`, `space w v`. Where each is a single char they're run together, as helix writes them: `gg` is `g` then `g`, `]d` is `]` then `d`.

‹c› is a char you type next, ‹r› a register's name, ‹o› a text object, and ‹a›‹b› two chars.

On Windows and Linux, ⌘ is Ctrl+Shift, ⌘⇧ is Ctrl+Alt+Shift and ⌘⌥ is Ctrl+Alt.

## How a view's layer works

A key is looked up in the layer of the view that has the keys first, then in the code's. So in Files `j` is a row down, but `space f` and `:` still work. In a view with no text to move through (Files, and what's beside a file), only its own keys and those that work anywhere do something: `:` and most `space` keys. On the terminal and on pages, the code's motions move through what's there.

## What's not here

- Keys you've bound yourself show in the cheatsheet under **Your keys**, or on the line of the command they run. See [Your own keys](../configuration/keys.md).
- Plugins can add keys for their files, or for one of their views. They show under **Plugin keys** in `space ?` while a file of theirs is in front. A plugin's views' keys show in **Views and panes**, and each view has a section of its own, by its title and key.
