# Edit

These keys change text. Most act on the selection. The last three rows work while you type, in insert mode.

| Keys | What they do | Commands |
| --- | --- | --- |
| `i` `a` `I` `A` | insert before, after, line start, end | `insert_mode`, `append_mode`, `insert_at_line_start`, `insert_at_line_end` |
| `o` `O` | open line below, above (3o: three, a cursor each) | `open_below`, `open_above` |
| `d` `c` `alt-d` `alt-c` | delete, change selection (alt: no yank) | `delete_selection`, `change_selection`, `delete_selection_noyank`, `change_selection_noyank` |
| `y` `p` `P` `R` | yank, paste after, before; replace with yanked | `yank`, `paste_after`, `paste_before`, `replace_with_yanked` |
| `"` ‹r› | use register r for the next y d c p (+: clipboard) | `select_register` |
| `r` ‹c› | replace with char | `replace` |
| `~` `` ` `` `` alt-` `` | switch case, lower, upper | `switch_case`, `switch_to_lowercase`, `switch_to_uppercase` |
| `>` `<` `J` `alt-J` | indent, unindent; join lines (alt: select the spaces) | `indent`, `unindent`, `join_selections`, `join_selections_space` |
| `] space` `[ space` | blank line below, above | `add_newline_below`, `add_newline_above` |
| `ctrl-a` `ctrl-x` `ctrl-c` | number up, down; comment out / in | `increment`, `decrement`, `toggle_comments` |
| `.` `Q` `q` | repeat the last change; record, play a macro | `repeat_last_change`, `record_macro`, `replay_macro` |
| `u` `U` | undo, redo | `undo`, `redo` |
| `ctrl-w` `alt-backspace` `alt-d` | (typing) delete the word before, after | |
| `ctrl-u` `ctrl-k` | (typing) delete to the line's start, end | |
| `ctrl-r ‹r›` | (typing) type out register r | |

The names in the last column are what you use to [remap a key](../configuration/keys.md).
