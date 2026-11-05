# Search

A search is a regular expression. It ignores case unless it has a capital letter. While you type it, every match on screen is marked.

| Keys | What they do | Commands |
| --- | --- | --- |
| `/` `?` | search the file, forward, back (esc goes back) | `search`, `rsearch` |
| `n` `N` | next match, previous (select mode: add it) | `search_next`, `search_prev` |
| `*` `alt-*` | search for the selection (as a word; anywhere) | `search_selection_detect_word_bounds`, `search_selection` |

The names in the last column are what you use to [remap a key](../configuration/keys.md).
