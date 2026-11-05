# Multiple cursors

Every selection has a cursor, and you can have many. Keys and typing happen at all of them, and the whole thing is one undo.

| Keys | What they do |
| --- | --- |
| `C`, `alt-C` | copy each selection to the line below, above, as long as it is (`3C`: three times) |
| `s` | ask for a regex, and put a cursor on each match in the selection (or the file) |
| `alt-s` | a cursor on each line of the selection |
| `,` | back to one cursor (`esc` in normal mode does this too) |
| `alt-,` | drop the main cursor |
| `(` `)` | make the one before or after the main one |
| `alt-minus` | merge all the selections into one |
| `alt-_` | merge the selections that touch |
| `&` | line the selections up in columns |
| `alt-a`, `alt-I` | a cursor on each syntax node beside this one, inside it |
| `3o` | open three lines, a cursor on each |

In select mode, `n` adds the next search match as another selection.

What a key does once, such as `:` commands, `u`, or a jump to another file, happens at the main cursor only. The view follows the main cursor.

A typical use: select a function with `maf`, press `s`, type a name and Enter. Now there's a cursor on every use of that name in the function, and `c` renames them all.
