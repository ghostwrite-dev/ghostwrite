# Undo, clear and timings

## Undoing a story

`u` undoes one step at a time, as for any edit. `:undo-story` takes back every step of the last story at once, in every file it changed. The files are left unsaved, so you can look before you write them.

It only works if nothing's been edited in those files since the story. If something has, it says which file, and `u` undoes it step by step instead.

`:undo-story` undoes edits. Files the story created, removed or renamed, and commands it ran, aren't undone.

## Starting over

```text
:clear
```

`:clear` ends the story that's up, and makes the model forget its notes on the project and its log of your recent requests. The next story starts with nothing remembered. See [Project memory](context.md#project-memory).

## Timings

```text
:timings
```

`:timings` says how long the model has kept you waiting lately: until the first of its reply arrived, and until there was something to look at. It also says whether the model's process had been started ahead of time. Use it to compare models and backends.

Ghostwrite starts Claude Code as you open the prompt, and a local model loads as you start typing, so the wait is shorter by the time you press Enter.

## Usage

At the end of a story, the status line says how many tokens it used: `done · 12.4k tokens in, 800 out`. That counts what was read from and written to the prompt cache too. The count is shown for the Anthropic API and Claude Code; a local model costs nothing to ask.
