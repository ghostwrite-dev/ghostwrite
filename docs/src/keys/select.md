# Select

These keys make, change and drop selections, and add more cursors. See [Selecting](../editing/selecting.md) and [Multiple cursors](../editing/cursors.md) for how they fit together.

| Keys | What they do | Commands |
| --- | --- | --- |
| `v` | select mode (motions extend) | `select_mode` |
| `esc` | normal mode; with cursors, back to one | `normal_mode` |
| `C` `alt-C` | copy the selection to the line below, above | `copy_selection_on_next_line`, `copy_selection_on_prev_line` |
| `s` `alt-s` | a cursor on each match in the selection; each line | `select_regex`, `split_selection_on_newline` |
| `,` `alt-,` | back to one cursor; drop the main one | `keep_primary_selection`, `remove_primary_selection` |
| `(` `)` | the main cursor: the one before, after | `rotate_selections_backward`, `rotate_selections_forward` |
| `alt-minus` `alt-_` `&` | merge selections (those touching); line them up | `merge_selections`, `merge_consecutive_selections`, `align_selections` |
| `x` `X` `alt-x` `%` | select line (again: next line), out to, in to whole lines; all | `extend_line_below`, `extend_to_line_bounds`, `shrink_to_line_bounds`, `select_all` |
| `;` `alt-;` `_` | collapse, flip, trim selection | `collapse_selection`, `flip_selections`, `trim_selections` |
| `click` `⇧click` | move there; select to there (or click after `v`) | |
| `drag, double click, triple click` | select; a word; a line | |

The names in the last column are what you use to [remap a key](../configuration/keys.md).
