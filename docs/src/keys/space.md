# Space

Ghostwrite's own keys are under `space`. Most of them work in any view. `space` and a letter is the one way to move between views: `e` Files, `t` the terminal, `x` the debugger, and the keys plugins' views declare (`v` Typst's pages, `g` Lean's goals). The same key again, in that view, goes back to the code.

| Keys | What they do | Commands |
| --- | --- | --- |
| `space a` | ask about the line or selection | `ask` |
| `space e` | Files, the file tree (⌘B too); again: back to the code | `file_explorer` |
| `space t` | the terminal, typing in it (⌘J too); again: back to the code | `terminal` |
| `space x` | the debugger, while something's being debugged; again: back to the code | `debugger` |
| a plugin's view's key | that view of this file (`space v` Typst's pages, `space g` Lean's goals), opened if it isn't shown yet; again: back to the code | `focus_view ‹name›` |
| `space f` `space b` | pick a file, an open file (fuzzy) | `file_picker`, `buffer_picker` |
| `space /` | search the project | `global_search` |
| `space k` `space d` | docs here; this file's diagnostics | `hover`, `file_diagnostics` |
| `space r` `space .` | rename; the server's code actions here | `rename_symbol`, `code_action` |
| `space s` | pick one of the file's symbols | `symbol_picker` |
| `space j` | next error (or warning); a file's pages follow | `goto_next_problem` |
| `space y` `space p` `space P` | clipboard yank, paste after, before | `yank_to_clipboard`, `paste_clipboard_after`, `paste_clipboard_before` |
| `space w v` `space w w` `space w q` `space w o` | panes: split, the other one, only this one | `vsplit`, `rotate_view`, `wonly` |
| `space ?` | this, every key | `cheatsheet` |


See [Views and panes](views.md).

The names in the last column are what you use to [remap a key](../configuration/keys.md).
