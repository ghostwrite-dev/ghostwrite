# Beside the file

The keys of what a plugin shows beside a file (Lean's goals, say), over the code's. `space g` goes there, and again back to the code; a click goes there too. The mode line calls it by its title, or `debugger` while a session runs. The debugger shows here whenever a session runs, on top of a plugin's panel if there is one, even when that panel is hidden. With nothing to show, `space g` says so in the status line. See [Pages and what's beside a file](../languages/pages.md).

| Keys | What they do | Commands |
| --- | --- | --- |
| `j` `down` `k` `up` | scroll down, up | `scroll_down`, `scroll_up` |
| `ctrl-d` `ctrl-u` | a screen down, up | `scroll_page_down`, `scroll_page_up` |
| `esc` | back to the code | `focus_code` |

It has no text, so the code's motions do nothing here. `:` and most `space` keys work as anywhere.

The names in the last column are what you use to [remap a key](../configuration/keys.md), here under `"aside"`.
