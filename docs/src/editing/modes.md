# Modes and views

Two things decide what a key does: the **mode** you're in, and the **view** that has the keys. The mode line shows both on the left: `NOR` in the code, `NOR · files`, `INS · terminal`, `NOR · pages`.

## Modes

The modes are the same in every view.

| Mode | Shown | What the keys do |
| --- | --- | --- |
| Normal | `NOR` | move, select and act |
| Insert | `INS` | type |
| Select | `SEL` | motions extend the selection instead of moving it |
| AI | `AI` | review a story: look around, decide with Tab, Enter and Esc |

### Normal mode

Where you start, and where `esc` takes you back to. Each motion selects what it moves over: `w` selects to the start of the next word, `e` to its end. An action then works on the selection.

A count before a key repeats it, or picks a place: `3w`, `10G`, `4gn`.

### Insert mode

`i` types before the selection and `a` after it. `I` and `A` type at the start and end of the line, `o` and `O` on a new line below or above. The selection stays while you type, and after `a` it takes in what's typed. `esc` goes back to normal mode.

While you type in the code:

- Brackets and quotes close themselves, as in helix. A language can add its own pairs (Typst's `$`).
- The bracket matching the one at the cursor is marked.
- `ctrl-w` (or `alt-backspace`) deletes the word before, `alt-d` the word after.
- `ctrl-u` and `ctrl-k` delete to the line's start and end.
- `ctrl-r` then a register's name types out that register.
- A language server's completions show as you type.

`3o` opens three lines with a cursor on each, indented, a line comment carried on if you were in one. `c` on whole lines types on a line of their own.

### Select mode

`v` enters select mode, and `v` again leaves it. In select mode every motion extends the selection instead of replacing it. `n` and `N` add the next search match as another selection.

### AI mode

While a [story](../stories/README.md) is up, the editor is in AI mode, in whichever view has the keys. The motions still work, so you can look around, but you can't edit until the story's done. AI mode goes with the keys from view to view.

## Views

A view is a place on screen that takes the keys:

| View | Mode line | What it is |
| --- | --- | --- |
| Code | `NOR` | a file's text, in either pane of a split |
| Files | `NOR · files` | the file tree, left of the code |
| Terminal | `NOR · terminal` | your shell, under the code |
| Pages | `NOR · pages` | a tab of a file's pages, which a plugin makes (Typst's, say) |
| Beside the file | `NOR · lean`, by its title; `NOR · debugger` while debugging | the side view, right of the code: what a plugin shows (Lean's goals, say), or the debugger |

Each view has its own layer of keys over the code's. A key is looked up in the layer of the view with the keys first, then in the code's, so `:` and the `space` keys work everywhere. In Files and beside the file there's no text, so only their own keys and those that work anywhere do something. On the terminal and on pages, the code's motions move through what's there. The [key reference](../keys/README.md) has each view's layer, and you can [remap keys in one view](../configuration/keys.md#keys-for-one-view).

### Moving between views

`space` and a letter is the one way to move between views, and the same key in that view goes back to the code.

| Keys | What they do |
| --- | --- |
| `space e` | go to Files; again, back to the code |
| `space t` | go to the terminal, typing in it; again, back to the code |
| `space v` | go to this file's pages, opening them if they aren't shown yet; again, back to the code |
| `space g` | go to the side view (a plugin's panel, or the debugger); again, back to the code |
| `esc` | from Files, the terminal (in normal mode) or beside the file, back to the code |
| ⌘B | go to Files; from Files, hide it |
| ⌘J | go to the terminal; from there, hide it |
| a click | the keys go to what you clicked |

Whatever was going on in a view ends as the keys leave it: typing and selecting in the code, typing in the shell, naming a file in Files.

### The code

As described in the rest of this chapter.

### Files

Files has the tree's keys as its own layer:

| Keys | What they do |
| --- | --- |
| `j` `k` (or ↓ ↑) | down, up a row (a count moves further) |
| `gg`, `ge` or `G` | the first, last row |
| `l`, `h` (or → ←) | open a folder, or preview a file; close it, or go up |
| `enter` | open the file, in the code; on a folder, open or close it |
| `a` | add a file, or a folder if the name ends in `/` |
| `r` | rename it: the name is edited in place, in insert mode |
| `d` | move it to the Trash |
| `esc` | back to the code |

### Terminal

- **Insert mode** types in the shell. `space t`, `:term`, ⌘J or a click goes there typing, and from normal mode `i`, `a`, `o` or `c` do.
- In insert mode, `esc` goes to the program running there (vim, fzf, a REPL). `esc esc`, or `ctrl-\ ctrl-n` as in Neovim, goes to normal mode. While a full-screen program has the terminal, every Esc is the program's, and `ctrl-\ ctrl-n` still leaves.
- **Normal mode** moves through the scrollback. Its scrollback and screen are text you move over with the editor's motions, like a file that can't be edited: `/` and `n` search, `y` yanks, `gf` or Enter opens a `path:line:col`, and `space a` asks the model about what's selected. `esc` goes back to the code.

See [The terminal](../terminal.md).

### Pages and beside the file

A plugin can show a file's pages in a tab (Typst does), or something beside the file (Lean's goals). Both are views: `space v` goes to the pages and `space g` to the side view. On pages, `j` `k` and `ctrl-d` `ctrl-u` scroll and `+` `-` `=` zoom; beside the file, `j` `k` and `ctrl-d` `ctrl-u` scroll and `esc` goes back. See [Pages and what's beside a file](../languages/pages.md).
