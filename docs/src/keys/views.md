# Views and panes

`space` and a letter is the one way to move between views. Each key goes to its view, and pressed in that view, goes back to the code. The editor's are below; a plugin's views have the keys they declare (Typst's pages on `space v`, Lean's goals on `space g`), and `space ?` lists them here too. See [Modes and views](../editing/modes.md#views).

| Keys | What they do | Commands |
| --- | --- | --- |
| `space e` | Files, the file tree | `file_explorer` |
| `space t` | the terminal, typing in it | `terminal` |
| `space x` | the debugger, while something's being debugged | `debugger` |
| a plugin's view's key | that view of this file, opened if it isn't shown yet (`space v` Typst's pages, `space g` Lean's goals) | `focus_view ‹name›` |
| click | the keys go to the view clicked | |
| mode line | the mode, and the view: `NOR · terminal`, a plugin's by its title: `NOR · goals` | |

`esc` in Files, the debugger, what's beside a file, or the terminal's normal mode also goes back to the code.

A plugin's view applies to its own languages' files: on any other, its key says whose it is. To give a view a key of your own, bind `focus_view` and its name: `"space p": "focus_view pages"`. If its own key is the editor's, or a view of a plugin installed before has it, it has none until you do: that's said when the plugin is installed, when Ghostwrite starts, and in `:plugins`.

## Panes

Splits are under `space w`, as in helix's window mode. They're for the code: in Files, the debugger or beside a file they do nothing.

| Keys | What they do | Commands |
| --- | --- | --- |
| `space w v` | two panes: this file's pages, or the next tab | `vsplit` |
| `space w w` | the other pane | `rotate_view` |
| `space w q` `space w o` | only this one | `wonly` |

The names in the last column are what you use to [remap a key](../configuration/keys.md).
