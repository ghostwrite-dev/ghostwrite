# AI stories

A story is one request to the model and the conversation it starts, reviewed one item at a time.

You point at some lines and say what you want. The model answers with one item: a note, a small edit, a file to make or remove or rename, or a command to run. You accept it with Tab, or say something with Enter, and it answers with the next item, knowing what you did. That goes on until the model says it's done or you stop it.

There's no plan to keep in step and nothing to skip. The model only ever answers "what's next?", seeing the files as they are now, with what you've accepted in them.

Nothing changes until you accept it:

- an edit is shown as ghost text until Tab applies it;
- a new file, a removal or a rename is shown in the file tree until Tab does it;
- a command waits at your terminal's prompt until Tab runs it;
- the model can read the project on its own, but nothing outside it unless you say yes.

This chapter covers:

- [Asking and reviewing](reviewing.md): `space a`, Tab, Enter, Esc Esc.
- [Steps](steps.md): what each kind of item looks like.
- [Commands in the terminal](commands.md): how a command runs and how the model sees its output.
- [Looking around and reading outside](reading.md): what the model can read, and when it asks first.
- [Undo, clear and timings](managing.md): `:undo-story`, `:clear`, `:timings`.
- [What the model is sent](context.md): context, privacy, and the project's memory.

Which model is asked, and how, is in [Models and backends](../models/README.md).
