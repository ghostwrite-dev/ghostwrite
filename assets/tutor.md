# Ghostwrite tutor

This is a scratch tab: change anything, and close it with :q! when you're done.
Ghostwrite is modal: keys move and act, and you only type text in insert mode.
Press ⌘/ at any time to see every key. j and k scroll it; any other key closes it.

## Moving

h j k l    left, down, up, right
w b e      next word, back a word, end of word
gg G       top of the file, bottom of the file
/ then n   search, then the next match

Try it: go to the line below, and put the cursor on the word "here".

    The cursor should end up here.

## Selecting, then acting

Keys select first and act second, the other way round from Vim.

w          select to the next word
x          select the whole line
d          delete what's selected
c          delete it and start typing
y p        copy it, paste it
u U        undo, redo

Try it: delete the extra word on the line below, with w and then d.

    This line has has one word too many.

## Typing

i a        start typing before the selection, or after it
esc        back to moving
:w         write the file

## Asking the model

Point at some lines, press space a, and say what you want. A question is answered
with notes pinned to the code. A change comes as small steps:

tab        accept this step, and see the next
enter      say something about it: redirect, or ask why
esc esc    stop, keeping what you've accepted

Nothing changes until you accept it, and :undo-story takes back a whole story at once.

To set up a model, use one of these:

:api-key ‹key›     a Claude API key, kept where your system keeps secrets
:model             see what can be asked, and :model ‹name› to choose

Claude Code works too if it's installed, under its own login.

## Getting around

⌘P         pick a file (⌘E: an open one)
⌘⇧F        search the project
:replace old new   change it across the project, step by step

⌘B         Files, the file tree; again, it hides
⌘J         the terminal, typing in it; again, it hides
space v    the file's pages, if a plugin makes them (a Typst file's, say)
space g    the side view: a plugin's panel (Lean's goals), or the debugger

Each is a view, as the code is: the mode line says where the keys are
(NOR · files, INS · terminal). A click puts them where it is.

⌘\               two panes; ⌘K the other one; space w q only this one
                 in the terminal, every key but ⌘ goes to the shell, Esc too
esc              from Files or the side view, back to the code

That's the start. ⌘/ has the rest.
