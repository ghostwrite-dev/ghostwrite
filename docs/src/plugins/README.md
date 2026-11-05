# Plugins

Everything Ghostwrite knows about a kind of file comes from a plugin: languages, language servers, a file's pages in a tab, a panel beside a file (in the side view), key bindings, `:` commands, and rules for the model's prompt. A plugin's code is WebAssembly, so one build runs on every system, in a sandbox.

## Installing

Installing is two steps. First:

```text
:plugin install ghostwrite-dev/typst
```

fetches the plugin and checks it, and the status line says all it adds and can reach (the project, your fonts, the network, the servers it starts), ending with what to type next:

```text
typst (version 1.0.0; .typ files, highlighted; …). :plugin install! ghostwrite-dev/typst to install it
```

Nothing is installed yet. Then:

```text
:plugin install! ghostwrite-dev/typst
```

puts what was fetched in place. See [Permissions and the sandbox](permissions.md) for what the line can say.

A source is any of:

| Source | What it installs |
| --- | --- |
| `owner/repo` | the latest GitHub release of `github.com/owner/repo` |
| `owner/repo@1.2.0` | that version, the release tagged `v1.2.0` |
| `owner/repo/name` | the plugin called `name`, from a release that has several |
| `https://github.com/owner/repo` | the same as `owner/repo` (and with `@1.2.0` or `/name`) |
| a folder | a copy of the plugin in that folder on this machine |
| any git URL (`https://`, `ssh://`, `git@…`) | the repository as it is, cloned, with its built `.wasm` in it |

A release's zip is `<name>-<version>.zip` or `<name>.zip`, and it's checked against the `.sha256` beside it or the release's `SHA256SUMS`. If it has neither, the line says so before you install it. A GitHub repository with no release is cloned, as a git URL is. GitHub's API is asked without a token, or with `GITHUB_TOKEN` if it's set.

A plugin is live as soon as it's installed: no restart. Files open in a language that arrives or goes keep what they had until they're opened again. Rules a plugin adds for the model apply from the next start.

## Linking a folder

```text
:plugin link ~/code/my-plugin
```

`link` uses a plugin from its folder, in place, rather than copying it. Its `plugin.json` and `.wasm` files are looked at every second, and it's read again when they change. It's what you want while writing one: rebuild, and the editor picks it up.

## Updating and removing

| Command | What it does |
| --- | --- |
| `:plugin update` | fetch every plugin you installed again |
| `:plugin update name` | fetch one again: a release's latest |
| `:plugin remove name` | remove one; for a linked one, only the link, leaving its folder alone |
| `:plugin list` (or `:plugins`) | the plugins there are, what each adds, and what's wrong with any that couldn't be read |

Nothing updates by itself, and a linked plugin isn't updated: it's read where it is. An update is fetched and checked before it replaces what's there, so a failed one leaves the plugin as it was. An update with no checksum to check it against isn't taken; remove the plugin and install it again to take it anyway. Tab completes plugin names after `update` and `remove`.

## Where plugins live

- The plugins that come with the app are in a folder beside it (inside the app bundle on a Mac).
- Those you install are in `plugins/` in the config folder: `~/.config/ghostwrite/plugins/`, or `%APPDATA%\Ghostwrite\plugins\` on Windows.

A plugin you install with the same name as a bundled one is used in its place.

A plugin with no code can also be a single `.json` file dropped into `plugins/`. See [Writing plugins](../writing-plugins/README.md).

## When no plugin handles a file

When you open a file no plugin handles, and one in [the catalog](https://github.com/ghostwrite-dev/default-plugins/blob/main/catalog.json) would, the status line says what the catalog calls it and the command that installs it, such as `:plugin install ghostwrite-dev/typst`. That's once for each kind of file, each time Ghostwrite runs. The catalog is fetched in the background and kept for a day.

## When a plugin can't be read

A plugin whose manifest is wrong, or that was built for another version of the plugin interface, isn't loaded. The others still are. `:plugins` says what's wrong with it.
