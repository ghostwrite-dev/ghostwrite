# The terminal

The terminal is a [view](editing/modes.md#views) under the code with your own shell in it: `$SHELL` as a login shell, or PowerShell on Windows. Its screen and scrollback (10,000 lines) are emulated by alacritty's terminal emulator, and it speaks kitty's keyboard protocol to programs that ask for it.

## Showing it and going there

| Keys | What they do |
| --- | --- |
| `space t` | go to the terminal, typing in it; from there, back to the code |
| ⌘J | go to the terminal, typing in it; from there, hide it |
| `:term` | go to the terminal, typing in it |
| `esc` | in normal mode, back to the code |
| a click | the keys go to the terminal |

The shell keeps running while the terminal is hidden. When a shell exits its tab closes, and with the last one, the terminal. Drag its top edge to resize it; the height is kept for next time.

## Modes in the terminal

The [modes](editing/modes.md) mean the same on the terminal as anywhere, applied to the shell:

- **Insert mode** types into the shell. Press `i` or `a` to start typing on the shell's line.
- **Normal mode** reads the terminal with the editor's keys.
- In insert mode, `esc` goes to the program running there, so vim, fzf and the rest get their Esc. `esc esc`, or `ctrl-\ ctrl-n` as in Neovim, goes back to normal mode. A single Esc reaches the program a moment later; two in a row never do. While a full-screen program like vim has the terminal, every Esc is the program's, and `ctrl-\ ctrl-n` still leaves.

The status line shows `INS · terminal` or `NOR · terminal`.

## Reading it in normal mode

In normal mode the terminal's scrollback and screen are text, like a file that can't be edited:

- the motions, select mode, `x`, `/` and `n`, and `y` all work as in a file, and the selection shows on the terminal;
- what's printed meanwhile comes in under it, and the selection stays on its text;
- Enter or `gf` opens the file a line names, at its line and column (`src/main.rs:12:5`);
- `space a` asks the model about what's selected, from an empty tab of its own, since the code in front isn't what it's about;
- `i`, `a`, `o` or `c` go back to typing on the shell's line, and `esc` goes back to the code.

In insert mode, dragging the mouse selects (a double click a word, a triple click a line) and ⌘C copies.

## Links

Hold ⌘ over a web address or a file the terminal printed (`src/main.rs:12:5`), and it's underlined. ⌘-click opens the address in your browser, or the file in the editor at that line.

## Tabs

The terminal has tabs of its own, a shell in each. With the keys on the terminal, the app's tab keys are about them:

| Keys | What they do |
| --- | --- |
| ⌘T | another shell in a new tab |
| ⌘W | close it |
| ⌘⇧[, ⌘⇧] | the previous, next tab |
| ⌘⌥←, ⌘⌥→, or drag | move the tab |

The tabs show once there are two.

## The environment

The shells start with:

| Variable | Value | Why |
| --- | --- | --- |
| `TERM` | `xterm-256color` | |
| `COLORTERM` | `truecolor` | |
| `PAGER`, `GIT_PAGER` | `cat` | `git log` prints all of it into the scrollback, to read and for the model, rather than waiting in `less` |

A program that asks the terminal's colors is told the theme's.

## Commands from a story

The model reads the terminal, and types into it only commands you accept. A command it proposes waits at the prompt, grey, with its reason above and its keys below. Tab runs it, and the story goes on once it's done, with what it printed. See [Commands in the terminal](stories/commands.md).
