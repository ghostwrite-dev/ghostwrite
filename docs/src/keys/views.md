# Views and panes

`space` and a letter is the one way to move between views. Each key goes to its view, and pressed in that view, goes back to the code. See [Modes and views](../editing/modes.md#views).

| Keys | What they do | Commands |
| --- | --- | --- |
| `space e` | Files, the file tree | `file_explorer` |
| `space t` | the terminal, typing in it | `terminal` |
| `space v` | this file's pages, opened if they aren't shown yet | `toggle_pages` |
| `space g` | the side view: a plugin's panel, or the debugger | `toggle_aside` |
| click | the keys go to the view clicked | |
| mode line | the mode, and the view: `NOR · terminal` | |

`esc` in Files, the side view, or the terminal's normal mode also goes back to the code.

## Panes

Splits are under `space w`, as in helix's window mode. They're for the code: in Files or the side view they do nothing.

| Keys | What they do | Commands |
| --- | --- | --- |
| `space w v` | two panes: this file's pages, or the next tab | `vsplit` |
| `space w w` | the other pane | `rotate_view` |
| `space w q` `space w o` | only this one | `wonly` |

The names in the last column are what you use to [remap a key](../configuration/keys.md).
