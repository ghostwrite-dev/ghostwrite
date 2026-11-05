# Language servers

A language plugin names the servers that can serve it. Ghostwrite starts the first one that's installed, once per language per project, on the first file that needs it, and again if it stops. It runs in the folder nearest the file with one of the server's root files in it (`Cargo.toml`, `lakefile.lean`), or else the project's.

Servers are found on your login shell's `PATH`, even when Ghostwrite is opened from the Dock, and start with the rest of that shell's environment too. The status line shows the server for the file in front.

A server is a program that runs as you do, with your permissions. That's why installing a plugin says which servers it starts.

## What they do

| Keys | What they do |
| --- | --- |
| `gd` | go to the definition |
| `gr` | go through the references, one at a time |
| `space k` | the docs for what's under the cursor, as a note under the line |
| `space r` | rename it everywhere |
| `space .` | the server's code actions here |
| `space s` | pick one of the file's symbols |
| `space d` | go through the file's diagnostics, one note at a time |
| `]d`, `[d` | the next, previous diagnostic |
| `space j` | the next error (round to the first), or with none, the next warning |
| `:format` | format the file |

While you type, completions show in a menu, and a call's signature shows as you type its arguments. When a server asks to show a document, Ghostwrite goes there.

## Diagnostics

Diagnostics show in the code as they arrive. The status line shows the one at the cursor. `space d` turns them into a [story](../stories/reviewing.md#stories-the-editor-gives): Tab reads on, Enter asks the model about the one you're on. The model is also sent the diagnostics near the lines you ask about.

`space j` includes a language's own kinds of problem: Lean's unsolved goals and `sorry`s among them. In a file with pages, the pages follow.

## Formatting

`:format` (or `:fmt`) formats the file with its server. Saving formats it too, unless `"format_on_save": false` is in the [config](../configuration/README.md).

## Installing servers

Ghostwrite doesn't install servers. Install them as you would for any editor, so they're on your `PATH`:

| Language | Servers, in the order they're tried |
| --- | --- |
| Rust | `rust-analyzer` |
| C, C++ | `clangd` |
| Go | `gopls` |
| JavaScript, TypeScript, JSX, TSX | `typescript-language-server` |
| Python | `basedpyright-langserver`, `pyright-langserver`, `pylsp` |
| Typst | `tinymist` |
| Lean | `lake serve` |

Python's servers are started in the project's virtualenv (`.venv` or `venv` in the project, or `$VIRTUAL_ENV`) and told its interpreter, so they see the project's packages.
