# Move

These keys move the cursor in normal mode. In select mode (`v`) the same keys extend the selection instead. A count before a key repeats it or picks a line: `3w` moves three words, `10G` goes to line 10.

| Keys | What they do | Commands |
| --- | --- | --- |
| `h` `j` `k` `l` | left down up right | `move_char_left`, `move_line_down`, `move_line_up`, `move_char_right` |
| `left` `down` `up` `right` | the same | `move_char_left`, `move_line_down`, `move_line_up`, `move_char_right` |
| `w` `b` `e` | next word, back, word end | `move_next_word_start`, `move_prev_word_start`, `move_next_word_end` |
| `W` `B` `E` | the same by WORD (between spaces) | `move_next_long_word_start`, `move_prev_long_word_start`, `move_next_long_word_end` |
| `f` `t` `F` `T` ‹c› | find / till a char, over lines (⏎: line end; F T: back) | `find_next_char`, `find_till_char`, `find_prev_char`, `till_prev_char` |
| `alt-.` | find again (and mi ma ]p [p) | `repeat_last_motion` |
| `gg` `ge` `G` | file start, last line; 10G: line 10 | `goto_file_start`, `goto_last_line`, `goto_line` |
| `gh` `gl` `gs` | line start, end, first non-blank | `goto_line_start`, `goto_line_end`, `goto_first_nonwhitespace` |
| `home` `end` | line start, end | `goto_line_start`, `goto_line_end` |
| `gt` `gc` `gb` | top, middle, bottom line on screen | `goto_window_top`, `goto_window_center`, `goto_window_bottom` |
| `zz` `zt` `zb` `zc` | scroll: this line to the middle, top, bottom (zc: middle) | `align_view_center`, `align_view_top`, `align_view_bottom` |
| `]d` `[d` `]p` `[p` | next, previous diagnostic; paragraph | `goto_next_diag`, `goto_prev_diag`, `goto_next_paragraph`, `goto_prev_paragraph` |
| `g.` | where the last change was | `goto_last_modification` |
| `ctrl-d` `ctrl-u` | half page down, up | `page_cursor_half_down`, `page_cursor_half_up` |
| `ctrl-f` `ctrl-b` `pagedown` `pageup` | page down, up | `page_cursor_down`, `page_cursor_up` |
| `gn` `gp` | next, previous tab (4gn: four along) | `goto_next_buffer`, `goto_previous_buffer` |
| `gN` `gP` | move the tab along (4gN: four places) | `move_buffer_next`, `move_buffer_previous` |
| `gd` `gr` | go to definition; through the references | `goto_definition`, `goto_reference` |
| `gf` | the file named here (path:line:col) | `goto_file` |
| `ctrl-o` `ctrl-i` | back where a jump came from, forward | `jump_backward`, `jump_forward` |

The names in the last column are what you use to [remap a key](../configuration/keys.md).
