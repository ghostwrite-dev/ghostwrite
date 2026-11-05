# Configuration

Settings are in `config.json`, read once when Ghostwrite starts:

| System | File |
| --- | --- |
| macOS | `~/.config/ghostwrite/config.json` |
| Linux | `~/.config/ghostwrite/config.json`, or `$XDG_CONFIG_HOME/ghostwrite/config.json` |
| Windows | `%APPDATA%\Ghostwrite\config.json` |

Every key is optional. A missing or broken file, or a missing key, means the default. Restart Ghostwrite after changing it.

## Every key

| Key | Default | What it does |
| --- | --- | --- |
| `tab_width` | `4` | columns a tab takes |
| `font_size` | `13` | the code's size in points, before zooming (6 to 48) |
| `wrap_column` | `100` | lines wrap at this column, or sooner if the window is narrower |
| `margin_lines` | `3` | lines kept between the cursor and the edge when scrolling to it |
| `max_file_size_mb` | `20` | files bigger than this aren't opened, so a stray log or binary can't hang the editor |
| `format_on_save` | `true` | format a file with its language server when it's saved |
| `animations` | `true` | the light that leads your eye to a story's next step, and the view gliding when it moves far |
| `max_recent_projects` | `20` | how many projects the projects page remembers |
| `model` | `"claude-opus-5-5"` | the Claude model asked through the API or Claude Code |
| `local_model` | none | a [local model](../models/local.md) to offer, and ask first unless you've chosen another with `:model`: Hugging Face's `user/repo:quantization`, or a `.gguf` file |
| `context_bytes` | `200000` | bytes of the file around the marked lines sent to the model |
| `max_diff` | `12000` | bytes of uncommitted changes sent to the model |
| `debugger` | `["/usr/bin/xcrun", "lldb-dap"]` on macOS, `["lldb-dap"]` elsewhere | the debug adapter `:debug` runs: the program and its arguments |
| `theme_light` | Gruvbox Light | the theme for light appearance, by name: see [Themes](themes.md) |
| `theme_dark` | Gruvbox Dark | the theme for dark appearance |
| `keys` | none | your own keys: see [Your own keys](../configuration/keys.md) |

`tab_width` and `wrap_column` must be more than 0. `context_bytes` and `max_diff` don't apply to local models, which are sent a small share of each part instead.

## An example

```json
{
  "font_size": 14,
  "wrap_column": 120,
  "format_on_save": false,
  "model": "claude-sonnet-5-5",
  "theme_dark": "One Dark",
  "theme_light": "One Light",
  "keys": {
    "ctrl-s": ":w",
    "space W": ":wa"
  }
}
```

## Environment variables

| Variable | What it does |
| --- | --- |
| `GHOSTWRITE_AI_EFFORT` | how hard Claude thinks: `low` (the default), `medium`, `high`, `xhigh`, `max`. See [Effort](../models/README.md#effort) |
| `ANTHROPIC_API_KEY` | an API key, used before the one `:api-key` saved |

## Beside the config

The folder `config.json` is in also holds:

- `themes/`: your [themes](themes.md);
- `plugins/`: the [plugins](../plugins/README.md) you've installed.

What Ghostwrite remembers between launches (recent projects, open files, the command history, pane sizes, the model's notes) is kept elsewhere, in its support folder: `~/Library/Application Support/Ghostwrite` on macOS, `%APPDATA%\Ghostwrite` on Windows, and `~/.local/share/ghostwrite` on Linux.
