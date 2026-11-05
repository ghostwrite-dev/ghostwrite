# Building and releasing

## Building by hand

A plugin with code builds like any Rust crate, for WebAssembly:

```sh
rustup target add wasm32-wasip2
cargo build --release --target wasm32-wasip2
cp target/wasm32-wasip2/release/my_plugin.wasm .
```

The `.wasm` goes beside `plugin.json`, under the name its `"code"` says. A plugin with no code needs no build: its folder is the plugin.

## Building with plugin-build

`scripts/plugin-build.sh` in the editor's repository puts a plugin together into a folder the editor loads:

```sh
sh path/to/ghostwrite/scripts/plugin-build.sh [plugin folder] [out folder]
```

The plugin folder is the current one unless given, and the out folder is `target/plugin/<name>` in it. It:

1. builds the plugin's code, if it has a `Cargo.toml`: `cargo build --release --target wasm32-wasip2` (with `--locked` if there's a `Cargo.lock`), copied to the name `"code"` says;
2. downloads the grammars its `grammars.txt` lists, once, and checks each against its sha256;
3. copies the rest of its files as they are, but for `Cargo.toml`, `Cargo.lock`, `src`, `grammars.txt`, `target`, and `README*` and `LICENSE*`.

`CARGO_TARGET_DIR` says where the code is built (`target` in the plugin's folder by default), and `GRAMMARS_DIR` where grammars are kept (`~/.cache/ghostwrite/grammars` by default).

## Releasing with the plugin workflow

`:plugin install owner/repo` installs a plugin from its repository's GitHub releases. The editor's repository has a reusable workflow, `.github/workflows/plugin.yml`, that makes those releases with `plugin-build.sh`. Call it from your plugin's repository, in `.github/workflows/release.yml`:

```yaml
name: release

on:
  push:
    branches: [main]
    tags: ["v*"]
  pull_request:

permissions:
  contents: write

jobs:
  plugin:
    uses: ghostwrite-dev/ghostwrite/.github/workflows/plugin.yml@plugin-api-v1.0.1
```

It takes no inputs: it reads the name and version from `plugin.json`. It needs `contents: write`, which your workflow gives it, to create the release. Each time it's called (here, on every push to main, every pull request and every tag), it:

1. checks out your plugin, and the editor's `scripts/` at the commit the tag you call it at points to;
2. on a tag, checks the tag is `v` followed by `plugin.json`'s `"version"` (`v1.2.0` for `1.2.0`), and fails if not;
3. installs Rust 1.97.0 with the `wasm32-wasip2` target, if the plugin has a `Cargo.toml`;
4. builds it with `plugin-build.sh`;
5. zips it as `<name>-<version>.zip`, its files in a `<name>/` folder, with `<name>-<version>.zip.sha256` beside it, and uploads both as the run's artifact;
6. on a tag starting `v`, creates the GitHub release for that tag, titled `<name> <version>`, with the zip and its `.sha256`.

To release a version, set `"version"` in `plugin.json`, then tag it:

```sh
git tag v1.2.0
git push origin v1.2.0
```

## How people install it

| Command | What it installs |
| --- | --- |
| `:plugin install you/my-plugin` | the latest release |
| `:plugin install you/my-plugin@1.2.0` | that version |
| `:plugin install you/my-plugin/name` | one plugin from a release of several |

`:plugin update my-plugin` fetches the latest release again.

## Versions

- Your plugin's `version` is yours to number.
- `"api": 1` is the plugin interface's version, not yours. Plugin crates depend on `ghostwrite-plugin-api` at the tag `plugin-api-v1.0.1`, and call the workflow at the same tag, so one tag pins both the interface and the build. Any release of the editor with interface version 1 runs them.

## The bundled plugins

The plugins that come with the app live in [ghostwrite-dev/default-plugins](https://github.com/ghostwrite-dev/default-plugins), each in a folder of its own, and are built with the same `scripts/plugin-build.sh`. A crate's code is built for WebAssembly, the grammars its `grammars.txt` lists are downloaded once and checked against their sha256, and the rest of its files are copied as they are. See [Grammars and queries](grammars.md#fetching-grammars-instead-of-committing-them).

To bundle a new language with the app, add its folder to default-plugins, with a `grammars.txt` saying where its grammar is published and its sha256, instead of the `.wasm`. Once it's released, pin it in the editor's `plugins.lock`: see [Contributing](../contributing.md#plugins-in-the-build).
