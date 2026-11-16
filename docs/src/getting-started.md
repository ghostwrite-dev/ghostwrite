# Getting started

## Open a project

A project is a folder. Open one from the projects page (`o`, or ⌘⇧O), from **File › Open Recent**, or with `:project ~/code/thing`. Each project opens in one window; opening it again goes to that window. ⌘⇧N opens a new window.

With a project open:

- ⌘P picks a file by typing a few of its letters (`edmod` finds `editor/mod.rs`).
- `:e path` opens a file by its path.

## Getting around

Files, the terminal, the debugger and the views plugins add (a file's pages, Lean's goals) are views, as the code is. The mode line says where the keys are: `NOR · files`, `INS · terminal`. The app's ⌘ keys take you to the editor's views:

| Keys | Goes to |
| --- | --- |
| ⌘B | Files, the file tree; from there, hides it |
| ⌘J | the terminal, typing in it; from there, hides it |
| ⌘⇧D | the debugger, while something's being debugged; from there, back to the code |
| `space v`, `space g` | a plugin's views: Typst's pages, Lean's goals; again, back to the code |

`esc` from Files or the debugger also goes back to the code. In the terminal every key but ⌘ goes to the shell, so ⌘J or a click on the code leaves it. ⌘\ splits the pane and ⌘K moves the keys to the other one. See [Modes and views](editing/modes.md).

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

## Every key: ⌘/

Press ⌘/ at any time, from any view, to see every key, in sections: moving, selecting, editing, views, each view's own keys (Files, the terminal, the debugger, and each plugin's view, by its title), reviewing a story, the `:` commands and the window. `j` and `k` scroll it; any other key closes it. It lists your own keys and the keys your plugins add, too. The [key reference](keys/README.md) in these docs is the same list.

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
