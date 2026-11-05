# What the model is sent

A story is a conversation. Its first message carries the context, then the file with your lines marked, then your request. Each turn after it says what you did with the last item and what you said, and shows the files as they are now.

## The first message

| Part | What it is | Limit |
| --- | --- | --- |
| Project instructions | `AGENTS.md`, `CLAUDE.md` and `.ghostwrite.md` at the project's root, if there are any | 20,000 bytes of each |
| The model's notes | what it chose to remember about the project and you, from earlier stories | 4,000 bytes |
| Recent requests | your last 12 requests in this project and what came of them; the last 4 with what the model said | |
| Project files | the list of the project's files (not their contents) | 400 files |
| The editor | the files open in tabs, and diagnostics near the marked lines | |
| The terminal | the end of what's on it | 8,000 bytes |
| Uncommitted changes | `git diff` of the project | `max_diff`, 12,000 bytes by default |
| The file | the file you asked about, its lines numbered, around the marked lines | `context_bytes`, 200,000 bytes by default |
| Your request | what you typed | |

Rules that plugins add for their languages are part of the system prompt.

## Each turn after

- what you decided about the last item, and what you said, if anything;
- the terminal, if it's changed since the last command;
- the file you asked about and the other files the story is in, as they are now, with what you've accepted in them.

Everything else the model reads, it asks for with [its read-only commands](reading.md), and you see it doing so.

With a local model, every part has a small share instead, so the whole first message fits in about 8,000 bytes. It can read more as it needs it.

## Privacy

What leaves your machine depends on the [backend](../models/README.md):

- **The Anthropic API** and **Claude Code** send all of the above to Anthropic.
- **A local model** sends nothing anywhere. It runs in a process of its own on your machine.

The model reads nothing outside the project without asking, and nothing from the web except images and PDFs. See [Looking around and reading outside](reading.md).

Your API key is kept in the system's secret store, and never on a command line. See [The Anthropic API](../models/api.md).

The last request and its replies are kept in Ghostwrite's support folder, in `last-reply.txt`, for when you want to see exactly what was sent.

## Project memory

The model keeps two things per project, in Ghostwrite's support folder rather than the project, so nothing lands in your repository:

- **Its notes.** When a story ends, the model may rewrite its notes on the project and on you: a preference, a convention, a decision. They're kept under 300 words, and go into every request after.
- **A log of recent requests.** What you asked, what you said during the review, and what came of it ("answered", "applied 2 of 3 steps", "stopped"). The last 200 are kept.

The support folder is `~/Library/Application Support/Ghostwrite` on macOS, `%APPDATA%\Ghostwrite` on Windows, and `~/.local/share/ghostwrite` on Linux. Each project has its own folder under `project-state/`.

`:clear` forgets both, for this project. See [Starting over](managing.md#starting-over).

## Telling the model how your project works

Put it in `AGENTS.md`, `CLAUDE.md` or `.ghostwrite.md` at the project's root. Ghostwrite reads all three, if they're there, and sends them with every story. That's the place for conventions you'd otherwise repeat: "tabs, not spaces", "tests go in `tests/`", "never touch `generated/`".
