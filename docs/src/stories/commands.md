# Commands in the terminal

The model can run commands, but only in your terminal, and only the ones you accept. It can't type anything else there.

## How a command step looks

A command the model proposes shows on the terminal, as a step does in a file: its reason above the prompt, the command in grey at the cursor, and the keys under it. You can look over what's on the terminal with the motions first, as in normal mode. Tab sets it going. The terminal comes up if it was hidden.

## What runs

- A command runs in the project's folder, as you'd type it, for all of its output. What a model adds to read less of it (`| tail`, `| grep`, `2>&1`, `; echo $?`) is taken off first.
- Whatever you'd typed on the shell's line is given up first, as Ctrl-C does. The command goes in as a paste, so your shell's plugins leave it alone.
- A command is never typed into a program. If something is running in the shell in front (a server, a REPL, vim), Tab runs the command in a new terminal tab instead, and the corner says so.
- The model can ask for a new tab itself, to leave something running, or ask to press Ctrl-C. Tab does both just the same. A Ctrl-C goes to the shell its command ran in, wherever you've been since.

## When it's done

The next item is asked for once the command is done: the shell back at its prompt and quiet. The model is sent what it printed, so it sees how it went, and can fix a failing build or test and run it again. Meanwhile the corner says what's running.

A command that keeps going (a server, a watcher) is waited for 30 seconds, then the story goes on, the model told it's still running. Tab goes on sooner.

If the story ends with a command, the keys stay on the terminal in normal mode, so you can read what it printed.

## What the model sees of the terminal

The model reads the terminal whether or not a story runs commands. With each request it gets the end of what's on it: up to 8,000 bytes of the screen and scrollback. In a follow-up, it gets what's come since the last command ran there, when that's changed. It reads the tab in front, or while a story is up, the one its command last ran in.

The shells start with `PAGER` and `GIT_PAGER` set to `cat`, so `git log` prints all of it into the scrollback, for you and the model to read, rather than waiting in `less`.
