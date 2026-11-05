# Moving and jumping

## Motions

| Keys | Where to |
| --- | --- |
| `h` `j` `k` `l` (or the arrows) | left, down, up, right |
| `w` `b` `e` | the next word, back a word, the word's end |
| `W` `B` `E` | the same by WORD: anything between spaces |
| `f` `t` ‹c› | on to, or just before, the next char typed; over lines too |
| `F` `T` ‹c› | back to it, or just after it |
| `alt-.` | the last `f`, `t`, `mi`, `ma`, `]p` (and the like) again |
| `gg`, `ge`, `10G` | the file's start, its last line, line 10 |
| `gh` `gl` `gs` | the line's start, end, first non-blank |
| `gt` `gc` `gb` | the top, middle, bottom line on screen |
| `]p` `[p` | the next, previous paragraph |
| `g.` | where the last change was |
| `ctrl-d` `ctrl-u`, `ctrl-f` `ctrl-b` | half a page, a page, down and up |

For `f` and `t`, Enter stands for a line's end and Tab for a tab.

`zz`, `zt` and `zb` scroll so the cursor's line is in the middle, at the top, or at the bottom.

When the keys move the view more than a couple of lines, it glides there and the cursor slides after it. `"animations": false` in the [config](../configuration/README.md) turns that off.

## By the syntax tree

With a language that has a grammar, you can move by what the code is:

| Keys | Where to |
| --- | --- |
| `]f` `[f` | the next, previous function |
| `]t` `]a` `]c` `]T` `]e` `]x` | the same for a type, argument, comment, test, entry, element |
| `alt-e` `alt-b` | the end, start of the node you're in |
| `mm` | the matching bracket |

## Going to definitions and files

- `gd` goes to the definition of what's under the cursor, as the language server says.
- `gr` goes through the references to it.
- `gf` opens the file named under the cursor, at its line and column (`src/main.rs:12:5`).
- `space j` goes to the next error, round to the first; with no errors, the next warning.
- `]d` and `[d` go to the next and previous diagnostic.

## The jump list

A jump is any move to another file, or further than a screen in the same one: `gg`, `G`, `gd`, a search, `space j`. `ctrl-o` goes back to where a jump came from, and `ctrl-i` forward again. A count goes several at once: `3ctrl-o` goes three back.

## The mouse

A click moves the cursor there. Shift-click, or a click after `v`, selects to there. Dragging selects; a double click selects a word, a triple click a line. The wheel and trackpad scroll as usual.
