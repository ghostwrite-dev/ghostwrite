# Contributing

Ghostwrite is at [github.com/ghostwrite-dev/ghostwrite](https://github.com/ghostwrite-dev/ghostwrite). It builds on macOS, Windows and Linux; [Install](install.md#from-source) says what each needs.

## Before you commit

```sh
./check.sh
```

`check.sh` runs what CI runs on all three systems, and what has to pass:

1. `cargo fmt --check`;
2. `cargo clippy --workspace --all-targets -- -D warnings`;
3. `scripts/build-plugins.sh`, which downloads the plugins `plugins.lock` and `test-plugins.lock` pin;
4. `cargo test --workspace`;
5. a check that it builds without local models: `cargo check -p ghostwrite --no-default-features --features wasm-plugins`.

CI pins the Rust toolchain it's developed with (1.97.0), so a new release's lints don't fail it unasked.

## Tests

A change comes with a test that fails without it.

- **The model is never asked in a test.** Stories are driven with scripted replies: a backend that answers with given texts in turn. The keys go through the modal editor as they would in AI mode, the replies arrive as events, and what the app would do is checked. That tests the editor, which is what's under test, and keeps the tests fast and the same every time.
- Tests that need a language server that may not be installed are `#[ignore]`d.
- A test that only works on one system says so, with `#[cfg(unix)]` and the like.
- Plugins are built for WebAssembly and run in `crates/wasm/tests`.

## Layout

Layered like helix and zed: pure crates at the bottom, the gpui app on top, and nothing below `app` knows about the UI.

| Crate | What's in it |
| --- | --- |
| `core` | text (a rope), selections, motions, text objects, the modal keymap, what a language can be (plain text, built in, is none), syntax over any grammar (highlights and the tree, kept as the text is edited, text objects), abbreviations |
| `project` | files, search, the file tree's model, shell runs, watching the disk, saved state |
| `terminal` | the user's shell on a pseudo-terminal, emulated by alacritty: its screen, to draw and to read |
| `page` | what a document's pages are, whoever lays them out: their sizes, and pages drawn |
| `lsp` | the language-server client, and starting the servers plugins add |
| `dap` | the debug adapter client |
| `plugin` | what a plugin can add, the plugins there are, and installing them |
| `plugin-api` | what a plugin written in Rust compiles against, and the interface itself (`wit/plugin.wit`) |
| `wasm` | runs plugins compiled to WebAssembly, sandboxed, and the grammars they bring |
| `model` | a local model: llama.cpp, in a process of its own, and the memory it may use |
| `ai` | the model: the reply format, prompts, backends, the review story, placing steps, its notes kept per project |
| `state` | the editor's state, without drawing: documents, tabs, the project, search, registers, jumps, `:` commands, the story under review, the config |
| `app` | the application (binary `ghostwrite`) |

Each module starts with a comment saying what it is. `crates/app/src/editor/mod.rs` is the way in to the editor.

## Where things go

- **Keys.** The default keymap is data: `crates/core/src/modal/keymap.rs`, in the cheatsheet's sections, so `space ?` comes from what's bound. `DEFAULT` is the code's keys and `VIEWS` each other view's layer. Commands and their names are in `command.rs` beside it. What the cheatsheet says by hand (typing, the mouse, the terminal's shell, reviewing, `:` commands, the window) is `BY_HAND` in `crates/app/src/editor/view.rs`, and has to be kept in step. The key reference in these docs is made from the same tables.
- **Views.** Which view has the keys, and the keys of views with no text, are in `crates/core/src/modal/views.rs`; moving between them in `crates/app/src/editor/views.rs`.
- **`:` commands** are parsed in `crates/state/src/commands.rs` and run in `crates/app/src/editor/commands.rs`.
- **Settings** are in `crates/state/src/config.rs`.
- **What's particular to a system** goes in `crates/project/src/platform.rs` if it can, behind `cfg` where it can't.

## Languages are plugins

The editor knows no file type in particular. Every language is a plugin, and nothing built in names one: a test keeps every crate from naming a language, and the build from building one in. `crates/app/src/plugins.rs` finds them in their folders. A change to what plugins can do goes in the plugin interface (`crates/plugin-api/wit/plugin.wit`) or the manifest (`crates/plugin/src/manifest.rs`).

The plugins themselves are in their own repositories: [default-plugins](https://github.com/ghostwrite-dev/default-plugins) (those the app comes with), [typst](https://github.com/ghostwrite-dev/typst) and [lean](https://github.com/ghostwrite-dev/lean). They build against the interface by its tag, `plugin-api-v<version>`, and with `.github/workflows/plugin.yml` at the same tag. After a change to the interface, they're released again and the lock files pinned to the new releases.

### Plugins in the build

`scripts/build-plugins.sh` puts the plugins the app comes with in `target/bundled-plugins`, which the app loads when run from the source and the installers copy. They're pinned in `plugins.lock`, a line each: `<name> <tag> <url> <sha256>`. Each zip is downloaded once into `target/plugin-downloads`, checked against its sha256, and unzipped. Typst and Lean, which only the tests use, are pinned the same way in `test-plugins.lock` and go to `target/test-plugins`, which nothing ships.

To work on the default plugins, set `GHOSTWRITE_PLUGINS_SRC` to a default-plugins checkout, and the script builds those in `plugins.lock` from it instead, with `scripts/plugin-build.sh`.

The plugin interface is version 1. Once released, it only grows in ways plugins built against it can't notice; anything else is version 2.

## These docs

The docs are an [mdBook](https://rust-lang.github.io/mdBook/) in `docs/`. To see them as you write:

```sh
mdbook serve docs
```

They're published to [ghostwrite-dev.github.io/ghostwrite](https://ghostwrite-dev.github.io/ghostwrite/) on every push to `main` that changes them.

## Licence

Ghostwrite is under the [Mozilla Public License 2.0](https://mozilla.org/MPL/2.0/). By contributing, you agree that your contribution is under it too. The word motions in `crates/core/src/movement.rs` are ported from helix, under the same license.
