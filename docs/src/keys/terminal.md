# Terminal

⌘J goes to the terminal, typing in it, and from there hides it. See [The terminal](../terminal.md).

## Typing in it

The terminal has no editor modes.

| Keys | What they do |
| --- | --- |
| any key but ⌘ | goes to your shell, Esc too |
| ⌘J, a click on the code | back to the code |
| drag, ⌘C | select, copy |
| ⌘-click | open a web address or file it shows |

## Reading it during a story

While a story's command waits on the terminal for review, the keys read what's on it in AI mode, and the code's keys move through it. Its own layer adds:

| Keys | What they do | Commands |
| --- | --- | --- |
| `enter` | open the file a line names, at its line (gf too) | `goto_file` |
| `esc` | back to the code (from select mode, normal mode) | `focus_code` |

The motions, `/` `n` and `y` move, select, search and yank as in a file. Tab runs the command.

## Tabs

The terminal has tabs, a shell in each. With the keys on the terminal, the app's tab keys are about them:

| Keys | What they do |
| --- | --- |
| ⌘T, ⌘W | another shell in a tab; close it |
| ⌘⇧[, ⌘⇧] | the previous, next tab |
| ⌘⌥←, ⌘⌥→ | move the tab, or drag it |
| ⌘J | hide the terminal |

The names in the layer's table are what you use to [remap a key](../configuration/keys.md), here under `"terminal"`.
