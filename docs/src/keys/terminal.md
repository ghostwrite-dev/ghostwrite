# Terminal

The keys of the terminal, over the code's. `space t` goes there, typing in it, and again back to the code. See [The terminal](../terminal.md).

## Normal mode: reading it

In normal mode the terminal's scrollback and screen are text, and the code's keys move through it. Its own layer adds:

| Keys | What they do | Commands |
| --- | --- | --- |
| `enter` | open the file a line names, at its line (gf too) | `goto_file` |
| `esc` | back to the code (from select mode, normal mode) | `focus_code` |

And from the cheatsheet:

| Keys | What they do |
| --- | --- |
| `i` `a` `o` `c` | insert mode: type in its shell; the model runs only what you accept |
| motions, `/` `n` `y` | move, select, search, yank, as in a file |
| `space a` | ask about what's selected |
| ⌘-click | open a web address or file it shows |

## Insert mode: typing in it

| Keys | What they do |
| --- | --- |
| any key | goes to your shell |
| `esc` | goes to the program running there (vim, fzf…), a moment later |
| `esc` `esc`, `ctrl-\ ctrl-n` | back to normal mode, to move through it |
| drag, ⌘C | select, copy |

While a full-screen program like vim has the terminal, every Esc is the program's. `ctrl-\ ctrl-n` still leaves.

## Tabs

The terminal has tabs, a shell in each. With the keys on the terminal, the app's tab keys are about them:

| Keys | What they do |
| --- | --- |
| ⌘T, ⌘W | another shell in a tab; close it |
| ⌘⇧[, ⌘⇧] | the previous, next tab |
| ⌘⌥←, ⌘⌥→ | move the tab, or drag it |
| ⌘J | hide the terminal |

The names in the first table are what you use to [remap a key](../configuration/keys.md), here under `"terminal"`.
