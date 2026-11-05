# Writing plugins

A plugin is a folder with a `plugin.json` in it. It needs only the parts it uses:

| Part | What it is | Needs code? |
| --- | --- | --- |
| a language | file types, comments, brackets, indentation | no |
| a grammar and queries | highlighting, text objects, languages inside it | no |
| language servers | the commands that start them, and their root files | no |
| key bindings | a key sequence running a `:` command | no |
| rules | lines added to the model's prompt | no |
| abbreviations and tokens | `\alpha` typing `α`; colors for a server's own kinds of token | no |
| `:` commands and events | code that runs when you type a command, or a file is opened, saved or closed | yes |
| highlighting by code | for a language no grammar parses well | yes |
| pages | a file shown as pages in a tab (a PDF, a typeset document) | yes |
| a panel | something in the side view beside a file, kept up to date from its server | yes |
| server configuration | what a server starts with, and its settings | yes |

Code is Rust compiled to WebAssembly (`wasm32-wasip2`), so one build runs on every system, and can't take the editor down.

## Where to start

- [A language with no code](language.md) is the place to start for most plugins.
- [A plugin with code](code.md) sets up a Rust crate and adds a command.
- The [manifest reference](manifest.md) lists every field.
- [Building and releasing](releasing.md) publishes it, so that `:plugin install owner/repo` works.

The plugins in [ghostwrite-dev/default-plugins](https://github.com/ghostwrite-dev/default-plugins) are good examples:

| Plugin | Shows |
| --- | --- |
| `rust` | a language that's no code at all |
| `c`, `javascript` | several languages in one plugin, sharing servers |
| `markdown` | an injected grammar |
| `python` | code that configures its servers |
| `pdf`, `image` | files that are only pages |

And [Lean](https://github.com/ghostwrite-dev/lean) colors itself, types symbols from abbreviations and has a panel; [Typst](https://github.com/ghostwrite-dev/typst) makes pages from text as you type.

## The plugin interface

The interface between the editor and a plugin's code is version 1: `"api": 1` in the manifest, and the WIT package `ghostwrite:plugin@1.0.0`. Version 1 only grows in ways plugins built against it can't notice; anything else would be version 2. An editor refuses a plugin built for another version, and `:plugins` says so.

A plugin with no code and no grammar doesn't need to say `"api"`.

## While you work on one

Link the folder, so the editor reloads it whenever it changes:

```text
:plugin link ~/code/my-plugin
```

`:plugins` shows what each plugin adds, and what's wrong with any that couldn't be read.
