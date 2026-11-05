# Views and the model

A design for how views — the code, Files, the terminal, and the views plugins
make — say what the model may read and what it may write, so that the model's
side of Ghostwrite is put together from what's registered rather than written
into the editor.

Status: proposed. Nothing here is built yet.

## The rule everything here serves

Ghostwrite exists for one way of working: **the model proposes one step at a
time, and the user decides each one.** The model stays inside the editor. It
reads what views offer it, and it writes only by proposing steps, which the
user sees in place, accepts with Tab or redirects with Enter. There is no
other way for it to act.

So a view, built in or from a plugin, registers two things: **what the model
may read** (context and tools) and **what the model may write** (step kinds).
Registering a write never lets the model write: it only lets the model
*propose* that kind of step. What keeps the user in control is the step
workflow itself, the same for every kind, not a short list of allowed kinds.

## Why

Today the model knows exactly three places: files (notes, edits, creating,
removing, renaming), the terminal (a command, in a new tab, Ctrl-C) and the
project (reading and listing files). Each is written into the editor in four
places at once:

- the prompt (`ai/src/protocol/prompt.rs`) describes every kind of item;
- the parser (`protocol/parse.rs`) knows every tag;
- the context (`ai/src/context.rs`) gathers files, the terminal and git;
- the story (`state/src/story/`) knows how to show and carry out each item.

A plugin can add nothing but a few lines of rules for the prompt. So the model
can't see Lean's goals, can't ask for Typst's errors, and can't point at a page
or a goal: the views where that would matter most are invisible to it.

The views refactor gives every place the user works a name and its own keys.
This takes the next step: every view also says what it gives the model, what
the model may ask it, and what the model may propose in it. The built-in
views register this the same way a plugin does, so the interface is used by
the code that matters most from the first day.

## What stays the same

- **The model proposes, the user decides.** Anything that changes something is
  an item reviewed with Tab and Enter, one at a time. Nothing in this design lets
  the model change anything on its own.
- **One item per reply,** in the fixed-order tag format, checked part by part as
  it streams.
- **Speed.** Context is gathered off the UI thread, and a view is only asked
  for what fits.
- **The sandbox.** A plugin's part runs in its WebAssembly sandbox with only the
  access it was granted.

## The three things a view registers

Each view kind registers an *AI facet* with up to three parts.

### 1. Context: what the model is shown

A context source is a named piece of text the view produces from its current
state, with a priority and a size limit:

| View | Context today or proposed |
|---|---|
| Code | the file asked about, its lines numbered and marked; the other files in the story (today) |
| Files | the project's files, as a tree (today, in `context.rs`) |
| Terminal | what was printed since the last command (today) |
| Lean (plugin) | the goals at the cursor |
| Typst (plugin) | the compile errors; the text of the page in view |
| PDF (plugin) | the text of the page in view |

The editor composes a request from the sources in priority order until the
room is used, as `context.rs` does now with fixed sections. Each source also
says whether it's *followed*: if so, a follow-up turn carries it again only when
it changed since the model was last told — the rule the terminal already uses
(`Told`), made general.

### 2. Tools: what the model may ask

A tool is a read-only question the model can write in its `<tool>` block, in
the same one-word style as today's `read` and `list`, with a description for the
prompt:

| View | Tool |
|---|---|
| Files | `read <path>`, `list <folder>` (today) |
| Terminal | `scrollback <lines>`: more of what was printed than the context had |
| Lean | `goals <file> <line>`: the goals at a line, without moving the user |
| Typst | `errors`: compile again and say what's wrong |

Tools answer with text. They can't change anything: a plugin's tool runs with
the plugin's read access only, even if the plugin may write elsewhere. Reading
outside the project keeps asking the user first.

### 3. Steps: what the model may propose

A step kind is one kind of item: its tag, its parts in order, what each part
means, how to check it as it streams, how it's shown for review, and what
accepting it does. The existing items become step kinds of the built-in views:

| View | Step kinds |
|---|---|
| Code | `note` (lines and what about them), `edit` (lines out, lines in, why) |
| Files | `create`, `remove`, `rename` |
| Terminal | `run`, `run-tab`, `interrupt` |

Plugins register step kinds the same way, notes and writes alike, and every
one goes through the same review. What the editor guarantees for any step
kind, built in or not:

- **It's shown before it's accepted.** A step kind must say exactly what
  accepting does, in its view: the lines that change, the file that appears,
  the command that runs, the goal that's closed. Nothing happens that the step
  didn't show.
- **Changes to files are the editor's to make.** A plugin's step that changes
  text describes the change; the editor applies it as an edit, in one undo
  step, as `edit` does now.
- **Anything else stays inside the plugin's access.** A step kind that does
  more than change text (a plugin command run on accept: rebuilding a preview,
  asking a proof checker) runs in the plugin's sandbox, with only the access
  the user granted when installing it, and the manifest says so, so it's shown
  at install.
- **One at a time.** A plugin's step is one item like any other: reviewed
  where it applies, with Tab and Enter, never batched, never accepted for the
  user.

Examples: Lean could register `close-goal` (the tactic to insert, shown with
the goal it closes, applied as an edit); Typst could register a note on a page
("this table overflows the margin"), shown on the page in its view.

## How the protocol is put together

The prompt's description of the reply format (`prompt::grammar`) becomes a
list built from the registered step kinds: each contributes its tag, its parts
and one example. Tools contribute their line to the tool section. Rules stay as
they are.

