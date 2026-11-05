# Claude Code

If [Claude Code](https://claude.com/claude-code) is installed and logged in, Ghostwrite can ask Claude through it, under your login. There's nothing to set up: with no API key, Claude Code is what's asked.

## Finding it

Ghostwrite looks for `claude` on your `PATH`, and in the places its installer usually puts it, since an app opened from the Dock doesn't get your shell's `PATH`:

- `~/.local/bin/claude`, `~/.npm-global/bin/claude`, `/opt/homebrew/bin/claude`, `/usr/local/bin/claude`
- on Windows, `%USERPROFILE%\AppData\Roaming\npm\claude.cmd` and `%USERPROFILE%\.local\bin\claude.exe`

## How it's asked

Ghostwrite runs `claude -p` with Claude Code's own tools turned off and Ghostwrite's prompt in their place. The model looks around the project with Ghostwrite's [read-only commands](../stories/reading.md), as every model does.

A story is one Claude Code session. Each follow-up resumes it with only the newest turn, so it still has the files it read. If the session is gone, the whole conversation is sent again. Items asked for ahead run in a fork of the session.

To save time, a Claude Code process is started as you open the prompt (`space a`), and during a review, started on the conversation so far, so it's ready by the time you press Enter.

The `model` key in the config, or `:model`, chooses the model, and [`GHOSTWRITE_AI_EFFORT`](README.md#effort) the effort.
