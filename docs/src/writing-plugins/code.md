# A plugin with code

Code is Rust, built for WebAssembly, so one `.wasm` file runs on every system. It's a crate that depends on `ghostwrite-plugin-api`, implements the `Plugin` trait, and says so with `register!`.

## Setting up

```sh
rustup target add wasm32-wasip2
cargo new --lib word-count
```

`Cargo.toml`:

```toml
[package]
name = "word-count"
version = "0.1.0"
edition = "2024"

[lib]
crate-type = ["cdylib"]

[dependencies]
ghostwrite-plugin-api = { git = "https://github.com/ghostwrite-dev/ghostwrite", tag = "plugin-api-v1.0.1" }
wit-bindgen = "0.57"

[profile.release]
opt-level = "s"
lto = true
```

## A command

`src/lib.rs`:

```rust
use ghostwrite_plugin_api::*;

struct WordCount;

fn count(context: &Context) -> String {
    format!("{} words", context.text.split_whitespace().count())
}

impl Plugin for WordCount {
    fn run(name: String, _args: Vec<String>, context: Context) -> Reply {
        match name.as_str() {
            "word-count" => Ok(vec![Action::Say(count(&context))]),
            "shout" => {
                let (start, end) = (context.selection_start, context.selection_end);
                let selected = &context.text[start as usize..end as usize];
                Ok(vec![Action::Edit(Edit { start, end, text: selected.to_uppercase() })])
            }
            other => Err(format!("no command {other}")),
        }
    }

    fn on_event(_event: Event, context: Context) -> Reply {
        Ok(vec![Action::Say(count(&context))])
    }
}

register!(WordCount);
```

`plugin.json`, beside the built `.wasm`:

```json
{
  "name": "word-count",
  "version": "0.1.0",
  "api": 1,
  "code": "word_count.wasm",
  "commands": [
    { "name": "word-count", "what": "count the words in the file" },
    { "name": "shout", "what": "make what's selected uppercase" }
  ],
  "events": ["saved"]
}
```

Build it, and link the folder:

```sh
cargo build --release --target wasm32-wasip2
cp target/wasm32-wasip2/release/word_count.wasm .
```

```text
:plugin link .
:word-count
```

This is `examples/word-count` in the Ghostwrite repository.

## What a plugin is given

Each call gets a `Context`: the file as it is now.

| Field | What it is |
| --- | --- |
| `root` | the project's folder |
| `path` | the file's path, if it has one |
| `language` | its language, if it has one |
| `text` | its whole text |
| `cursor` | the cursor, in bytes into `text` |
| `selection_start`, `selection_end` | the selection, in bytes |

With no file open, there's no path and no text.

## What it answers

A command or an event answers with a `Reply`: a list of `Action`s, or an error for the status line.

| Action | What the editor does |
| --- | --- |
| `Action::Say(text)` | shows a line in the status bar |
| `Action::Edit(Edit { start, end, text })` | replaces `start..end` of the text it was shown. All of a reply's edits are about that same text, and are one undo step. If the text has changed since, they aren't made |
| `Action::Open(path)` | opens a file of the project, relative to its root |

## Commands

Each command the manifest lists under `"commands"` becomes a `:` command, with Tab completion. Names are letters, digits, `-` and `_`. `run` gets the command's name and its arguments, split on spaces. The editor's own commands come first, so a plugin can't replace one.

## Events

`"events"` lists what the plugin wants to hear of: `opened`, `saved` and `closed`. `on_event` is called with the event and the file.

## Highlighting by code

For a language no grammar parses well, the plugin can color it itself. Say `"highlights": true` (and no `grammar`), and implement `highlight`:

```rust
fn highlight(text: String) -> Vec<Span> {
    // Spans in order, not overlapping, in bytes: Span { start, end, scope }.
    vec![Span { start: 0, end: 4, scope: "keyword".into() }]
}
```

`scope` is one of the editor's [scopes](grammars.md#highlights). Lean's plugin does this, coloring by its tokens until Lean's own colors arrive.

## Calling back into the editor

Two functions go the other way, as far as the manifest's `"access"` lets them:

| Function | What it does |
| --- | --- |
| `fetch(url)` | download from a host `access.network` lists, over https |
| `which(program)` | where a program is, on the `PATH` it's been given (`access.env`) |

## Keeping state

Each call is a function of what it's given. What a plugin keeps from one call to the next it keeps in statics, and that's lost if the plugin is stopped (when a call takes too long, say). A panel beside a file has a better way: see [Beside a file](panels.md).

## Every function

The `Plugin` trait has a default for each, so implement only what the manifest says it does.

| Function | When it's called | Manifest |
| --- | --- | --- |
| `run` | a `:` command | `commands` |
| `on_event` | a file opened, saved or closed | `events` |
| `highlight` | coloring a file | `"highlights": true` |
| `open_pages`, `make_pages`, `draw` | making and drawing pages | `"pages": true`: see [Pages](pages.md) |
| `server_env`, `server_settings` | starting and configuring a server | `"configured_by_code": true`: see [Configuring servers](servers.md) |
| `panel_open`, `panel_answered`, `panel_notification` | the panel beside a file | `"panel"`: see [Beside a file](panels.md) |

`crates/plugin-api/wit/plugin.wit` in the Ghostwrite repository is the whole interface.
