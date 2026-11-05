# Manifest reference

A plugin says what it adds in JSON: `plugin.json` in a folder of its own, or, for a plugin with no code, grammar or abbreviations, a single `.json` file in the plugins folder. It's read when the editor starts, or when the plugin is installed, so nothing has to be rebuilt.

Every file a manifest names has to be inside the plugin's folder: no `..`, no absolute paths, and no links that lead out of it.

## The plugin

| Field | Type | What it is |
| --- | --- | --- |
| `name` | string | **Required.** Letters, digits, `-` and `_`, not starting with `-`. Also the name of its own language, if it has one |
| `version` | string | its version, shown when it's installed and listed |
| `api` | number | the plugin interface it was built for: `1`. **Required** with `code`, a `grammar` or `grammars` |
| `code` | path | its WebAssembly, built for `wasm32-wasip2` |
| `commands` | list | `:` commands its code runs: `[{ "name": "word-count", "what": "count the words" }]`. Names are letters, digits, `-` and `_` |
| `events` | list | what its code hears of: `"opened"`, `"saved"`, `"closed"` |
| `keys` | list | key bindings: `[{ "keys": "space z", "command": "term", "what": "…" }]`. `command` is a `:` command (with or without its `:`) or an editor command by name (`zoom_in`). For its languages' files, or every file if it has none. `"in"` names the view they're for: `"pages"`, `"aside"`, `"files"` or `"terminal"`; without it, the code (and a tab of its pages) |
| `rules` | string | lines starting `- ` added to the model's prompt |
| `tokens` | object | kinds of semantic token its servers name, and the [scope](grammars.md#highlights) each shows as: `{ "leanSorryLike": "constant.builtin" }` |
| `server` | list | the short form of one server: its command (`["zls"]`), named `‹name›-server` |
| `servers` | list | its language servers: see below |
| `languages` | list | more languages, each written as the plugin's own: see below |
| `grammars` | list | grammars that aren't a language's own, there to be injected by name, each as a `grammar` |
| `pages` | bool | its code makes pages of its language's files: see [Pages](pages.md) |
| `only_pages` | bool | its files aren't text: they open as pages, read-only. Needs `pages` |
| `pages_at_own_size` | bool | pages are never shown bigger than they are |
| `highlights` | bool | its code colors its language, instead of a grammar |
| `panel` | object | a panel beside its files: see below |
| `access` | object | what its code may reach: see below |

## The plugin's own language

A plugin's own language takes these fields at the top level. A plugin with neither `extensions` nor `files` has no language of its own.

| Field | Type | What it is |
| --- | --- | --- |
| `extensions` | list | file extensions, without the dot |
| `files` | list | whole file names: `["Makefile"]` |
| `id` | string | what language servers call it, if that isn't its name |
| `comment` | string | the line comment's leader |
| `pairs` | list | more pairs that close themselves, two chars each: `["$$"]` |
| `unpaired` | list | quotes that don't close themselves: `["'"]` |
| `indent_after` | string | what a line ends with when the next goes in a level. Default: brackets |
| `grammar` | object | its grammar and queries |
| `abbreviations` | path | a table of `\` abbreviations: lines of an abbreviation, a tab and its symbol |

## Languages

Each entry of `languages` takes the fields above, with its own `name` (required) and either `extensions` or `files`. It can also say:

| Field | Type | What it is |
| --- | --- | --- |
| `servers` | list | the names of the servers that serve it, in place of the plugin's |

Only the plugin's own language can be highlighted by code (`highlights`) or be pages (`only_pages`).

## Grammar

| Field | Type | What it is |
| --- | --- | --- |
| `name` | string | **Required.** The grammar's name |
| `wasm` | path | **Required.** The grammar, built with `tree-sitter build --wasm` (0.22 or later) |
| `query` | path or list | how it's colored: several files are read in order, the last winning |
| `injections` | path | which other languages are inside it |
| `textobjects` | path | what `maf`, `]f` and the rest select |

A language can't have both a `grammar` and `highlights`.

## Servers

| Field | Type | What it is |
| --- | --- | --- |
| `name` | string | **Required.** Letters, digits, `-` and `_` |
| `command` | list | **Required.** The program and its arguments |
| `roots` | list | files that mark the folder it's started in, nearest the file up; else the project's |
| `configured_by_code` | bool | its code says what it starts with and answers its settings: see [Configuring servers](servers.md). Needs `code` |

## Panel

| Field | Type | What it is |
| --- | --- | --- |
| `title` | string | its title; the plugin's name if not given |
| `requests` | list | requests to the server about the cursor |
| `notifications` | list | the server's notifications the panel hears of |

Needs `code`, and at least one language. See [Beside a file](panels.md).

## Access

What code may reach beyond what it's handed. Nothing, to begin with. All of it is shown when the plugin is installed. See [Permissions and the sandbox](../plugins/permissions.md).

| Field | Type | What it gives | Mounted at |
| --- | --- | --- | --- |
| `project` | bool | the project, read-only | `/project` |
| `fonts` | bool | the system's font folders, read-only | `/fonts` |
| `cache` | bool | a folder of the plugin's own | `/cache` |
| `read` | list | folders under the user's data or cache folder, read-only: `"{data}/typst/packages"` | `/data/…`, `/user-cache/…` |
| `write` | list | the same, to write: `"{cache}/typst/packages"` | `/data/…`, `/user-cache/…` |
| `network` | list | hosts it may download from, over https, with `fetch` | |
| `env` | list | environment variables it's shown when asked about a server of its own, as that server would have them | |
| `memory_mb` | number | memory a call may use. Default 256, at most 4096 | |
| `seconds` | number | time a call may take. Default 5, at most 300 | |

`read` and `write` only take folders under `{data}` or `{cache}`. `network` takes host names, `env` variable names.

## A full example

```json
{
  "name": "typst",
  "version": "1.0.0",
  "api": 1,
  "code": "typst.wasm",
  "pages": true,
  "extensions": ["typ"],
  "comment": "//",
  "pairs": ["$$"],
  "server": ["tinymist"],
  "highlights": true,
  "access": {
    "project": true,
    "fonts": true,
    "cache": true,
    "read": ["{data}/typst/packages"],
    "write": ["{cache}/typst/packages"],
    "network": ["packages.typst.org"],
    "memory_mb": 4096,
    "seconds": 120
  }
}
```

## What's refused

A manifest is checked when it's read, and a plugin that breaks a rule isn't loaded; `:plugins` says why. Among the rules:

- it has a `name`, and the names it uses are letters, digits, `-` and `_`;
- with `code` or a grammar, it says which `api` it was built for, and that's the editor's;
- `commands`, `events`, `pages`, `panel` and `configured_by_code` need `code`;
- `only_pages` needs `pages`;
- a grammar and `highlights` aren't both there;
- a language with a grammar says which files are in it;
- every scope in `tokens` is one the editor knows;
- `access` asks for no more than the most it can have.

Its code and grammars are also loaded and checked before an install puts it in place.
