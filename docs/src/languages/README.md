# Languages

Built in, Ghostwrite knows only plain text. Every language is a [plugin](../plugins/README.md): its file types, its grammar for highlighting and text objects, its comment syntax, and the language servers it starts. Thirteen plugins come with the app, and Typst and Lean are a command away. See [Bundled languages](bundled.md).

- [Language servers](servers.md): definitions, references, rename, diagnostics, formatting, completions.
- [The debugger](debugger.md): breakpoints and stepping, through any debug adapter.
- [Pages and what's beside a file](pages.md): PDFs, images, Typst's pages and Lean's goals.

## Highlighting

Colors come from the language's tree-sitter grammar, and from its language server over them where the server has its own (semantic tokens). The server knows what each name is: a local, a type, a constant. While you type, its last colors stay, moved past the edit, until it sends new ones. Only what changed is colored again.

## Comments and indentation

`ctrl-c` comments lines out and back in with the language's comment leader. A new line goes in a level after a line ending in an opening bracket (or what the language says: Python adds `:`).
