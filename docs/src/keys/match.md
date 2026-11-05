# Match (m)

`m` leads on to matching brackets, surrounding pairs and text objects. `]` and `[` move by syntax: functions, types, arguments and the rest come from each language's `textobjects.scm`. The `alt` keys move through the syntax tree.

For `mi` and `ma`, the object ‹o› is one of:

| Object | What it selects |
| --- | --- |
| `w` `W` | a word, a WORD (between spaces) |
| `p` | a paragraph |
| `(` `[` `{` `<` `'` `"` `` ` `` | inside or around that pair |
| `m` | the nearest pair around the cursor |
| `f` `t` `a` `c` `T` `e` `x` | a function, type, argument, comment, test, entry, element (from the language's queries) |

| Keys | What they do | Commands |
| --- | --- | --- |
| `mm` | matching bracket | `match_brackets` |
| `ms` ‹c› | surround with a pair | `surround_add` |
| `mr` ‹a›‹b› | replace surrounding pair | `surround_replace` |
| `md` ‹c› | delete surrounding pair | `surround_delete` |
| `mi` `ma` ‹o› | select inside / around: w W p ( [ { &lt; ' " ` m, and f t a c T e x | `select_textobject_inner`, `select_textobject_around` |
| `]f` `[f` `]t` `[t` `]a` `[a` `]c` `[c` `]T` `[T` `]e` `[e` `]x` `[x` | next, previous function (t type, a argument, c comment, T test, e entry, x element) | `goto_next_function`, `goto_prev_function`, `goto_next_class`, `goto_prev_class`, `goto_next_parameter`, `goto_prev_parameter`, `goto_next_comment`, `goto_prev_comment`, `goto_next_test`, `goto_prev_test`, `goto_next_entry`, `goto_prev_entry`, `goto_next_xml_element`, `goto_prev_xml_element` |
| `alt-o` `alt-i` | out to the syntax node around, back in | `expand_selection`, `shrink_selection` |
| `alt-n` `alt-p` `alt-a` `alt-I` | next, previous node; a cursor on each beside, inside | `select_next_sibling`, `select_prev_sibling`, `select_all_siblings`, `select_all_children` |
| `alt-e` `alt-b` | to the end, start of the node | `move_parent_node_end`, `move_parent_node_start` |

The names in the last column are what you use to [remap a key](../configuration/keys.md).
