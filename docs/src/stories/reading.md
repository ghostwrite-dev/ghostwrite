# Looking around and reading outside

## Inside the project

The model looks around the project as it likes, without asking you. It does that with a small set of read-only commands, which Ghostwrite runs itself:

| Command | What it does |
| --- | --- |
| `read path [first lines]` | read a file, or part of it, with numbered lines |
| `list [folder]` | list a folder |
| `search pattern [folder]` | search the project with a regular expression |
| `git status`, `log`, `diff`, `show`, `blame` | read the project's git history |

None of these change anything; changes only ever come as steps you accept. Every model uses the same commands, however small. While they run, the top right says what the model is looking at.

## Images, PDFs and the web

`read` on an image or a PDF shows the model the file itself, if it can see them (Claude can; local models are told they can't). A file that's too big, or in a format it doesn't take, is converted first.

`read` on a web address works only for images and PDFs. A web page's text is never passed on, so nothing on the web gets to tell the model what to do. And only the public web is reached: nothing on your machine or its network, however a model is led there.

## Outside the project

When the model wants to read a file or folder outside the project (a config file in your home folder, another project), it has to ask. The top right shows what it wants to read:

| Keys | What they do |
| --- | --- |
| `tab` (⇥) | let it: that file, or everything in that folder, for the rest of the story |
| `enter` (⏎) | don't |

A permission lasts only for the story. The next story asks again.
