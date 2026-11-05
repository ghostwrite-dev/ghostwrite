# Beside the file

The keys of a plugin's view of blocks: what it shows beside a file (Lean's goals, say), over the code's. The view's key (`space g` for Lean's) goes there, and again back to the code; a click goes there too. The mode line calls it by its title: `NOR · goals`. On a file of another language, the key says whose view it is. See [Pages and what's beside a file](../languages/pages.md).

| Keys | What they do | Commands |
| --- | --- | --- |
| `j` `down` `k` `up` | scroll down, up | `scroll_down`, `scroll_up` |
| `ctrl-d` `ctrl-u` | a screen down, up | `scroll_page_down`, `scroll_page_up` |
| `esc` | back to the code | `focus_code` |

It has no text, so the code's motions do nothing here. `:` and most `space` keys work as anywhere.

The names in the last column are what you use to [remap a key](../configuration/keys.md), here under the view's name: `"goals"` for Lean's. In `space ?`, the section is called by the view's title and key: **Goals  space g**.

## The debugger

While something's being debugged, the debugger is a view of its own beside the code, on top of a plugin's panel if there is one: where the program is stopped, its variables, and what it printed. `space x` goes there, and again back to the code; a click goes there too. The mode line says `NOR · debugger`. With nothing being debugged, `space x` says so.

| Keys | What they do | Commands |
| --- | --- | --- |
| `j` `down` `k` `up` | scroll down, up | `scroll_down`, `scroll_up` |
| `ctrl-d` `ctrl-u` | a screen down, up | `scroll_page_down`, `scroll_page_up` |
| `esc` | back to the code (`space x` too) | `focus_code` |

Remap them under `"debugger"`.
