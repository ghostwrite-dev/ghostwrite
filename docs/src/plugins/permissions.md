# Permissions and the sandbox

## What a plugin's code can do

A plugin's code runs in WebAssembly, in a sandbox. By itself it gets no files, no network and no environment. It sees what it's asked about (the file, the cursor, the selection) and answers with what it would like done:

- say something in the status line;
- edit the text it was shown (as one undo step, and not if the text has changed since);
- open a file of the project.

The editor does that, or says why not. A plugin can't do anything you don't see it doing, and the editor's own commands can't be replaced.

## What it can ask for

Anything more, a plugin asks for in its manifest's `"access"`, and you're shown it when you install:

| Access | What it means | Shown as |
| --- | --- | --- |
| `project` | the project, read-only, when it makes pages of a file | reads the project |
| `fonts` | the system's font folders, read-only | reads the system's fonts |
| `cache` | a cache folder of its own | |
| `read` | folders under your data or cache folder, read-only | reads `{data}/…` |
| `write` | folders under your data or cache folder, to write | writes `{cache}/…` |
| `network` | downloads, over https, from these hosts only | downloads from `packages.typst.org` |
| `env` | these environment variables, as its language servers would have them | reads `$PATH`, `$VIRTUAL_ENV` as its servers have them |
| `memory_mb`, `seconds` | more memory or time than the default | may use 4096 MB and 120 s at a time |

## Limits

A plugin's code is stopped if a call takes more than 5 seconds or 256 MB of memory, unless it asks for more (at most 300 seconds and 4096 MB). It starts afresh the next time it's needed. A slow plugin holds nothing up: plugins are asked off the editor's main thread.

## What the sandbox doesn't cover

- **Language servers.** A server a plugin names is an ordinary program that runs as you do, with your permissions. That's why installing a plugin says which commands it starts: `starts rust-analyzer with your permissions`.
- **Grammars.** A tree-sitter grammar runs in WebAssembly too, but isn't stopped if it runs away. The worst it can do is hold up coloring.

## What installing says

`:plugin install` fetches a plugin and checks it, and says everything it adds in one line, in the status line. Nothing is installed until you type `:plugin install!` with the same source. For Typst, for example:

```text
typst (version 1.0.0; .typ files, highlighted; starts `tinymist` with your permissions;
reads the project; reads the system's fonts; reads {data}/typst/packages;
writes {cache}/typst/packages; downloads from packages.typst.org;
may use 4096 MB and 120 s at a time). :plugin install! ghostwrite-dev/typst to install it
```

The line names, in order: its version; its `:` commands; when it runs (`runs when a file is saved`); each language's files, and whether they're highlighted or typed with abbreviations; each server it starts; what it shows beside its files; what it may reach; its keys; and whether it adds rules to the model's prompt. A release with no checksum adds `. <zip> has no checksum beside it to check it against` before the last part.
