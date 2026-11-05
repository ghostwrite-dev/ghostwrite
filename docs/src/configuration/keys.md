# Your own keys

`"keys"` in `config.json` binds keys in normal and select mode (and AI mode, for looking around). Yours go over the default keymap, and over any a plugin binds.

```json
{
  "keys": {
    "ctrl-s": ":w",
    "space W": ":wa",
    "Z": "extend_line_below",
    "Z Z": ":x",
    "x": "no_op"
  }
}
```

A key can be bound to:

- **a `:` command**, starting with `:`. Anything you can type on the command line works: `":w"`, `":format"`, `":model haiku-5.5"`, a plugin's command.
- **one of the editor's commands, by name.** Where helix has the same command, it has helix's name.
- **`no_op`**, which unbinds the key, and whatever it led on to.

The command names are the last column of each page of the [key reference](../keys/README.md), and all of them are [listed below](#command-names).

## Writing keys

Keys pressed one after another are written apart: `"space w v"`, `"g x"`, `"Z Z"`. A key can lead on to keys of your own: binding `"Z Z"` makes `Z` a key that waits for another.

| Write | For |
| --- | --- |
| `a`, `A`, `%`, `;` | a char (capitals as typed) |
| `space`, `minus` | the space bar, `-` |
| `ctrl-s`, `alt-x`, `alt-minus` | with ctrl, with alt |
| `esc`, `enter`, `tab`, `shift-tab`, `backspace`, `del` | those keys |
| `left`, `right`, `up`, `down`, `home`, `end`, `pageup`, `pagedown` | those keys |

## Checking

`space ?` shows your keys: on the line of the command they run, if the cheatsheet has one, or under **Your keys** at the end.

A key that can't be bound (a command name that isn't one, keys that aren't keys, or keys that lead on to others, such as `space w` or `g`: bind keys after them instead, as `space w x`) is said when Ghostwrite starts, and that key stays as it was.

## Keys for one view

Keys said flat, as above, are the code's, and so every [view's](../editing/modes.md#views) under its own layer. To bind a key in one view only, put it under that view's name: `"files"`, `"terminal"`, `"pages"` or `"aside"` (what a plugin shows beside a file).

```json
{
  "keys": {
    "ctrl-s": ":w",
    "files": { "R": "rename_file", "r": "no_op" },
    "terminal": { "Y": ":w" },
    "pages": { "J": "scroll_page_down" }
  }
}
```

A key in a view's layer comes before the code's there, and nowhere else: above, `R` renames in Files and still replaces with what was yanked in the code. In a view's layer, `no_op` hides the code's binding in that view only.

Each view's own commands (`files_down`, `scroll_down`, `zoom_in` and the rest) are at the end of the list below. `"code"` is a name too, the same as saying keys flat.

A key under a name that isn't a view, or that can't be bound, is said when Ghostwrite starts, with the view it's in: `pages: "q": no command "no_such"`.

## Command names

Every command a key can run, as `crates/core/src/modal/command.rs` has them:

| Command | What it does |
| --- | --- |
| `no_op` | nothing: a key bound to it is unbound |
| `move_char_left` | left |
| `move_char_right` | right |
| `move_line_down` | down a line |
| `move_line_up` | up a line |
| `page_cursor_half_down` | half a page down |
| `page_cursor_half_up` | half a page up |
| `page_cursor_down` | a page down |
| `page_cursor_up` | a page up |
| `move_next_word_start` | to the next word |
| `move_prev_word_start` | back to a word's start |
| `move_next_word_end` | to the word's end |
| `move_next_long_word_start` | to the next WORD (words between spaces) |
| `move_prev_long_word_start` | back to a WORD's start |
| `move_next_long_word_end` | to the WORD's end |
| `goto_line` | to the line counted (10G); without a count, nowhere |
| `goto_file_start` | to the file's start, or the line counted |
| `goto_last_line` | to the last line |
| `goto_line_start` | to the line's start |
| `goto_line_end` | to the line's end |
| `goto_first_nonwhitespace` | to the line's first non-blank |
| `goto_last_modification` | to where the last change was |
| `goto_window_top` | to the top line on screen |
| `goto_window_center` | to the middle line on screen |
| `goto_window_bottom` | to the bottom line on screen |
| `find_next_char` | select on to the next char typed (Enter: the line's end) |
| `find_till_char` | select on to just before the next char typed |
| `find_prev_char` | select back to the char typed |
| `till_prev_char` | select back to just after the char typed |
| `repeat_last_motion` | the last f t mi ma ]p (and the like) again |
| `goto_next_paragraph` | to the next paragraph |
| `goto_prev_paragraph` | to the paragraph before |
| `goto_next_function` | to the next function |
| `goto_prev_function` | to the function before |
| `goto_next_class` | to the next type |
| `goto_prev_class` | to the type before |
| `goto_next_parameter` | to the next argument |
| `goto_prev_parameter` | to the argument before |
| `goto_next_comment` | to the next comment |
| `goto_prev_comment` | to the comment before |
| `goto_next_test` | to the next test |
| `goto_prev_test` | to the test before |
| `goto_next_entry` | to the next entry |
| `goto_prev_entry` | to the entry before |
| `goto_next_xml_element` | to the next element |
| `goto_prev_xml_element` | to the element before |
| `expand_selection` | out to the syntax node around |
| `shrink_selection` | back in to the node inside |
| `select_next_sibling` | the next syntax node |
| `select_prev_sibling` | the syntax node before |
| `move_parent_node_end` | to the end of the node |
| `move_parent_node_start` | to the start of the node |
| `match_brackets` | to the matching bracket |
| `select_mode` | select mode, where motions extend (again: normal mode) |
| `normal_mode` | normal mode |
| `extend_line_below` | select the line (again: the next too) |
| `extend_to_line_bounds` | out to whole lines |
| `shrink_to_line_bounds` | in to the whole lines |
| `select_all` | everything |
| `collapse_selection` | the selection down to its cursor |
| `flip_selections` | the cursor to the selection's other end |
| `trim_selections` | the selection without the spaces at its ends |
| `split_selection_on_newline` | a selection of each line |
| `select_all_siblings` | a cursor on each syntax node beside this one |
| `select_all_children` | a cursor on each syntax node inside this one |
| `select_textobject_around` | select around the object typed |
| `select_textobject_inner` | select inside the object typed |
| `copy_selection_on_next_line` | copy the selection to the line below |
| `copy_selection_on_prev_line` | copy the selection to the line above |
| `select_regex` | a cursor on each match in the selection |
| `keep_primary_selection` | back to one cursor |
| `remove_primary_selection` | drop the main cursor |
| `rotate_selections_backward` | the main cursor: the one before |
| `rotate_selections_forward` | the main cursor: the one after |
| `merge_selections` | the selections merged into one |
| `merge_consecutive_selections` | the selections that touch merged |
| `align_selections` | the selections lined up in columns |
| `insert_mode` | type before the selection |
| `append_mode` | type after the selection |
| `insert_at_line_start` | type at the line's first non-blank |
| `insert_at_line_end` | type at the line's end |
| `open_below` | a new line below (3o: three, a cursor each) |
| `open_above` | a new line above |
| `delete_selection` | delete the selection, yanked |
| `delete_selection_noyank` | delete the selection |
| `change_selection` | type in place of the selection, yanked |
| `change_selection_noyank` | type in place of the selection |
| `yank` | yank the selection |
| `paste_after` | paste after the selection |
| `paste_before` | paste before the selection |
| `replace_with_yanked` | the selection replaced with what was yanked |
| `select_register` | the register typed, for the next yank, delete or paste |
| `replace` | each char of the selection replaced with the one typed |
| `switch_case` | switch the selection's case |
| `switch_to_lowercase` | the selection in lower case |
| `switch_to_uppercase` | the selection in upper case |
| `indent` | indent the selection's lines |
| `unindent` | unindent the selection's lines |
| `join_selections` | join the selection's lines (or this and the next) |
| `join_selections_space` | join lines, and select the spaces put between |
| `add_newline_below` | a blank line below |
| `add_newline_above` | a blank line above |
| `increment` | the number here up by the count |
| `decrement` | the number here down by the count |
| `toggle_comments` | the selection's lines commented out, or back in |
| `surround_add` | surround the selection with the pair typed |
| `surround_replace` | the pair around typed first replaced with the one typed second |
| `surround_delete` | delete the pair around typed |
| `repeat_last_change` | the last change again |
| `record_macro` | record keys (again: stop) |
| `replay_macro` | play the keys recorded |
| `undo` | undo |
| `redo` | redo |
| `search` | search the file, as it's typed |
| `rsearch` | search the file backward |
| `search_next` | the next match (select mode: add it) |
| `search_prev` | the match before (select mode: add it) |
| `search_selection_detect_word_bounds` | search for the selection, as a word |
| `search_selection` | search for the selection, anywhere |
| `command_mode` | the command line |
| `jump_backward` | back where a jump came from |
| `jump_forward` | forward again |
| `goto_next_buffer` | the next tab (4gn: four along) |
| `goto_previous_buffer` | the tab before |
| `move_buffer_next` | move the tab along (4gN: four places) |
| `move_buffer_previous` | move the tab back |
| `goto_definition` | to the definition |
| `goto_reference` | through the references |
| `goto_file` | the file named here (path:line:col) |
| `goto_next_diag` | the next diagnostic |
| `goto_prev_diag` | the diagnostic before |
| `align_view_center` | scroll: this line to the middle |
| `align_view_top` | scroll: this line to the top |
| `align_view_bottom` | scroll: this line to the bottom |
| `vsplit` | two panes: this file's pages, or the next tab |
| `rotate_view` | the keys to the other pane |
| `wonly` | only this pane |
| `focus_code` | back to the code (from select mode, normal mode first) |
| `ask` | ask the model about the line or selection, or to change it |
| `file_explorer` | the file tree |
| `terminal` | the terminal: type in it |
| `toggle_pages` | this file's pages, opened if they aren't shown yet; from there, back to the code |
| `toggle_aside` | the side view: a plugin's panel, or the debugger; from there, back to the code |
| `file_picker` | pick a file (fuzzy) |
| `buffer_picker` | pick an open file |
| `global_search` | search the project |
| `hover` | the docs here |
| `file_diagnostics` | this file's diagnostics |
| `rename_symbol` | rename what's here |
| `code_action` | the language server's code actions here |
| `symbol_picker` | pick one of the file's symbols |
| `goto_next_problem` | the next error (or warning); a file's pages follow |
| `yank_to_clipboard` | yank the selection to the clipboard |
| `paste_clipboard_after` | paste the clipboard after the selection |
| `paste_clipboard_before` | paste the clipboard before the selection |
| `cheatsheet` | every key |
| `files_down` | the row below |
| `files_up` | the row above |
| `files_first` | the first row |
| `files_last` | the last row |
| `expand_folder` | open the folder, or preview the file |
| `collapse_folder` | close the folder, or go up to the one it's in |
| `open_file` | open the file (and go to the code), or open or close the folder |
| `add_file` | a new file or folder here (name/ for a folder) |
| `rename_file` | rename the file or folder |
| `delete_file` | move the file or folder to the Trash |
| `scroll_down` | scroll down |
| `scroll_up` | scroll up |
| `scroll_page_down` | scroll down a screen |
| `scroll_page_up` | scroll up a screen |
| `zoom_in` | zoom in |
| `zoom_out` | zoom out |
| `zoom_reset` | back to fitting the view |
