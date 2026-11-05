# Ghostwrite tutor

This is a scratch tab: change anything, and close it with :q! when you're done.
Ghostwrite is modal: keys move and act, and you only type text in insert mode.
Press space ? at any time to see every key. j and k scroll it; any other key closes it.

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

space f    pick a file
space e    Files, the file tree (again: back to the code)
space t    the terminal, typing in it (again: back to the code)
space /    search the project
:replace old new   change it across the project, step by step

Files and the terminal are views, as the code is, and the keys work the same way
in each: the mode line says where they are (NOR · files, INS · terminal).

ctrl-w h j k l   the view left, below, above, right
esc esc          typing in the terminal: back to normal mode, to move through
                 what it printed (in vim and the like, ctrl-\ ctrl-n)
i                type in the terminal again
esc              from Files or the terminal, back to the code

That's the start. space ? has the rest.
