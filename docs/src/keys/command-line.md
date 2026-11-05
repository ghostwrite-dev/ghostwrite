# Command line (:)

`:` opens the command line, in any view (its command name is `command_mode`). ↑ and ↓ go through the commands you've run before (kept between launches), and Tab completes a command's name, a path, a model's name or a plugin's. `:api-key` is never kept in the history.

A number on its own goes to that line: `:42`.

## Files and tabs

| Command | What it does |
| --- | --- |
| `:e path`, `:edit`, `:o`, `:open` | open files |
| `:w [path]`, `:write` | write the file (or to `path`) |
| `:w! [path]`, `:write!` | write over changes made on disk, or over another file |
| `:wq`, `:x`, `:write-quit` | write and close; `:x` only writes if there's something to |
| `:wa`, `:write-all` | write every file |
| `:wqa`, `:xa`, `:write-quit-all` | write every file and quit |
| `:q`, `:quit` | close the window, asking about unsaved changes |
| `:q!`, `:quit!` | close it without asking |
| `:qa`, `:quit-all` (`:qa!`) | quit the app: every window |
| `:n`, `:new` | a new, empty tab |
| `:bn`, `:bnext`, `:buffer-next` | the next tab |
| `:bp`, `:bprev`, `:buffer-previous` | the previous tab |
| `:bc`, `:bclose`, `:buffer-close` (`:bc!`) | close the tab (`!`: without saving) |
| `:tab-left`, `:tab-right` | move the tab along the tab bar |
| `:rl`, `:reload` (`:rl!`) | reload the file from disk (`!`: dropping your changes) |
| `:rm [path]` (`:rm!`) | move the file, or `path`, to the Trash |
| `:42` | go to line 42 |

## Views

| Command | What it does |
| --- | --- |
| `:tree`, `:explorer` | go to Files, the file tree; from there, hide it |
| `:term`, `:terminal` | go to the terminal, typing in it |
| `:preview` | a file's pages in a tab of their own, or what a plugin shows beside it; again to go back |
| `:vs`, `:vsplit` | a second pane beside this one |
| `:only` | only this pane |
| `:project [folder]`, `:projects` | the projects page, or straight into a folder |

## Search and language servers

| Command | What it does |
| --- | --- |
| `:grep pattern`, `:g`, `:search` | search the project, and go through the matches |
| `:replace old new` | go through the project's matches as steps |
| `:replace! old new` | make every change at once |
| `:format`, `:fmt` | format the file with its language server |

## The model

| Command | What it does |
| --- | --- |
| `:model` | say which model is asked, and which could be |
| `:model name` | ask that model from now on (Tab completes) |
| `:api-key key` | keep a Claude API key where your system keeps secrets |
| `:api-key` | say whether there's a key saved |
| `:api-key clear` | take the saved key out |
| `:undo-story` | take back every step of the last story |
| `:timings` | how long the model has kept you waiting, lately |
| `:clear` | start over: end the story, and forget the model's notes and recent requests |

## Debugging

| Command | What it does |
| --- | --- |
| `:break` | a breakpoint on the cursor's line, or none if there was one |
| `:debug program args` | run the program in the debugger |
| `:over` | run to the next line |
| `:step` | step into the call |
| `:finish` | step out of it |
| `:continue` | on to the next breakpoint |
| `:stop` | stop debugging |

## Plugins and the app

| Command | What it does |
| --- | --- |
| `:plugin install source` | fetch a plugin and check it, and ask whether to install it, saying all it adds and can reach: see [Plugins](../plugins/README.md) |
| `:plugin install! source` | install it without asking |
| `:plugin link folder` | use a plugin from a folder where it is, reading it again when it changes |
| `:plugin update [name]` | fetch one plugin again, or all you installed (asking first if one would reach more) |
| `:plugin remove name` | remove one (a linked one: only the link) |
| `:plugin list`, `:plugins` | the plugins there are, and what's wrong with any that couldn't be read |
| `:tutor` | learn the keys, in a scratch tab |
| `:update` | say whether there's a newer version |

Plugins can add commands of their own. They're completed with the rest, and the editor's own commands always come first: a plugin can't replace one.
