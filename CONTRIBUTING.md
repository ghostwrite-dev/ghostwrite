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
`crates/plugin/src/manifest.rs` for what a plugin can say. The plugins themselves are in their own
repositories: [default-plugins](https://github.com/ghostwrite-dev/default-plugins) (those the app
comes with: `rust` is a language that's no code at all, `python` one whose code configures its
servers), [lean](https://github.com/ghostwrite-dev/lean) (one that colors itself, types symbols
from abbreviations and has a panel beside its files) and [typst](https://github.com/ghostwrite-dev/typst)
(one that makes pages). `scripts/build-plugins.sh` downloads the releases `plugins.lock` and
`test-plugins.lock` pin; with `GHOSTWRITE_PLUGINS_SRC` set to a default-plugins checkout, it builds
those from it instead. Nothing is built in, so a change to what plugins can do goes in the plugin
interface (`crates/plugin-api/wit/plugin.wit`) or the manifest; the plugins build against it by its
tag, `plugin-api-v<version>`, and are released again, and the lock files pinned to the new releases.

Ghostwrite is under the Mozilla Public License 2.0 ([LICENSE](LICENSE)), and so is what you contribute.
