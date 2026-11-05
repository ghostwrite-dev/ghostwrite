# Grammars and queries

A language is highlighted, and gets text objects, from a [tree-sitter](https://tree-sitter.github.io) grammar compiled to WebAssembly and a few query files.

```json
"grammar": {
  "name": "zig",
  "wasm": "tree-sitter-zig.wasm",
  "query": "highlights.scm",
  "injections": "injections.scm",
  "textobjects": "textobjects.scm"
}
```

| Field | What it is |
| --- | --- |
| `name` | the grammar's name, as tree-sitter knows it |
| `wasm` | the grammar, built for WebAssembly |
| `query` | how it's colored: a file, or several read in order (`["highlights-cpp.scm", "highlights.scm"]`), the last winning where two color the same thing |
| `injections` | which other languages are inside it, by name |
| `textobjects` | what `maf`, `]f` and the rest select |

Every file the manifest names has to be in the plugin's folder.

## The grammar

A grammar is what `tree-sitter build --wasm` makes. It has to be from tree-sitter 0.22 or later; older ones aren't read. Many grammar projects publish a `.wasm` with each release, so you may not need to build one.

A grammar running in WebAssembly is a little slower than one compiled in: a 10,000-line Rust file parses in about 40 ms rather than 26, and is colored in about 62 ms rather than 47. Only what changed is parsed and colored again after an edit.

## Highlights

`highlights.scm` captures nodes with the names helix or nvim-treesitter use, so their queries work as they are. A capture shows as the longest of these scopes all of whose parts it has (`keyword.control` shows as `keyword`):

```text
attribute  boolean  comment  comment.doc  constant  constant.builtin  constructor
embedded  escape  function  function.builtin  function.macro  function.method
keyword  label  module  number  operator  property  punctuation
punctuation.bracket  punctuation.delimiter  punctuation.special  string
string.escape  string.regex  string.special  string.special.symbol  tag
text.emphasis  text.literal  text.reference  text.strong  text.title  text.uri
type  type.builtin  variable  variable.builtin  variable.parameter
```

## Text objects

`textobjects.scm` captures `function.inside` and `function.around`, `class.*`, `parameter.*`, `comment.*`, `test.*`, `entry.*` and `xml-element.*`, as helix's do. Helix's own (`runtime/queries/*/textobjects.scm` in [helix](https://github.com/helix-editor/helix)) work as they are, with the files each inherits from put in it.

Helix's queries are under the [Mozilla Public License 2.0](https://mozilla.org/MPL/2.0/).

## Injections

`injections.scm` says what other languages are inside this one, by name: a Markdown code block's language, say. A grammar that's only there to be injected (Markdown's inline text) goes under `"grammars"` at the top of the manifest:

```json
{
  "name": "markdown",
  "version": "1.0.0",
  "api": 1,
  "extensions": ["md", "markdown"],
  "grammar": { "name": "markdown", "wasm": "tree-sitter-markdown.wasm",
               "query": "highlights.scm", "injections": "injections.scm" },
  "grammars": [{ "name": "markdown_inline", "wasm": "tree-sitter-markdown_inline.wasm",
                 "query": "highlights-inline.scm" }]
}
```

## Fetching grammars instead of committing them

The default plugins don't keep the `.wasm` in their repository. Each lists its grammars in `grammars.txt`, one per line, the sha256 and then where the grammar is published:

```text
# sha256, then where it is.
24c89bd9252255e4aebbcbd7d2d308bd92c86dd95a130fdc80efa49577b8d738 https://github.com/tree-sitter/tree-sitter-rust/releases/download/v0.24.2/tree-sitter-rust.wasm
```

The build downloads each one once, checks it against its sha256, and puts it in the plugin. See [Building and releasing](releasing.md).
