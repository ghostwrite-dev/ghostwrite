# Bundled languages

Thirteen plugins come with the app. They live in [ghostwrite-dev/default-plugins](https://github.com/ghostwrite-dev/default-plugins).

| Plugin | Files | Comments | Language server |
| --- | --- | --- | --- |
| `bash` | `.sh` `.bash` `.zsh`, `.bashrc` `.zshrc` `.profile` | `#` | |
| `c` | C: `.c` `.h`; C++: `.cc` `.cpp` `.cxx` `.hpp` `.hh` `.hxx` | `//` | `clangd` |
| `css` | `.css` | | |
| `go` | `.go` | `//` | `gopls` |
| `image` | `.png` `.jpg` `.jpeg` `.gif` `.webp` `.bmp` `.tif` `.tiff` `.ico` | | |
| `javascript` | JavaScript: `.js` `.mjs` `.cjs`; JSX: `.jsx`; TypeScript: `.ts` `.mts` `.cts`; TSX: `.tsx` | `//` | `typescript-language-server` |
| `json` | `.json` | | |
| `markdown` | `.md` `.markdown` | | |
| `pdf` | `.pdf` | | |
| `python` | `.py` `.pyi` | `#` | `basedpyright`, `pyright` or `pylsp` |
| `rust` | `.rs` | `//` | `rust-analyzer` |
| `toml` | `.toml`, `Cargo.lock` | `#` | |
| `yaml` | `.yaml` `.yml` | `#` | |

Every language among them has a tree-sitter grammar for highlighting, and helix's text objects (`maf`, `]f` and the rest). Markdown highlights code blocks in their own language.

## On demand

| Plugin | Install with | Adds |
| --- | --- | --- |
| Typst | `:plugin install ghostwrite-dev/typst` | `.typ` files, their pages as you type, `tinymist` |
| Lean | `:plugin install ghostwrite-dev/lean` | `.lean` files, its goals beside the file, abbreviations, `lake serve` |

See [Pages and what's beside a file](pages.md) for what they do.

## Anything else

Any other file opens as plain text. To add a language, see [A language with no code](../writing-plugins/language.md): most are a `plugin.json`, a grammar and its queries.
