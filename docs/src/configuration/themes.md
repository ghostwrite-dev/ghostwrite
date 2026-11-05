# Themes

Ghostwrite comes with Gruvbox, light and dark, and follows the system's appearance.

To use another theme, put its file in `themes/` beside the config (`~/.config/ghostwrite/themes/`), and name it in `config.json`:

```json
{
  "theme_dark": "One Dark",
  "theme_light": "One Light"
}
```

Theme files are JSON in [Zed's theme format](https://zed.dev/docs/themes), so Zed's themes work as they are. A file can hold several themes (a family); Ghostwrite looks through every file in the folder for one whose `name` matches.

If a theme isn't found, the bundled Gruvbox is used for that appearance.

The theme also colors the terminal: its sixteen colors and its defaults, and what programs are told when they ask.
