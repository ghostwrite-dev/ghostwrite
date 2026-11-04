# Ghostwrite

A helix-style modal editor for macOS, Windows and Linux, built on [gpui](https://www.gpui.rs),
with Claude in it. Point at some lines (`space a`) and say what you want:
a question is answered with notes pinned to the code, across files; a change
comes as small whole-line steps you accept with Tab, one at a time, like
autocomplete, and commands to run in your terminal (a build, the tests); one
you don't want, you redirect. Nothing changes until you accept it, and
anything you say mid-review (a question, a correction) is a follow-up that
can revise what comes next. While a story is up you can look around freely (move,
open files with `space f` or `space b`, switch tabs, scroll): a step that comes
meanwhile waits in the corner rather than pulling you away, and the first Tab
brings you back to it; the next one accepts.

## Install

```sh
./install.sh                    # macOS: installs /Applications/Ghostwrite.app
scripts/install-linux.sh        # Linux: installs ~/.local/bin/ghostwrite and a desktop entry
scripts/install-windows.ps1     # Windows: installs for this user, with a Start menu entry
```

Building needs Rust; on macOS the Metal toolchain, on Linux the packages named at the top of
its script, and on Windows the MSVC toolchain. `--no-default-features --features lean,wasm-plugins`
builds without local models, and without needing `cmake` and a C++ compiler for llama.cpp.

**How far each system has come.** macOS is what it's used on every day. Windows and Linux
have only been type-checked and tested in CI so far, so expect rough edges. They differ in that:

- the app's keys are Ctrl+Shift where a Mac has ⌘ (Ctrl+Shift+T for ⌘T), and Ctrl+Alt+Shift
  where it has ⌘⇧, since plain Ctrl belongs to the editor and the shell; `space ?` shows them as they are;
- a local model runs on the CPU, so it's slower: Metal is the only GPU it uses so far;
- the terminal is PowerShell on Windows;
- settings are in `%APPDATA%\Ghostwrite` on Windows and `~/.config/ghostwrite` elsewhere, which
  is what `~/.config/ghostwrite` below means.

Claude is reached with an API key, kept where the system keeps secrets with `:api-key ‹key›`
(or `ANTHROPIC_API_KEY`, if it's set), else through Claude Code if it's installed,
under your login.

A local model can be asked instead of Claude. llama.cpp is built in (building
Ghostwrite needs `cmake`), so there's nothing else to install or start: name a
model on Hugging Face and its quantization, or a `.gguf` file. It's downloaded
the first time, and loaded as you start typing a request.

```json
{ "local_model": "bartowski/Qwen2.5-Coder-14B-Instruct-GGUF:Q3_K_M" }
```

It runs in a process of its own, on the GPU, with an 8-bit KV cache and a small
context: every reply is one small step, so it's sent only what that step needs
(`context_bytes` and `max_diff` are Claude's), and earlier steps shrink to a line each. That process ends with the app, when `:model`
switches away, and after 5 idle minutes, so its memory is only held while it's used.

That memory can't be paged out, so a model only loads if it fits, there's only
one at a time across every running Ghostwrite, and if memory runs out later (to
other GPU-heavy work, say) the model stops, before the Mac panics.

`:model opus-5.5` or `:model qwen3:4b` chooses what's asked, with suggestions as
you type and Tab to complete, among Claude's models and every local one on this
Mac, whether Ghostwrite, Hugging Face's tools or Ollama downloaded it. The choice is remembered. Before there is one, a `local_model` in
the config is asked first, and otherwise Claude. Every model works the same way, however small: the same prompt, and
looking around with `<tool>` blocks that Ghostwrite runs (`read`, `list`,
`search`, `git`), so that needs no accepting. A local model also has a grammar:
it can't write anything but a well-formed small item or block, nor name a file
that isn't there.

## Using it

The keys are helix's, as helix has them; `space ?` shows them all. Ghostwrite's
own are under `space`, and ⌘ is for the app: tabs, windows, files. As in
helix, brackets and quotes close themselves while you type (and `$` in Typst), and the
bracket matching the one at the cursor is marked.

| Keys | What it does |
| --- | --- |
| `space a` | ask about the line or selection; Tab accepts, Enter (or `space a`) says what you want instead, Esc stops |
| `space e`, `⌘B` | the file tree (`a` `r` `d`: add, rename, trash); Claude's file operations are proposed there |
| `space t`, `⌘J` | terminal mode: your shell, under the code, has the keys; Esc Esc reads it instead (below), `⌘J` hides it |
| `/`, `?`, `n`, `N`, `*` | search the file (as you type, every match marked); the next and previous match, whichever way the search went (`3n`: three on; in select mode, as another selection); `*` makes the selection the search, as a whole word (`alt-*`: anywhere), and `n` finds it |
| `space f`, `space b` | pick a file of the project, or an open one, by typing a few of its letters |
| `.`, `Q`/`q`, `"a` | repeat the last change; record and play a macro; use a named register (`"+` is the clipboard) |
| `J`, `alt-J`, `ctrl-a`/`ctrl-x`, `ctrl-c` | join lines, a comment's leader taken off the lines joined (`alt-J` selects the spaces put in); a number up or down; comment lines out and back in |
| `ctrl-o`, `ctrl-i` | back to where a jump (`gd`, `10G`, `ge`, a search, another file) came from, and forward again (`3ctrl-o`: three back) |
| `i` `a`, `o` `O`, `c` | as in helix: the selection stays while you type, and after `a` takes in what's typed; `3o` opens three lines with a cursor each, indented, a line comment going on; `c` on whole lines types on a line of their own; `alt-c`, `alt-d` don't yank |
| `f` `t` `F` `T`, `alt-.` | find a char, over lines too (Enter: a line's end, Tab: a tab); `alt-.` finds again (`mi`, `]p` too) |
| `]p` `[p`, `] space` `[ space`, `g.` | the next and previous paragraph; blank lines below and above; where the last change was |
| `alt-o` `alt-i`, `alt-n` `alt-p`, `alt-a` `alt-I`, `alt-e` `alt-b` | by the syntax tree, as in helix: out to the node around the selection and back in; the next and previous node beside it; a cursor on each node beside it, or inside it; to the end or start of the node it's in |
| `maf` `mif`, `]f` `[f` | a function, all of it or inside; the next and previous one. Likewise `t` a type, `a` an argument, `c` a comment, `T` a test, `e` an entry, from helix's queries (in `crates/core/queries`) |
| `space /`, `:grep` | search the project and go through the matches |
| `space k`, `space d` | docs for what's under the cursor; go through the file's diagnostics, one note at a time |
| `gr`, `space r`, `space .`, `space s` | the language server: go through the references, rename everywhere, its code actions here, pick one of the file's symbols |
| `:model opus-5.5` | choose what's asked, Tab completing: Claude's models, and every local one |
| `:format` | format the file with its language server (saving does too, unless `format_on_save` is off) |
| `space j` | the next error (round to the first), or with none, the next warning; Lean's unsolved goals and `sorry`s among them, and a Typst file's pages follow |
| click | moves the cursor there; shift-click, or a click after `v`, selects to it; a double click is the word |
| `⌘N` `⌘W`, `⌘⇧[` `⌘⇧]` | new and close tab, previous and next tab |
| `gN` `gP` (`4gN`: four places), drag a tab, `⌘⌥→` `⌘⌥←`, `:tab-right` `:tab-left` | move a tab along the tab bar |
| `⌘O`, `⌘⇧O`, `⌘⇧N` | open a file, a folder; a new window (each project is open in one window); File › Open Recent has the projects opened lately |
| `:clear` | start over: the story up ends, and Claude forgets its notes on the project and your recent requests, so the next story begins with nothing remembered |
| `:timings` | how long the model has kept you waiting lately: until the first of its reply, and until there was something to look at |
| `⌘=`, `⌘-`, `⌘0` | zoom the window in, out, and back to 100% |

Claude reads the terminal: what's on it (and some scrollback) goes with
each request, and again in a follow-up when it's changed. It types into it
only what you accept: a command it proposes (to build, test, install) waits in
the terminal as a step does in a file, grey at the prompt with its note above and
its keys below, and Tab sets it going and goes straight on to the next step (or, if
it was the last, ends the story): nothing waits for it to finish. It can
ask for a new tab of the terminal to run it in, or to press Ctrl-C, which Tab does just the same. A command runs as you'd type it, for all of its output:
what a model adds to read less of it (`| tail`, `| grep`, `2>&1`, `; echo $?`) is taken off.

Esc Esc in the terminal doesn't leave it: the keys read it instead, in normal
mode, as a file that can't be edited. Its scrollback and screen are the text: the
motions, select mode, `x`, `/` and `n`, `y` all work as in a file, and the selection
shows on the terminal. What's printed meanwhile comes in under it, the selection staying
on its text. Enter (or `gf`, which works in files too) opens the file a line names, at
its line and column (`src/main.rs:12:5`), and `space a` asks about what's selected. `i`,
`a` or `o` go back to typing on the shell's line, and Esc goes back to the code. An Esc
on its own still reaches the shell, a moment later; two in a row never do. A story's
command is reviewed the same way, in AI mode: look through what's on the terminal with
the motions, and Tab runs it; if that was the last thing to do, the keys stay on the
terminal in normal mode, to read what it prints.

Hold ⌘ over a web address or a file the terminal printed (`src/main.rs:12:5`) and it's
underlined: ⌘-click opens the address in your browser, or the file in the editor at that
line.

The terminal has tabs too, a shell in each. In terminal mode the Mac's tab keys
are about them: `⌘T` another, `⌘W` closes it, `⌘⇧[` and `⌘⇧]` go through them,
and `⌘⌥←` and `⌘⌥→` (or dragging) move them. The model reads the one in front. To look around the project it writes
`<tool>` blocks, which Ghostwrite runs in the background.

`:preview` (or `space v`) opens a Typst file's pages in a tab of their own, as a PDF has, compiled
as you type, saved or not, and goes back to the file from there. Typst is built in:
the project's files and fonts are used as they are, and packages come from
Typst's cache, downloaded the first time. The cursor and the selection show
on the pages too, on the glyphs they're on, and clicking a glyph puts the
cursor on the text it came from, ready for when you go back to the file; a double click goes back to it. Moving around leaves the pages be; an edit
that takes you to another line, or `space j`, brings them to the cursor.

Pages zoom on their own, whatever the editor's zoom: `⌘`-scroll over them (around the pointer), or
`⌘=` `⌘-` `⌘0` while their tab is in front, from a tenth of the view's width to four times it. Wider
than the view, they scroll across too. Zoomed in, only the tiles on screen are drawn, as sharp as
the screen, over the whole page drawn smaller, which shows until they're done.

Colors come from the grammar, and from the language server over them where
it has its own (semantic tokens): it knows what each name is, a local, a
type, a constant. While you type, its last colors stay, moved past the
edit, until it sends new ones. Lean has no grammar that parses it well (its
syntax grows with every `notation`), so it's colored by its tokens until
Lean's own colors arrive.

A Lean file shows its goals beside it, as Lean's infoview does: the proof
state at the cursor (or the type expected there), the messages on the
cursor's line (`#eval`'s output among them), and how far Lean has got
checking the file (`:preview` hides them too). Lean runs through `lake
serve`, started in the file's Lake project, so the project's toolchain and
packages are used. A backslash types a symbol, as in Lean's VS Code
extension and with its abbreviations: `\to` is `→`, `\N` is `ℕ`, `\<>` is
`⟨⟩` around the cursor. An abbreviation turns into its symbol as soon as no
longer one starts with it, or when you type a space or another char that
can't go on to one (Tab finishes it alone); `\\` is a backslash.

A PDF opens as its pages, read-only, by way of a plugin (`plugins/pdf`, drawing them with hayro):
`j` and `k` go from page to page. There's no text to select in one. A model can read one with its usual `read`, and is shown
the PDF itself, if it can see them (Claude can; local models are told they can't); what it says
about it goes in the file it's made from. It's drawn again when the file changes, a rebuild say.

An image opens the same way (`plugins/image`): PNG, JPEG, GIF (its first frame), WebP, BMP, TIFF or
ICO, the way up the file says, and shown no bigger than it is until you zoom in. It's read-only, and it's drawn again
when the file changes. A model reads one with its usual `read`, the same as a PDF: what's too big, or in a format it doesn't
take, is made into something it can be shown first.

The tree and open files follow changes other programs make, as they're made. In a
file with unsaved changes, theirs come in alongside yours when they're to other
lines; where both changed the same lines, yours are kept, the status line says it
changed on disk, and saving won't write over theirs until `:w!`.

## Settings

`~/.config/ghostwrite/config.json`, every key optional:

```json
{
  "model": "claude-opus-5-5",
  "tab_width": 4,
  "font_size": 13,
  "margin_lines": 3,
  "max_file_size_mb": 20,
  "context_bytes": 200000,
  "max_diff": 12000,
  "max_recent_projects": 20,
  "format_on_save": true,
  "local_model": null
}
```

`GHOSTWRITE_AI_EFFORT` (`low` by default, up to `max`) sets how hard Claude thinks.

## Code

Layered like helix and zed: pure crates at the bottom, the gpui app on top,
and nothing below `app` knows about the UI.

| Crate | What's in it |
| --- | --- |
| `core` | text, selections, motions, text objects, the modal keymap, the languages (what's particular about each), syntax, abbreviations |
| `project` | files, search, the file tree model, shell runs and their stops, watching the disk, saved state |
| `terminal` | the user's shell on a pseudo-terminal, emulated by alacritty: its screen, to draw and to read |
| `page` | what a document's pages are, whoever lays them out: their sizes, and pages drawn |
| `lsp` | the language-server client |
| `plugin` | what a plugin can add, the plugins there are, and installing them |
| `plugin-api` | what a plugin written in Rust compiles against, and the interface itself |
| `wasm` | runs plugins compiled to WebAssembly, sandboxed, and their grammars |
| `lean` | a plugin: Lean's language, `lake serve`, its abbreviations, and its goals beside a file |
| `model` | a local model: llama.cpp, in a process of its own, and the memory it may use |
| `ai` | the model: the reply format, prompts, backends, the review story, placing steps, its notes kept per project |
| `view` | the editor's state, without drawing: documents, tabs, the project, search, registers, jumps, `:` commands, and the story under review |
| `app` | the application (binary `ghostwrite`) |

The editor knows no file type in particular. What's particular to one is a plugin:
a crate implementing `ghostwrite_plugin::Plugin`, which can add languages, language
servers, a `Renderer` (a file's pages in a tab), a `Panel` (something beside a file,
from its server), key bindings (a key sequence running a `:`
command, shown under "Plugin keys" in `space ?`), and rules for the prompt.
`crates/app/src/plugins.rs` is the one place that lists them, each behind a cargo
feature, and a test keeps every other crate from naming them.

Each module starts with a comment saying what it is; `crates/app/src/editor/mod.rs`
is the way in to the editor.

```sh
./check.sh                                  # formatting, clippy, and the tests: run before committing
cargo test -p ghostwrite-ai -- --ignored     # live checks against Claude Code
```

## Where to look

When a story moves on to a step, on other lines, in another file, the file tree or the terminal, a
light stretches from where it was to where it is now, leading edge first, and settles there, so your eye
follows it. While the model is thinking it isn't on any line: ✦ and what it's doing show in the
top right of the code, under what you asked. The step's lines light up for a moment once it's
accepted, and when the keys move the view more than a couple of lines it glides there, the cursor
sliding after (the wheel and the trackpad scroll as they always have). `"animations": false` in
the config turns all of this off.

So that it's ready by the time you press Enter, Claude Code is started as you open the prompt
(and, during a review, started on the conversation so far).

The model reads the project as it likes, and nothing else unless you let it: when it would read a
file or folder elsewhere (a config file, another project), the top right asks, `⇥` lets it (that
file, or everything in that folder, for the rest of the story) and `⏎` doesn't.

## Debugging

`:break` puts a breakpoint on the cursor's line, and `:debug path/to/program args` runs it in
the debugger: Xcode's `lldb-dap`, or what `"debugger"` in the config says. When it stops, the
cursor goes there, and the calls, variables and what it printed show under the code. `:over`,
`:step`, `:finish`, `:continue` and `:stop` go on from there, and can be given keys of your own.

## Plugins

`:plugin install ‹source›` installs a plugin from a folder, a git URL, or GitHub's `owner/repo`,
says all it adds, and it's there at once. `:plugin update [name]` fetches it again (nothing
updates by itself), `:plugin remove ‹name›` takes it out, and `:plugins` lists them and says what's
wrong with any that couldn't be read. They live in `plugins/` in the config folder. Files open
when a language arrives or goes keep what they had until they're opened again.

A plugin's code can't reach files, the network or the environment unless its manifest asks: it
sees the file it's asked about, and answers with what it would like done, which the editor does
or says why not. It's stopped if it takes over 5 seconds or 256 MB (or what it asks for instead),
and starts afresh next time. Installing says all it asks for. That doesn't cover a language server
a plugin names: that's a program that runs as you do, which is why installing says what it
starts. Nor is a grammar stopped if it runs away: it can only hold up coloring.

Typst, PDFs and images come with the app as such plugins (`plugins/typst`, `plugins/pdf`, `plugins/image`). Typst reads the project and the system's
fonts, keeps Typst's packages in Typst's own folders, and downloads from packages.typst.org and
nowhere else. `scripts/build-plugins.sh` builds it. The first time, it takes some seconds to
compile and to read the fonts; after that it starts at once.

### Writing one

A plugin is a folder with a `plugin.json`, and it needs only the parts it uses. Without code, a
single `.json` file in `plugins/` will do:

```json
{ "name": "zig", "extensions": ["zig"], "comment": "//", "server": ["zls"],
  "keys": [{ "keys": "space z", "command": "term", "what": "the terminal" }],
  "rules": "- Keep functions small." }
```

Code is Rust, built for WebAssembly, so the one file runs on every system (`examples/word-count`):

```rust
use ghostwrite_plugin_api::*;

struct WordCount;

impl Plugin for WordCount {
    fn run(_name: String, _args: Vec<String>, context: Context) -> Reply {
        Ok(vec![Action::Say(format!("{} words", context.text.split_whitespace().count()))])
    }
}

register!(WordCount);
```

```json
{ "name": "word-count", "version": "0.1.0", "api": 1, "code": "word_count.wasm",
  "commands": [{ "name": "word-count", "what": "count the words in the file" }],
  "events": ["saved"] }
```

```sh
rustup target add wasm32-wasip2
cargo build --release --target wasm32-wasip2
cp target/wasm32-wasip2/release/word_count.wasm .
```

Then `:plugin install` that folder. To install from git, commit the `.wasm`. A plugin can say
something, edit the text it was shown (one undo step; not if the text has changed since), and
open a file of the project. It can hear `opened`, `saved` and `closed`. The editor's own commands
can't be replaced. `crates/plugin-api/wit/plugin.wit` is the whole interface, and `"api"` is its
version: a plugin built for another version is refused, and says so.

For highlighting, add a grammar and a query. A grammar is what `tree-sitter build --wasm` makes
(tree-sitter 0.22 or later: older ones aren't read), and many projects publish theirs:

```json
{ "name": "json5", "api": 1, "extensions": ["json5"],
  "grammar": { "name": "json5", "wasm": "json5.wasm", "query": "highlights.scm" } }
```

A plugin can color files itself instead (`"highlights": true`), and make a file's pages for a tab
of their own (`"pages": true`), as Typst's does. Pages are drawn by one call, `draw`, for any
region of a page at any scale: the whole page is the region as big as it, and zoomed in the
editor asks for the tiles on screen. `ghostwrite_page::check_regions` tests that a plugin's pieces
meet as its whole pages do. Asking for more is under `"access"`, in
`crates/plugin/src/manifest.rs`. A file type that isn't text says `"only_pages": true`, as PDFs do. Panels still take a crate built into the editor, as Lean's does.

## Two panes

`ctrl-w v` or `:vsplit` opens a second pane beside this one, with this file's pages if
they're open, else the next tab. `ctrl-w w` or a click moves the keys over, and `ctrl-w q` or
`:only` goes back to one. Each pane shows its own tab: going to a tab that's in the other
pane moves there. A preview beside its file keeps up as you type, and clicking it moves the
cursor in the file without taking the keys away.

## More than one cursor

`C` copies each selection to the next line below it's as long as (`alt-C` above; `3C` three
times), `s` asks for a regex and puts one on each match in the selection (or in the file),
`alt-s` puts one on each line, and `,` (or Esc in normal mode) goes back to one; `alt-,` drops
the main one. `(` and `)` make the one before or after the main one, `alt-minus` merges them
all (`alt-_`: those that touch), and `&` lines them up in columns. Keys and typing happen at
all of them, as one undo.

## Your own keys and theme

In `~/.config/ghostwrite/config.json`, `keys` binds keys in normal and select mode. A key
can run a `:` command (it starts with `:`), or one of the editor's own commands by name, as
helix names them where helix has it (`extend_line_below`, `goto_definition`, `move_char_left`;
`space ?` shows what each key does). `no_op` unbinds a key. Keys pressed one after another are
written apart (`space w`, `g x`), and a key can lead on to keys of your own (`Z Z`). Yours come
before a plugin's, and `space ?` shows them:

```json
{
  "keys": { "ctrl-s": ":w", "space w": ":wa", "Z": "extend_line_below", "x": "no_op" },
  "theme_dark": "One Dark",
  "theme_light": "One Light"
}
```

A command name that isn't one is said when the editor starts, and that key stays as it was.
The commands and the default keys are in `crates/core/src/modal/command.rs` and `keymap.rs`.

Themes are found by name in `~/.config/ghostwrite/themes/`, as JSON files in Zed's format.

A project can say how it wants things done in `AGENTS.md`, `CLAUDE.md` or `.ghostwrite.md`
at its root, and the model is told. Unsaved changes are copied every couple of seconds, and
come back after a crash.

## License

MIT or Apache-2.0, at your option: see [LICENSE-MIT](LICENSE-MIT) and [LICENSE-APACHE](LICENSE-APACHE).
