# Changing text

## Editing the selection

| Keys | What they do |
| --- | --- |
| `d`, `c` | delete the selection; delete it and start typing. Both yank it first |
| `alt-d`, `alt-c` | the same without yanking |
| `y` | yank the selection |
| `p`, `P` | paste after, before the selection |
| `R` | replace the selection with what was yanked |
| `r` ‹c› | replace each char of the selection with ‹c› (Enter for a line break) |
| `~` | switch case; `` ` `` lower case; `` alt-` `` upper case |
| `>`, `<` | indent, unindent the selection's lines |
| `J` | join the selection's lines (or this and the next), taking a comment's leader off the lines joined |
| `alt-J` | join lines, and select the spaces put between |
| `] space`, `[ space` | a blank line below, above |
| `ctrl-a`, `ctrl-x` | the number here up, down (by the count: `5ctrl-a`) |
| `ctrl-c` | comment the selection's lines out, or back in |
| `u`, `U` | undo, redo |

## Registers

Yanks and deletes go to one register shared by every file. To use another, type `"` and its name before the key: `"ay` yanks into `a`, `"ap` pastes from it.

`"+` is the system clipboard: `"+y` copies, `"+p` pastes. `space y`, `space p` and `space P` do the same in fewer keys. ⌘C and ⌘V work as everywhere.

While typing, `ctrl-r` and a register's name types out what's in it.

## Repeating

`.` repeats the last change, from its first key back to normal mode: a `c` and what you typed after it, an `x` `d`, a `>`.

`Q` starts recording keys and `Q` again stops. `q` plays them back. Recording shows as **recording** in the status line.

## Formatting

`:format` formats the file with its language server. Saving formats too, unless `format_on_save` is `false` in the [config](../configuration/README.md).

## Saving

| Command | What it does |
| --- | --- |
| `:w`, ⌘S | write the file |
| `:w path` | write it to `path` |
| `:wa` | write every file |
| `:w!` | write over changes made on disk since |
| `:wq`, `:x` | write and close (`:x` only writes if there's something to) |
| `:rl` | reload the file from disk (`:rl!` discards your changes) |

## When files change on disk

The file tree and open files follow changes other programs make, as they're made: a shell, git, another editor. In a file with unsaved changes, theirs come in alongside yours when they're to other lines. Where both changed the same lines, yours are kept, the status line says the file changed on disk, and saving won't write over theirs until `:w!`.

Unsaved changes are also copied every couple of seconds, and come back, still unsaved, if Ghostwrite crashes.
