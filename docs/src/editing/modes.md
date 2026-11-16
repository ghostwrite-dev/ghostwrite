# Modes and views

Two things decide what a key does: the **mode** you're in, and the **view** that has the keys. The mode line shows both on the left: `NOR` in the code, `NOR · files`, `INS · terminal`, `NOR · pages`.

## Modes

The modes are the same in every view but the terminal, which has none: its keys are the shell's.

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
| Terminal | `INS · terminal` | your shell, under the code |
| Debugger | `NOR · debugger` | where a program being debugged is stopped, right of the code |
| A plugin's | by its title: `NOR · pages`, `NOR · goals` | a view a plugin declares for its files: a tab of a file's pages (Typst's), or what's beside a file (Lean's goals) |

Each view has its own layer of keys over the code's. A key is looked up in the layer of the view with the keys first, then in the code's, so `:` and the `space` keys work everywhere but the terminal. In Files, the debugger and beside the file there's no text, so only their own keys and those that work anywhere do something. On pages, the code's motions move through what's there. In the terminal every key but ⌘ goes to the shell. The [key reference](../keys/README.md) has each view's layer, and you can [remap keys in one view](../configuration/keys.md#keys-for-one-view).

### Moving between views

The app's ⌘ keys go to the editor's views, from any view. A plugin's views have the `space` keys they declare.

| Keys | What they do |
| --- | --- |
| ⌘B | go to Files; from Files, hide it |
| ⌘J | go to the terminal, typing in it; from there, hide it |
| ⌘⇧D | go to the debugger, while something's being debugged; from there, back to the code |
| a plugin's view's key | go to that view of this file, opening it if it isn't shown yet (`space v` Typst's pages, `space g` Lean's goals); again, back to the code |
| `esc` | from Files, the debugger or beside the file, back to the code |
| ⌘\\ | split the pane: this file's pages, or the next tab |
| ⌘K | the keys to the other pane |
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

- The terminal has no modes. ⌘J, `:term` or a click goes there typing, and every key but ⌘ goes to the shell, Esc included. ⌘J or a click on the code leaves it.
- While a story's command waits there for review, the keys read what's on it in AI mode, like a file that can't be edited: the motions, `/` and `n` search, `y` yanks, and `gf` or Enter opens a `path:line:col`. Tab runs the command. When a story ends with a command, the keys stay in the shell, typing.

See [The terminal](../terminal.md).

### A plugin's views

A plugin declares its views of its own languages' files, each with a name, a title and maybe a key: a file's pages in a tab (Typst's, on `space v`), or something beside the file (Lean's goals, on `space g`). The editor has no such view of its own. On pages, `j` `k` and `ctrl-d` `ctrl-u` scroll and `+` `-` `=` zoom; beside the file, `j` `k` and `ctrl-d` `ctrl-u` scroll and `esc` goes back. Your keys for a view go under its name, and `focus_view ‹name›` goes there from a key of yours. See [Pages and what's beside a file](../languages/pages.md).

### Debugger

While a program's being debugged, the debugger is beside the code, over a plugin's panel if there's one. ⌘⇧D goes there; `j` `k` scroll it, and `esc` goes back. See [Debugging](../languages/debugger.md).
