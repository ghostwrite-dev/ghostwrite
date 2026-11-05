# Steps

Every reply from the model is exactly one item. Each has a short reason, shown with it.

## Notes

A note is pinned to some lines and says one to three sentences about them. Questions are answered with notes, in the order you'd walk someone through the code. A note about a file as a whole sits at its top. Tab reads on.

## Edits

An edit replaces a few lines, usually about one: the next thing you'd type. The lines it removes are struck through, and its new lines show as ghost text under them. Tab applies it at once, so the model's next item sees the file with it.

A change that spans files goes file by file: a function and its callers, a type and where it's built, the code and its tests. Each step names its file, and Ghostwrite opens it when the step is shown.

The model quotes the lines it changes. Ghostwrite finds them even when the quote's spacing or indentation is a little off, and new lines take the file's indentation.

## Files made, removed and renamed

| Item | What you see | What Tab does |
| --- | --- | --- |
| a new file | the file in the tree, with its first few lines (at most 8) | creates it; later steps fill it in |
| a removal | the file marked in the tree | moves it to the Trash |
| a rename or move | the file and its new path in the tree | renames it |

A new file is never written all at once: after its first lines, the rest comes as ordinary edits, so you see it built step by step.

## Commands

A command step runs something in your terminal: a build, the tests, a formatter, an install, a git command that changes something. It waits at the shell's prompt, grey, with its reason above and its keys below. Tab runs it. See [Commands in the terminal](commands.md).

Two other items act on the terminal:

- a command in a new tab, leaving what's running in this one going (a server to test against, say);
- an interrupt, which stops what's running, as Ctrl-C does.

## Done

When there's nothing more to show or change, the model says it's done, and the story ends. It may also update its notes on the project then: see [What the model is sent](context.md#project-memory).
