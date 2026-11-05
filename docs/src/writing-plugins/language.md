# A language with no code

Most languages need no code: a folder with a `plugin.json`, a grammar and its queries.

```text
zig/
  plugin.json
  tree-sitter-zig.wasm
  highlights.scm
  textobjects.scm
```

```json
{
  "name": "zig",
  "version": "1.0.0",
  "api": 1,
  "extensions": ["zig", "zon"],
  "comment": "//",
  "grammar": {
    "name": "zig",
    "wasm": "tree-sitter-zig.wasm",
    "query": "highlights.scm",
    "textobjects": "textobjects.scm"
  },
  "servers": [{ "name": "zls", "command": ["zls"], "roots": ["build.zig"] }]
}
```

Then `:plugin link zig` (or `:plugin install zig`) and open a `.zig` file.

## Even smaller

Without a grammar, a plugin can be a single `.json` file in the `plugins/` folder, with no `"api"`. This one adds the language's files and comments, its server, a key and a rule for the model:

```json
{
  "name": "zig",
  "extensions": ["zig"],
  "comment": "//",
  "server": ["zls"],
  "keys": [{ "keys": "space z", "command": "term", "what": "the terminal" }],
  "rules": "- Keep functions small."
}
```

`"server"` is the short form: one command, a server named after the plugin (`zig-server`).

## The language's fields

| Field | What it is |
| --- | --- |
| `extensions` | file extensions, without the dot: `["zig", "zon"]` |
| `files` | whole file names: `["Makefile"]` |
| `id` | what language servers call it, if that isn't its name (`"shellscript"` for bash) |
| `comment` | the line comment's leader, for `ctrl-c` and `J` |
| `pairs` | more pairs that close themselves as they're typed, two chars each: `["$$"]` |
| `unpaired` | quotes that don't close themselves (Rust's `'`): `["'"]` |
| `indent_after` | what a line ends with when the next one goes in a level. Brackets unless it says: `"{[(:"` |
| `grammar` | its tree-sitter grammar and queries: see [Grammars and queries](grammars.md) |
| `abbreviations` | a table of `\` abbreviations: see [Abbreviations and tokens](abbreviations.md) |

A language needs `extensions` or `files`. Without either, the plugin adds no language.

## Several languages

A plugin can have several languages under `"languages"`, each written the same way, with a grammar each. They're served by the plugin's servers, unless one names others:

```json
{
  "name": "c",
  "version": "1.0.0",
  "api": 1,
  "languages": [
    { "name": "c", "extensions": ["c", "h"], "comment": "//",
      "grammar": { "name": "c", "wasm": "tree-sitter-c.wasm",
                   "query": "highlights.scm", "textobjects": "textobjects.scm" } },
    { "name": "cpp", "extensions": ["cc", "cpp", "hpp"], "comment": "//",
      "grammar": { "name": "cpp", "wasm": "tree-sitter-cpp.wasm",
                   "query": ["highlights-cpp.scm", "highlights.scm"],
                   "textobjects": "textobjects-cpp.scm" } }
  ],
  "servers": [{ "name": "clangd", "command": ["clangd"] }]
}
```

A language among `"languages"` can say `"servers": ["other-ls"]` to be served by those instead.

## Language servers

```json
"servers": [
  { "name": "zls", "command": ["zls"], "roots": ["build.zig"] }
]
```

| Field | What it is |
| --- | --- |
| `name` | the server's name: letters, digits, `-` and `_` |
| `command` | the program and its arguments |
| `roots` | files that mark the folder it's started in, nearest the file up. Without them, or none found, the project's |
| `configured_by_code` | the plugin's code says what it starts with, and its settings: see [Configuring servers](servers.md) |

When a language lists several servers, the first one installed is started.

## Keys and rules

```json
"keys": [{ "keys": "space z", "command": "term", "what": "the terminal" }],
"rules": "- Prefer `where` clauses over long bounds."
```

A key runs a `:` command (with or without its `:`), or one of the editor's commands by name (`zoom_in`), in normal and select mode, for files of the plugin's languages; a plugin with no languages binds it for every file. Keys show under **Plugin keys** in `space ?`. A key only works where the keymap doesn't bind it, so the editor's keys and the user's own come first.

A key is for the code (and a tab of its pages) unless it says which [view](../editing/modes.md#views) it's for, with `"in"`: `"pages"`, `"aside"` (the side view), `"files"` or `"terminal"`:

```json
"keys": [{ "in": "pages", "keys": "space n", "command": "scroll_page_down", "what": "the next page" }]
```

`rules` are lines starting `- `, added to the model's prompt.