The parser becomes generic over step kinds: it knows the tag syntax and the
fixed order of parts, and asks the step kind to check each part as it closes.
The built-in kinds keep their current checks (lines that must be found, a path
inside the project, a command that's plainly a command). A reply with an
unknown tag goes back to the model as a correction, as malformed replies do now.

A step kind is only offered while its view exists in the window: no `run`
without a terminal, no Lean notes without Lean installed. The prompt says only
what can be done.

## The interface

### Built-in views

A Rust trait in `ghostwrite-state`, implemented by the code, Files and the
terminal, and by an adapter for plugin views:

```rust
pub trait AiFacet {
    /// What the model is shown, by source, for this request.
    fn context(&self, room: usize) -> Vec<Context>;
    /// The tools this view answers, and answering one.
    fn tools(&self) -> &[ToolSpec];
    fn tool(&self, name: &str, args: &[&str], room: usize) -> Result<String, String>;
    /// The step kinds this view offers.
    fn steps(&self) -> &[&'static dyn StepKind];
}

pub trait StepKind {
    fn tag(&self) -> &'static str;
    fn parts(&self) -> &[Part];          // names, in order, and whether optional
    fn example(&self) -> &'static str;   // for the prompt
    fn check(&self, part: &str, text: &str, cx: &CheckCx) -> Result<(), String>;
    fn show(&self, item: &Item, view: &mut dyn ShowCx) -> Show;
    fn accept(&self, item: &Item, cx: &mut ApplyCx) -> Result<Applied, String>;
}
```

For a plugin's step kind, the adapter calls into the plugin for `check` and
for what to show, and for `accept` takes back a description of what to do:
edits for the editor to apply, and at most one of the plugin's own commands
to run in its sandbox. `ApplyCx` never hands a plugin the document or the
terminal: it applies what the plugin described.

### Plugins

An optional interface in the WIT package, `ghostwrite:plugin/ai@1.0.0`, beside
the existing one. A plugin that doesn't export it has no AI facet, and every
plugin built today keeps working: this adds to version 1 instead of changing it.

```wit
interface ai {
    record context { name: string, text: string }
    /// What the plugin's view shows the model now, given its state.
    context: func(view-state: string, room: u32) -> list<context>;
    /// Answer one of the tools the manifest names.
    tool: func(name: string, args: list<string>, room: u32) -> result<string, string>;

    record edit { file: string, old: list<string>, new: list<string> }
    record proposal {
        /// What the user sees before accepting: blocks, as panels have.
        shown: list<block>,
        /// What accepting does: edits the editor applies, and a command of the plugin's own.
        edits: list<edit>,
        command: option<string>,
    }
    /// Check one part of a step of a kind it registered, as it streams in.
    check-part: func(kind: string, part: string, text: string) -> result<_, string>;
    /// The whole step, once it's in: what to show, and what accepting would do.
    propose: func(kind: string, parts: list<tuple<string, string>>, view-state: string) -> result<proposal, string>;
}
```

The editor shows `shown` and the edits in the plugin's view for review. On
Tab it applies the edits itself and only then runs `command`, if any, in the
sandbox. On Enter nothing happens and the user's words go back to the model.

The manifest declares the rest, so the editor can describe it without running
anything and show it when the plugin is installed:

```json
"ai": {
  "context": [{ "name": "goals", "priority": 2, "followed": true }],
  "tools": [{ "name": "goals", "args": "<file> <line>", "what": "the goals at a line" }],
  "steps": [{
    "kind": "close-goal",
    "parts": ["at", "tactic", "say"],
    "what": "insert a tactic that closes the goal at a line",
    "example": "<close-goal>\n<at>\n12│  sorry\n</at>\n<tactic>\nsimp\n</tactic>\n<say>\nsimp closes it\n</say>\n</close-goal>",
    "runs": null
  }]
}
```

`runs` names the plugin command accepting runs, if any. Installing a plugin
shows what it lets the model read and propose, alongside what it can reach:
"shows the model the goals at the cursor; answers `goals`; may propose
`close-goal` (an edit)".

## Privacy

Everything sent to the model today is what the user can see in the editor or
the project. Plugin context keeps that rule: a context source only describes
what its view shows. The docs' page on what's sent lists each registered
source, and `:plugin list` says which plugins add to it.

## Order of work

1. **The facet for built-in views.** Move today's items, context sections and
   tools behind `AiFacet` and `StepKind` without changing what the model sees:
   the same prompt, byte for byte, built from registrations. The existing story
   tests are the check.
2. **The terminal's `scrollback` tool,** as the first new tool, built-in.
3. **The plugin interface:** the optional `ai` WIT interface and the manifest
   block, with Lean's goals as context and its `goals` tool, and Typst's
   errors.
4. **Plugin step kinds,** reviewed in the plugin's view: notes first, then
   writes, with Lean's `close-goal` as the first.
5. **Edits shown in a plugin's view** (Typst's pages beside the change).

Each step lands on its own with scripted tests: replies are scripted, never a
real model.

## Open questions

- **Priorities across plugins.** When Lean's goals and a large terminal output
  both want room, which wins? A fixed order (the view in focus first) may be
  enough.
- **Tool budgets.** Should a plugin's tools have a time limit per call beyond
  the plugin's own `seconds`? A slow tool holds up the model's answer.
- **Local models.** Small models struggle with many tags. The prompt could
  offer only the step kinds of the views on screen, which helps them too.
- **Ordering of tags in the prompt.** Byte-for-byte equality in step 1 fixes
  today's order; later kinds append.
