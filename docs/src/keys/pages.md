# Pages

The keys of a tab of a file's pages, which a plugin makes (Typst's, say), over the code's. `space v` (or `:preview`) goes there, opening the pages if they aren't shown yet, and again goes back to the code. On a file no plugin makes pages of, it says so in the status line; on a file that is its pages (a PDF, an image), it says that too. See [Pages and what's beside a file](../languages/pages.md).

| Keys | What they do | Commands |
| --- | --- | --- |
| `j` `down` `k` `up` | scroll down, up | `scroll_down`, `scroll_up` |
| `ctrl-d` `ctrl-u` | a screen down, up | `scroll_page_down`, `scroll_page_up` |
| `+` `-` `=` | zoom in, out; fit the view (⌘= ⌘- ⌘0) | `zoom_in`, `zoom_out`, `zoom_reset` |

The code's other keys work as ever, moving through the text the pages are made from. The cursor and the selection show on the pages.

A PDF or an image isn't this view: it's a tab of its own that's only pages, where `j` and `k` go from page to page as the code's keys.

The names in the last column are what you use to [remap a key](../configuration/keys.md), here under `"pages"`.
