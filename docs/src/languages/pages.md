# Pages and what's beside a file

Some plugins show a file as something other than text: its pages in a tab, or something in the side view beside it. Both are [views](../editing/modes.md#views).

## PDFs

A PDF opens as its pages, read-only, through the `pdf` plugin (which draws them with hayro). `j` and `k` go from page to page. There's no text to select. It's drawn again when the file changes, after a rebuild say.

The model can read a PDF with its usual `read`, and is shown the PDF itself if it can see them. What it says about one goes in the file it's made from, if there is one.

## Images

An image opens the same way, through the `image` plugin: PNG, JPEG, GIF (its first frame), WebP, BMP, TIFF or ICO. It's shown the right way up, and no bigger than it is until you zoom in. It's read-only, and drawn again when the file changes. The model reads one as it does a PDF.

## Zooming pages

Pages zoom on their own, whatever the editor's zoom:

- ⌘-scroll over them zooms around the pointer;
- ⌘=, ⌘- and ⌘0 zoom them while their tab is in front.

They go from a tenth of the view's width to four times it, and scroll across when they're wider than the view. Zoomed in, only the tiles on screen are drawn, as sharp as the screen, over the whole page drawn smaller, which shows until they're done.

## Typst

[Typst](https://typst.app) comes as a plugin you install on demand:

```text
:plugin install ghostwrite-dev/typst
```

Opening a `.typ` file without it suggests that command.

`space v` (or `:preview`) opens a Typst file's pages in a tab of their own, compiled as you type, saved or not, and from the pages goes back to the file. `space v` is the editor's own key, for any file with pages. `space w v` puts them beside the file instead, and they keep up as you type.

- The project's files and your system's fonts are used as they are. Packages come from Typst's cache, downloaded from packages.typst.org the first time.
- The cursor and selection show on the pages too, on the glyphs they're on. Clicking a glyph puts the cursor on the text it came from, ready for when you go back; a double click goes back.
- Moving around leaves the pages where they are. An edit that takes you to another line, or `space j`, brings them to the cursor.
- `$` closes itself as you type, like a bracket.

The first time, it takes some seconds to compile and read the fonts; after that it starts at once. Highlighting comes from the plugin itself, and the language server is `tinymist`.

## Lean

[Lean](https://lean-lang.org) also comes as a plugin you install on demand:

```text
:plugin install ghostwrite-dev/lean
```

A Lean file shows its goals in the side view, right of the code, as Lean's infoview does:

- the proof state at the cursor, or the type expected there;
- the messages on the cursor's line, `#eval`'s output among them;
- how far Lean has got checking the file.

`space g` goes to the side view, where `j` and `k` scroll it, and again back to the code. `:preview` shows or hides it.

Lean runs through `lake serve`, started in the file's Lake project, so the project's toolchain and packages are used.

A backslash types a symbol, as in Lean's VS Code extension and with its abbreviations: `\to` is `→`, `\N` is `ℕ`, `\<>` is `⟨⟩` around the cursor. An abbreviation turns into its symbol as soon as no longer one starts with it, or when you type a space or another char that can't go on to one. Tab finishes it alone, and `\\` is a backslash. The abbreviation being typed is underlined.

Lean has no grammar that parses it well (its syntax grows with every `notation`), so the plugin colors it by its tokens until Lean's own colors arrive.
