# Ghostwrite

A helix-style modal editor for macOS, built on [gpui](https://www.gpui.rs),
with Claude in it. Point at some lines (`a`) and say what you want:
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

The keys are helix's; `?` shows them all. The parts that aren't helix:

| Keys | What it does |
| --- | --- |
| `a` | ask Claude about the line or selection; Tab accepts, `a` or Enter says what you want instead, Esc stops |
| `space k` | docs for what's under the cursor |
| `space d` | go through the file's diagnostics, one note at a time |
| `space /`, `:grep` | search the project and go through the matches |
| `:sh ‹cmd›`, `:stop` | run a command in the background; Claude is told it and its output |
| `:output` | go through the last command's errors (or its output) |
| `f`, `space e` | the file tree; Claude's file operations are proposed there |
| `⌘=`, `⌘-`, `⌘0` | zoom the window in, out, and back to 100% |

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
  "max_recent_projects": 20
}
```

`GHOSTWRITE_AI_EFFORT` (`low` by default, up to `max`) sets how hard Claude thinks.

## Code

Layered like helix and zed: pure crates at the bottom, the gpui app on top,
and nothing below `app` knows about the UI.

| Crate | What's in it |
| --- | --- |
| `core` | text, selections, motions, text objects, the modal keymap, syntax |
| `project` | files, search, the file tree model, shell runs and their stops, watching the disk, saved state |
| `lsp` | the language-server client |
| `ai` | Claude: the reply format, prompts, backends, the review story, placing steps |
| `app` | the application (binary `ghostwrite`) |

Each module starts with a comment saying what it is; `crates/app/src/editor/mod.rs`
is the way in to the editor.

```sh
./check.sh                                  # formatting, clippy, and the tests: run before committing
cargo test -p ghostwrite-ai -- --ignored     # live checks against Claude Code
```
