# Space

Ghostwrite's own keys are under `space`. Most of them work in any view. `space` and a letter is the one way to move between views: `e` Files, `t` the terminal, `v` pages, `g` the side view. The same key again, in that view, goes back to the code.

| Keys | What they do | Commands |
| --- | --- | --- |
| `space a` | ask about the line or selection | `ask` |
| `space e` | Files, the file tree (⌘B too); again: back to the code | `file_explorer` |
| `space t` | the terminal, typing in it (⌘J too); again: back to the code | `terminal` |
| `space v` | this file's pages, opened if they aren't shown yet (Typst's, say); again: back to the code | `toggle_pages` |
| `space g` | the side view: a plugin's panel (Lean's goals, say) or the debugger; again: back to the code | `toggle_aside` |
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
