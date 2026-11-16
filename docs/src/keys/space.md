# Space

Ghostwrite's own keys are under `space`. Most of them work in any view but the terminal, whose keys are the shell's. The editor's views and pickers are on the app's ⌘ keys (see [Window](window.md)); the views plugins declare have `space` keys (`v` Typst's pages, `g` Lean's goals). The same key again, in that view, goes back to the code.

| Keys | What they do | Commands |
| --- | --- | --- |
| `space a` | ask about the line or selection | `ask` |
| a plugin's view's key | that view of this file (`space v` Typst's pages, `space g` Lean's goals), opened if it isn't shown yet; again: back to the code | `focus_view ‹name›` |
| `space k` `space d` | docs here; this file's diagnostics | `hover`, `file_diagnostics` |
| `space r` `space .` | rename; the server's code actions here | `rename_symbol`, `code_action` |
| `space s` | pick one of the file's symbols | `symbol_picker` |
| `space j` | next error (or warning); a file's pages follow | `goto_next_problem` |
| `space y` `space p` `space P` | clipboard yank, paste after, before | `yank_to_clipboard`, `paste_clipboard_after`, `paste_clipboard_before` |
| `space w q` `space w o` | only this pane | `wonly` |


See [Views and panes](views.md).

The names in the last column are what you use to [remap a key](../configuration/keys.md).
