# Ghostwrite

A helix-style modal editor for macOS, built on [gpui](https://www.gpui.rs),
with Claude in it. Point at some lines (`space a`) and say what you want:
a question is answered with notes pinned to the code, across files; a change
comes as small whole-line steps you accept with Tab, one at a time, like
autocomplete; one you don't want, you redirect. Nothing changes until you accept it, and
anything you say mid-review (a question, a correction) is a follow-up that
can revise what comes next.

## Install

```sh
./install.sh   # builds a release and installs /Applications/Ghostwrite.app
```

Building needs Rust and the Metal toolchain (the shaders compile at launch).
Claude is reached with `ANTHROPIC_API_KEY` if it's set, else through Claude
Code (`claude` on the PATH or in `~/.local/bin`), under your login.

## Using it

The keys are helix's, as helix has them; `space ?` shows them all. Ghostwrite's
own are under `space`, and ⌘ is for the app: tabs, windows, files. As in
helix, brackets and quotes close themselves while you type.

| Keys | What it does |
| --- | --- |
| `space a` | ask Claude about the line or selection; Tab accepts, Enter (or `space a`) says what you want instead, Esc stops |
| `space e`, `⌘B` | the file tree (`a` `r` `d`: add, rename, trash); Claude's file operations are proposed there |
| `space t`, `⌘J` | terminal mode: your shell, under the code, has the keys; Esc Esc goes back, `⌘J` hides it |
| `/`, `?`, `n`, `N`, `*` | search the file (as you type, every match marked), go to the next and previous match, search for the selection |
| `space f`, `space b` | pick a file of the project, or an open one, by typing a few of its letters |
| `.`, `Q`/`q`, `"a` | repeat the last change; record and play a macro; use a named register (`"+` is the clipboard) |
| `J`, `ctrl-a`/`ctrl-x`, `ctrl-c` | join lines; a number up or down; comment lines out and back in |
| `ctrl-o`, `ctrl-i` | back to where a jump (`gd`, `G`, a search, another file) came from, and forward again |
| `space /`, `:grep` | search the project and go through the matches |
| `space k`, `space d` | docs for what's under the cursor; go through the file's diagnostics, one note at a time |
| `gr`, `space r`, `space .`, `space s` | the language server: go through the references, rename everywhere, its code actions here, pick one of the file's symbols |
| `:format` | format the file with its language server (saving does too, unless `format_on_save` is off) |
| `space j` | the next error (round to the first), or with none, the next warning; Lean's unsolved goals and `sorry`s among them, and a Typst file's pages follow |
| `:sh ‹cmd›`, `:stop` | run a command in the background; Claude is told it and its output |
| `:output` | go through the last command's errors (or its output) |
| click | moves the cursor there; shift-click, or a click after `v`, selects to it; a double click is the word |
| `⌘N` `⌘W`, `⌘⇧[` `⌘⇧]` | new and close tab, previous and next tab |
| `⌘O`, `⌘⇧O`, `⌘⇧N` | open a file, a folder; a new window (each project is open in one window) |
| `⌘=`, `⌘-`, `⌘0` | zoom the window in, out, and back to 100% |

Claude reads the terminal: what's on it (and some scrollback) goes with
each request, and again in a follow-up when it's changed. Only you type
into it.

A Typst file shows its pages beside it, compiled as you type, saved or not
(`:preview` hides them, and their edge drags to resize). Typst is built in:
the project's files and fonts are used as they are, and packages come from
Typst's cache, downloaded the first time. The cursor and the selection show
on the pages too, on the glyphs they're on, and clicking a glyph puts the
cursor on the text it came from. Moving around leaves the pages be; an edit
that takes you to another line, or `space j`, brings them to the cursor.

Colors come from the grammar, and from the language server over them where
it has its own (semantic tokens): it knows what each name is, a local, a
type, a constant. While you type, its last colors stay, moved past the
edit, until it sends new ones. Lean has no grammar that parses it well (its
syntax grows with every `notation`), so it's colored by its tokens until
Lean's own colors arrive.

A Lean file shows its goals beside it, as Lean's infoview does: the proof
state at the cursor (or the type expected there), the messages on the
cursor's line (`#eval`'s output among them), and how far Lean has got
checking the file (`:preview` hides them too). Lean runs through `lake
serve`, started in the file's Lake project, so the project's toolchain and
packages are used. A backslash types a symbol, as in Lean's VS Code
extension and with its abbreviations: `\to` is `→`, `\N` is `ℕ`, `\<>` is
`⟨⟩` around the cursor. An abbreviation turns into its symbol as soon as no
longer one starts with it, or when you type a space or another char that
can't go on to one (Tab finishes it alone); `\\` is a backslash.

A PDF opens as its pages. It's read-only text underneath: the motions and
selecting (keys or mouse) work as in code, and you can copy from it. Claude
can read a PDF, but doesn't comment on one: ask in the file it's made from.

Claude writes math in its notes as Typst math (`$…$`), and it's shown
typeset by the built-in Typst; in a Typst file the whole note is Typst.

The tree and open files follow changes other programs make. A file you have
unsaved changes in keeps them, and the status line says it changed on disk.

## Settings

`~/.config/ghostwrite/config.json`, every key optional:

```json
{
  "model": "claude-opus-5-5",
  "tab_width": 4,
  "font_size": 13,
  "margin_lines": 3,
  "max_file_size_mb": 20,
  "context_bytes": 200000,
  "max_diff": 12000,
  "max_recent_projects": 20,
  "format_on_save": true
}
```

`GHOSTWRITE_AI_EFFORT` (`low` by default, up to `max`) sets how hard Claude thinks.

## Code

Layered like helix and zed: pure crates at the bottom, the gpui app on top,
and nothing below `app` knows about the UI.

| Crate | What's in it |
| --- | --- |
| `core` | text, selections, motions, text objects, the modal keymap, syntax, Lean's abbreviations |
| `project` | files, search, the file tree model, shell runs and their stops, the terminal's shell, watching the disk, saved state |
| `pdf` | PDFs through macOS's PDFKit: their text, where each character is on the page, pages drawn |
| `typst` | Typst compiled in-process: a file's pages as it's typed, each glyph mapped to its source, pages drawn |
| `lsp` | the language-server client |
| `ai` | Claude: the reply format, prompts, backends, the review story, placing steps |
| `app` | the application (binary `ghostwrite`) |

Each module starts with a comment saying what it is; `crates/app/src/editor/mod.rs`
is the way in to the editor.

```sh
./check.sh                                  # formatting, clippy, and the tests: run before committing
cargo test -p ghostwrite-ai -- --ignored     # live checks against Claude Code
```
