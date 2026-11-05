# Selecting and text objects

## Selections

There's always a selection: in normal mode it may be one char wide. Motions replace it with what they moved over; in select mode (`v`) they extend it.

| Keys | What they do |
| --- | --- |
| `x` | select the line; again, the next one too |
| `X`, `alt-x` | out to whole lines, in to whole lines |
| `%` | everything |
| `;` | collapse the selection to its cursor |
| `alt-;` | the cursor to the selection's other end |
| `_` | trim spaces from the selection's ends |

## Text objects

`mi` selects inside an object and `ma` around it. Type the object after them:

| Object | What it is |
| --- | --- |
| `w` `W` | a word, a WORD |
| `p` | a paragraph |
| `(` `[` `{` `<` `'` `"` `` ` `` | a pair of brackets or quotes |
| `m` | the nearest pair around the cursor, whatever it is |
| `f` | a function |
| `t` | a type |
| `a` | an argument |
| `c` | a comment |
| `T` | a test |
| `e` | an entry (in a list or a table) |
| `x` | an element (markup) |

The objects from `f` on come from each language's `textobjects.scm`, which is helix's, so they cover what helix covers.

## Surrounding pairs

| Keys | What they do |
| --- | --- |
| `ms` ‹c› | surround the selection with the pair for ‹c› |
| `mr` ‹a›‹b› | replace the pair ‹a› around the cursor with ‹b› |
| `md` ‹c› | delete the pair around the cursor |

## By the syntax tree

| Keys | What they do |
| --- | --- |
| `alt-o` | out to the syntax node around the selection |
| `alt-i` | back in |
| `alt-n` `alt-p` | the next, previous node beside it |
| `alt-a` | a cursor on each node beside it |
| `alt-I` | a cursor on each node inside it |

`alt-o` pressed a few times is often the fastest way to select an expression, then a statement, then a block.
