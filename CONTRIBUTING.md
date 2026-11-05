# Contributing

Ghostwrite builds on macOS, Windows and Linux with Rust 1.97:
[Install](https://ghostwrite-dev.github.io/ghostwrite/install.html#from-source) says what
each needs. The full guide is the docs'
[Contributing](https://ghostwrite-dev.github.io/ghostwrite/contributing.html) page.

```sh
./check.sh      # formatting, clippy, the plugins, the tests: what CI runs on all three, and what has to pass
scripts/record-demo.sh  # records the README's GIF from docs/demo (macOS, with Screen Recording allowed)
```

A change comes with a test that fails without it. The model is never asked in a test:
stories are driven with scripted replies. Tests that need a language server that may not be
installed are `#[ignore]`d, and a test that only works on one system says so with `#[cfg(unix)]`.

The crates are layered: pure ones at the bottom (`core`, `project`, `terminal`, `page`, `lsp`,
`dap`, `plugin`, `plugin-api`, `wasm`, `model`, `ai`, `state`), and the gpui app, `app`, on
top. Each module starts with a comment saying what it is; `crates/app/src/editor/mod.rs` is
the way in to the editor.

Every language is a plugin, and nothing built in names one (a test keeps to that). A change
to what plugins can do goes in the interface (`crates/plugin-api/wit/plugin.wit`) or the
manifest (`crates/plugin/src/manifest.rs`). The plugins are in their own repositories:
[default-plugins](https://github.com/ghostwrite-dev/default-plugins),
[typst](https://github.com/ghostwrite-dev/typst) and [lean](https://github.com/ghostwrite-dev/lean).
`scripts/build-plugins.sh` downloads the releases `plugins.lock` and `test-plugins.lock` pin;
with `GHOSTWRITE_PLUGINS_SRC` set to a default-plugins checkout, it builds those from it instead.

The docs are an mdBook in `docs/` (`mdbook serve docs`), published on every push to `main`
that changes them.

Ghostwrite is under the Mozilla Public License 2.0 ([LICENSE](LICENSE)), and so is what you contribute.
