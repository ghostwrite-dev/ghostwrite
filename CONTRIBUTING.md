# Contributing

Ghostwrite builds on macOS, Windows and Linux: the README says what each needs.

```sh
./check.sh      # formatting, clippy and the tests: what CI runs on all three, and what has to pass
```

What's particular to a system goes in `crates/project/src/platform.rs` if it can, behind `cfg`
where it can't, and a test that only works on one says so with `#[cfg(unix)]`.

A change comes with a test that fails without it. Tests that need a language server
that may not be installed are `#[ignore]`d.

Every language is a plugin, and nothing built in names one (a test keeps to that): see
`crates/plugin/src/manifest.rs` for what a plugin can say, `plugins/rust` for a language that's
no code at all, `plugins/python` for one whose code configures its servers, `plugins/lean` for one
that colors itself, types symbols from abbreviations and has a panel beside its files, and
`plugins/typst` for one that makes pages. The README's "Adding a language" says how to add one.
Nothing is built in, so a change to what plugins can do goes in the plugin interface
(`crates/plugin-api/wit/plugin.wit`) or the manifest, and every plugin is rebuilt with
`scripts/build-plugins.sh`.
