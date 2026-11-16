# Views and panes

The app's ⌘ keys go to the editor's views, from any view. A plugin's views have the keys they declare (Typst's pages on `space v`, Lean's goals on `space g`): each goes to its view, and pressed in that view, goes back to the code. ⌘/ lists them here too. See [Modes and views](../editing/modes.md#views).

| Keys | What they do | Commands |
| --- | --- | --- |
| ⌘B | Files, the file tree; from there, hide it | |
| ⌘J | the terminal, typing in it; from there, hide it | |
| ⌘⇧D | the debugger, while something's being debugged; from there, back to the code | |
| a plugin's view's key | that view of this file, opened if it isn't shown yet (`space v` Typst's pages, `space g` Lean's goals) | `focus_view ‹name›` |
| click | the keys go to the view clicked | |
| mode line | the mode, and the view: `INS · terminal`, a plugin's by its title: `NOR · goals` | |

`esc` in Files, the debugger or what's beside a file also goes back to the code. In the terminal, Esc is the shell's: ⌘J or a click on the code leaves it.

The ⌘ keys are the app's, so they can't be remapped. To go to a view with a key of your own, bind `file_explorer`, `terminal` or `debugger`.

A plugin's view applies to its own languages' files: on any other, its key says whose it is. To give a view a key of your own, bind `focus_view` and its name: `"space p": "focus_view pages"`. If its own key is the editor's, or a view of a plugin installed before has it, it has none until you do: that's said when the plugin is installed, when Ghostwrite starts, and in `:plugins`.

## Panes

⌘\\ splits the pane and ⌘K moves the keys to the other one. `space w`, as in helix's window mode, has the rest. They're for the code: in Files, the debugger or beside a file they do nothing.

| Keys | What they do | Commands |
| --- | --- | --- |
| ⌘\\ | two panes: this file's pages, or the next tab | `vsplit` |
| ⌘K | the keys to the other pane | `rotate_view` |
| `space w q` `space w o` | only this one | `wonly` |

The names in the last column are what you use to [remap a key](../configuration/keys.md). On a ⌘ key's row, the name is the command that does the same, for a key of your own.
