# Testing

## By hand

Link the plugin's folder, and the editor reloads it whenever its files change:

```text
:plugin link ~/code/my-plugin
```

Rebuild, and try it. If it doesn't load, `:plugins` says why: a manifest field it doesn't accept, a file it can't find, code built for another interface version, or code or a grammar that can't be loaded.

## Unit tests

Most of a plugin is plain Rust: parsing, laying out, deciding what to show. Keep that in functions that don't touch the plugin interface, and test them with `cargo test` on your own machine, as for any crate. Only the thin layer that implements `Plugin` needs WebAssembly.

For a plugin with a panel, `panel_answered` and `panel_notification` take the state and the server's JSON and give back a `Panel`, so they can be tested with canned answers from the server, no server running.

## Pages

Zoomed in, the editor draws pages in pieces, and the pieces have to meet. `ghostwrite_page::check_regions` (from the `ghostwrite-page` crate in the Ghostwrite repository) checks that drawing a page in regions cut at odd places, one reaching past the page's edges, gives the same pixels, give or take antialiasing, as drawing the whole page:

```rust
ghostwrite_page::check_regions(&pages, 0, 800)?;
```

## In the editor's own tests

The Ghostwrite repository builds its plugins for WebAssembly and runs them in its tests (`crates/wasm/tests`): what they can do, what they're stopped from doing, and real grammars. A change to what plugins can do comes with a test there. See [Contributing](../contributing.md).
