# Files, tabs and panes

## Pickers

| Keys | Picks |
| --- | --- |
| `space f` | a file of the project |
| `space b` | an open file |
| `space s` | a symbol in this file |
| `space .` | one of the language server's code actions here |

Typing narrows the list fuzzily: `edmod` finds `editor/mod.rs`. ↑ and ↓ choose, Enter takes it, `esc` closes.

## The file tree

`space e` goes to Files, the file tree, and `space e` again (or `esc`) goes back to the code. ⌘B goes there too, and from there hides it.

| Keys | What they do |
| --- | --- |
| `j` `k`, `gg` `G` | move |
| `l`, `h` | open a folder (or preview a file), close it (or go up) |
| `enter` | open the file, in the code |
| `a` | add a file, or a folder if the name ends in `/` |
| `r` | rename: type the new name in place, in insert mode |
| `d` | move to the Trash |

These are Files' own keys, its layer over the code's. `:` and the `space` keys work as anywhere. See [Modes and views](modes.md#files) and the [Files keys](../keys/files.md).

When a story would create, remove or rename a file, the step is shown in the tree, in place.

## Tabs

Each open file is a tab.

| Keys | What they do |
| --- | --- |
| `gn`, `gp` | the next, previous tab (`4gn`: four along) |
| ⌘⇧], ⌘⇧[ | the same |
| `gN`, `gP` | move the tab along the tab bar (`4gN`: four places) |
| ⌘⌥→, ⌘⌥←, `:tab-right`, `:tab-left` | the same, or drag it |
| ⌘N, `:n` | a new, empty tab |
| ⌘W, `:bc` | close the tab (`:bc!` without saving) |
| ⌘O, `:e path` | open a file |

## Two panes

`space w v` (or `:vsplit`) opens a second pane beside this one. It shows this file's pages if it has them (a Typst document, say), or else the next tab.

| Keys | What they do |
| --- | --- |
| `space w w`, a click | the keys to the other pane |
| `space w q`, `space w o`, `:only` | back to one pane |

Each pane shows its own tab. Going to a tab that's in the other pane moves you there. A preview beside its file keeps up as you type, and clicking on it moves the cursor in the file without taking the keys away.

## Windows and zoom

| Keys | What they do |
| --- | --- |
| ⌘⇧O | open a folder |
| ⌘⇧N | a new window |
| ⌘=, ⌘-, ⌘0 | zoom in, out, back to 100% |
| `:project` | the projects page |
| `:q`, `:qa` | close the window, quit |

On Windows and Linux, read Ctrl+Shift for ⌘ (see [Install](../install.md#how-far-each-system-has-come)).
