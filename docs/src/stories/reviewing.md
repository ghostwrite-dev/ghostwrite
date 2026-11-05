# Asking and reviewing

## Asking

Put the cursor on a line, or select some lines, and press `space a`. A prompt opens under the code. Type what you want and press Enter.

- A question ("what does this do?", "why is this slow?", "walk me through the request path") is answered with notes pinned to the code, one at a time, across files as needed.
- A request for a change ("add a timeout", "handle the empty case", "rename this to `size`") comes as steps.

Where you ask from matters: the lines you point at are marked for the model, and it starts from them. You can ask in any view. On the terminal, `space a` asks about what's selected there.

While the model is thinking, ✦ and what it's doing ("reading src/lib.rs") show in the top right of the code, under what you asked.

## Reviewing

While a story is up the editor is in AI mode. These are the keys:

| Keys | What they do |
| --- | --- |
| `tab` | accept the step and see the next. For a note, read on |
| `enter` (or `space a`) | say something: redirect, correct, or ask a question |
| `esc` `esc` | stop. What you've accepted stays |
| motions | look around; you can't edit until the story's done |

The first `esc` says "esc again to end this story", so one stray Esc doesn't throw anything away.

### Redirecting

Enter opens the prompt again. What you type goes to the model with the step you're on: "no, use the existing helper", "why this and not a match?", "also do the tests". The next item follows what you said. A question is answered with a note, and the story goes on from there.

There's no skip: if you don't want a step, say what you want instead.

### Looking around

While a story is up you can move, open files with `space f` or `space b`, switch tabs and scroll. A step that arrives while you're elsewhere waits in the corner rather than pulling you away. The first Tab brings you back to it; the next one accepts it.

### Where to look

When the story moves on to a step on other lines, in another file, in the file tree or in the terminal, a light stretches from where it was to where it is now, leading edge first, so your eye follows it. A step's lines light up for a moment once it's accepted. `"animations": false` in the [config](../configuration/README.md) turns this off.

### Speed

While you review one item, the model is already asked for the next ones, up to ten ahead, each assuming you accept the ones before. Accept, and the next is there or already streaming. Say something, and what was asked ahead is dropped. A command is where asking ahead stops, since what comes next depends on what it prints.

A local model is asked one item ahead at most, since it answers one request at a time.

### When a reply goes wrong

If the model writes something Ghostwrite can't use (a reply that breaks the format, an edit whose lines aren't in its file, a file that doesn't exist), it goes back to the model as a correction before you see it, up to three tries in a row.

## When it ends

The story ends when the model says it's done, or when you stop it. The status line says what came of it: `applied 3 of 4 steps · 12.4k tokens in, 800 out`.

## Stories the editor gives

The same review works for lists the editor makes itself:

- `space d` goes through the file's diagnostics, one note at a time.
- `space /` and `:grep` go through the project's search matches.
- `:replace old new` offers each line it would change as a step.

Tab reads on, or accepts. Enter asks the model about the item you're on, and the model takes over the story from there.
