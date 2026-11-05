# Searching

## In the file

`/` searches forward and `?` back. The search runs as you type, and every match on screen is marked. Enter keeps the match; `esc` goes back to where you were.

A search is a regular expression, and ignores case unless it has a capital letter.

| Keys | What they do |
| --- | --- |
| `n`, `N` | the next match, the one before, whichever way the search went (`3n`: three on) |
| `*` | search for the selection, as a whole word |
| `alt-*` | search for the selection, anywhere |

The search goes round the end of the file. In select mode, `n` and `N` add the match as another selection.

## In the project

`space /` asks for a pattern and searches the whole project. `:grep pattern` does the same (`:g` and `:search` too).

The matches come up one at a time, like a [story](../stories/README.md): each is a note at its line. Tab goes on to the next. Enter asks the model about the match you're on, and the model takes over from there.

The project's files are all searched, except the folders tools make (`target`, `node_modules` and the like).

## Replacing across the project

```text
:replace old new
```

`:replace` finds `old` (a regular expression) across the project and offers each line it would change as a step. Tab accepts a step and moves to the next; `esc esc` stops. `:replace! old new` makes every change at once.

## Symbols

`space s` picks one of the file's symbols, as its language server lists them.
