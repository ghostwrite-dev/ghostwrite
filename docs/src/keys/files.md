# Files

The keys of Files, the file tree, over the code's. `space e` goes there, and again back to the code. See [Modes and views](../editing/modes.md#files).

| Keys | What they do | Commands |
| --- | --- | --- |
| `j` `down` `k` `up` | down, up a row (3j: three) | `files_down`, `files_up` |
| `gg` `home` `ge` `G` `end` | first, last row | `files_first`, `files_last` |
| `l` `right` `h` `left` | open a folder, preview a file; close, up | `expand_folder`, `collapse_folder` |
| `enter` | open the file, in the code (a folder: open, close) | `open_file` |
| `a` `r` `d` | add (name/: a folder), rename, trash | `add_file`, `rename_file`, `delete_file` |
| `esc` | back to the code (space e too) | `focus_code` |

Files has no text, so the code's motions do nothing here. `:` and most `space` keys work as anywhere.

The names in the last column are what you use to [remap a key](../configuration/keys.md), here under `"files"`.
