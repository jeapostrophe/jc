# TODO

# Claude
## TODO
> uuid=40f39fb5-dacf-4da0-9151-f2db04ecba2d
### Message 0
Sometimes saving jumps the TODO.md focus to the top; it doesn't always, but I haven't been able to figure out what is doing it.
### Message 1
TI want to revise the Cmd-; command to always use the right-most visible pane to seek. The basic idea is that I almost always want Claude to be on left-pane and then I will use the right-pane for TODO and the third pane for other things; but if the window is small, I'll use the right-pane for the problem seeking
### Message 2
Tiny problem: In the TODO view (and presumably code view), when I press Shift-Delete, it doesn't do a character deletion
### Message 3
Commit all the changes in the repo
### WAIT
## Detach
> uuid=7e6eef68-f1c6-4e54-b96d-cf985da010c4
### Message 0
I want to investigate this problem/feature:
- I have some projects that I want to use sometimes, but rarely. I don't want to delete them from the state.toml's project list, because I want them easily accessible.
- The problem is that when I start 'jc' it automatically attaches to everything in the TODO file.
- I want to extend the TODO format with a simple [D] note that causes the session to NOT be automatically attached.
- This might be the same thing as the current "deleted" which is bound to Cmd-Shift-Backspace in the picker, but with a bit of a more ergonomic format AND I want it to show up in the 'adopt' list, which I think deleted things don't. If I want something truly deleted, I'll detach and then remove the todo section
### Message 1
Let's make Cmd-Shift-Backspace toggle [D] and ensure that if the user changes the '[D]' to '[DELETED]' we notice and remove it from the adoption list. Thus D -> DELETED will always be a human thing
### Message 2
Let's remove support for the backwards compatible 'DELETED' option.

We need to ensure that if a project has no entries in its TODO at all, then in the Cmd-P picker we show the option to create a new one and not auto-create one ourselves on start-up
### Message 3
The project picker (Cmd-P) is too optimistic. It shows things as adoptable sessions even if they have no uuid=, which means trying to use them would actually create a new session.
### Message 4
It also appears that the TODO parser is marking as sessions everything at the '##' height rather than only things inside '# Claude'. An example is @../gb.rs/TODO.md
### Message 5
Let's make the project actions picker and project picker have a small box at the bottom (after the picker entries, with a bar) that shows a little glossary of what the symbols mean and for Cmd-Shift-P it should say that Cmd-Shift-Backspace detaches a session
### Message 6
Change the project picker so that (adopt) and (new) entries are always near the bottom. It should be:

[this project, problems]
[this project, attached]
[other projects, attached (everything for each project together)]
[other projects, new if no sessions]
[this project, detached]
[other projects, detached]
[the current session]

Are there any other categories I'm forgetting about how they should sort?
### WAIT
## jc from other directories
> uuid=06486e67-f388-4e12-bdbe-9144bc447675
### Message 0
When I am in another directory and I run the 'jc' command in my path

lrwxr-xr-x 1 jay staff 25 Mar 17 18:49 /Users/jay/Dev/dot-files/bin/jc -> /Users/jay/Dev/jc/make.sh

It fails. I think we need to launch a sub-shell for the build and then return to the original shell for execution
### WAIT
