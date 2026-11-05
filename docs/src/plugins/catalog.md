# Catalog

## Come with the app

These live in [ghostwrite-dev/default-plugins](https://github.com/ghostwrite-dev/default-plugins), and are installed with Ghostwrite.

| Plugin | What it adds | Code |
| --- | --- | --- |
| `bash` | Bash and Zsh | no |
| `c` | C and C++, `clangd` | no |
| `css` | CSS | no |
| `go` | Go, `gopls` | no |
| `image` | images, as pages | yes |
| `javascript` | JavaScript, JSX, TypeScript and TSX, `typescript-language-server` | no |
| `json` | JSON | no |
| `markdown` | Markdown, with code blocks highlighted in their language | no |
| `pdf` | PDFs, as pages | yes |
| `python` | Python, with `basedpyright`, `pyright` or `pylsp` started in the project's virtualenv | yes |
| `rust` | Rust, `rust-analyzer` | no |
| `toml` | TOML | no |
| `yaml` | YAML | no |

Most are no code at all: a `plugin.json`, a tree-sitter grammar compiled to WebAssembly, and its queries.

## Install on demand

| Plugin | Install | What it adds |
| --- | --- | --- |
| [Typst](https://github.com/ghostwrite-dev/typst) | `:plugin install ghostwrite-dev/typst` | `.typ` files, highlighted by the plugin; their pages, compiled as you type; `tinymist` |
| [Lean](https://github.com/ghostwrite-dev/lean) | `:plugin install ghostwrite-dev/lean` | `.lean` files; its goals beside the file; `\` abbreviations; `lake serve` |

See [Pages and what's beside a file](../languages/pages.md) for both.

## Examples

`examples/word-count` in the Ghostwrite repository is a small plugin with code: `:word-count` says how many words the file has, `:shout` upper-cases the selection, and it says the count whenever a file is saved. See [A plugin with code](../writing-plugins/code.md).
