# Pages

A plugin can show a file as pages, in a view of their own, a tab: a typeset document from its source, or a file that isn't text at all. The view's `key` goes there and back; see [Views](manifest.md#views).

```json
{
  "name": "typst",
  "api": 1,
  "code": "typst.wasm",
  "views": [{ "name": "pages", "title": "Pages", "shows": "pages", "place": "tab", "key": "space v" }],
  "extensions": ["typ"],
  "access": { "project": true, "fonts": true }
}
```

For a file that isn't text (a PDF, an image), add `"only_pages": true`: it opens as its pages (its view needs no key: the file's tab is its pages), read-only, and the plugin reads the file itself each time they're made, so it needs `"access": { "project": true }`. `"pages_at_own_size": true` keeps pages from being shown bigger than they are, as for images.

## The three calls

```rust
fn open_pages(path: String, utc_offset_minutes: i32) -> Result<(), String>;
fn make_pages(text: String) -> Result<Vec<Page>, String>;
fn draw(key_high: u64, key_low: u64, region: Region) -> Option<Drawing>;
```

1. **`open_pages`** gets ready to make pages of the file at `path`, in the project (mounted at `/project` if `access.project` says so). Local time is `utc_offset_minutes` from UTC, for a document that prints the date.
2. **`make_pages`** lays out pages from the file's text as it is now, saved or not. It's called again as the text changes, in the background.
3. **`draw`** draws part of one page.

A `Page` is its size in points, a 128-bit key (`key_high`, `key_low`) and its glyphs:

| Field | What it is |
| --- | --- |
| `width`, `height` | the page's size, in points |
| `key_high`, `key_low` | what's on it: the same key, the same drawing. A page whose key hasn't changed isn't drawn again |
| `glyphs` | each glyph's box, in points from the page's top left, and the bytes of the text it came from, if it did |

The glyphs are how the cursor and the selection show on the pages, and how a click on a page puts the cursor in the text.

## Drawing

`draw` is one call for any region of a page at any scale. A `Region` is the page drawn `scale` pixels to a point, and of that, `width` by `height` pixels from `x`, `y`. The whole page is the region from 0, 0 as big as the page at that scale; zoomed in, the editor asks for the tiles on screen.

It answers a `Drawing` exactly as wide and as high as the region, a row at a time, four bytes a pixel: blue, green, red and alpha, premultiplied. What's past the page's edge is see-through. `draw` can be asked about the last pages made or the ones before.

`ghostwrite_page::check_regions` tests that a plugin's pieces meet as its whole pages do: see [Testing](testing.md).

## Keys

Pages open with the view's `key` (or `:preview`, or the user's `focus_view pages`). Keys for the view itself go under `"in"` and its name:

```json
"keys": [{ "in": "pages", "keys": "space n", "command": "scroll_page_down", "what": "the next page" }]
```
