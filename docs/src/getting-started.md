# Getting started

## Open a project

A project is a folder. Open one from the projects page (`o`, or ⌘⇧O), from **File › Open Recent**, or with `:project ~/code/thing`. Each project opens in one window; opening it again goes to that window. ⌘⇧N opens a new window.

With a project open:

- `space f` picks a file by typing a few of its letters (`edmod` finds `editor/mod.rs`).
- `:e path` opens a file by its path.

## Getting around

Files, the terminal, a file's pages and the side view are views, as the code is, and the modes work the same in each. The mode line says where the keys are: `NOR · files`, `INS · terminal`. `space` and a letter takes you to a view, and the same key again brings you back to the code:

| Keys | Goes to |
| --- | --- |
| `space e` | Files, the file tree |
| `space t` | the terminal, typing in it (`esc esc` to move through what it printed) |
| `space v` | this file's pages, such as Typst's preview |
| `space g` | the side view: Lean's goals, say, or the debugger |

That's the one way to move around. `esc` from Files or the terminal's normal mode also goes back to the code, and splits are under `space w` (`space w v` splits, `space w w` goes to the other pane). See [Modes and views](editing/modes.md).

Ghostwrite keeps your open files, and remembers recent projects for the next launch. Unsaved changes are copied every couple of seconds and come back after a crash.

## The five keys you need

Ghostwrite is modal. Keys move and act; you only type text in insert mode. As in helix, you select first and act second.

| Keys | What they do |
| --- | --- |
| `i`, then `esc` | type before the selection, then go back to moving |
| `h` `j` `k` `l`, `w` `b` | move by char and line, by word |
| `x`, then `d` or `c` | select the line, then delete it or change it |
| `space a` | ask the model about the line or selection |
| `tab` | accept the model's step and see the next |

`:w` writes the file and `u` undoes.

## Every key: space ?

Press `space ?` at any time to see every key, in sections: moving, selecting, editing, views, each view's own keys (Files, the terminal, pages, the side view), reviewing a story, the `:` commands and the window. `j` and `k` scroll it; any other key closes it. It lists your own keys and the keys your plugins add, too. The [key reference](keys/README.md) in these docs is the same list.

## The tutor

`:tutor` opens a scratch tab with a short lesson: moving, selecting then acting, typing, asking the model, and getting around the views. Change anything in it, and close it with `:q!` when you're done.

## Asking for the first time

Pick a backend first, if you haven't:

```text
:api-key sk-ant-...     a Claude API key, kept where your system keeps secrets
:model                  see what can be asked, and :model ‹name› to choose
```

[Claude Code](models/claude-code.md) works without either, if it's installed and logged in.

Then put the cursor on a function, press `space a`, and type a request: "explain this", or "add a timeout parameter". Press Enter to send it.

A question comes back as notes beside the code. A change comes as steps:

| Keys | What they do |
| --- | --- |
| `tab` | accept this step, and see the next |
| `enter` | say something about it: redirect, or ask why |
| `esc` `esc` | stop, keeping what you've accepted |

`:undo-story` takes back a whole story at once. [AI stories](stories/README.md) has the rest.
