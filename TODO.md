# TODO

# Claude
## Task 0
### Message 0
Review the @README.md and ask me clarifying questions and suggest ways to improve the layout. Begin building a checklist at the bottom of tasks that need to be performed for this project.
### Message 1
* Your restructuring is good, do it.
Questions:
1. The user can create as many windows as they want, but each window shares the same "sessions". So, I can have two windows looking at 'Alpha > Task 1' and if I change/scroll in one, it does in the other two. As far as multiple panes, I think that I only ever want to have two panes. Sometimes, I want to do a quick keybinding to open up the terminal window in the bottom view in 'Quake' style, but that is a special case and not a generic panel.
2. I assume the Markdown editor does source editting but has 'light' rendering by making things bold or highlighted
3. The code viewer allows modification of code. However, this is not a full-fletch code editor. There are many things that people want out of code editors that we won't do. There's a keybinding to open up the file in a "real" editor, like Zed.
4. The mobile app has a dashboard view, but can also send commands. I expect that the mobile app will be much worse at viewing code and editting it and I expect it will mostly be about reviewing and taking notes; and dealing with Claude permission requests. It is LIGHT.
5. I assume that the app is the only modifier of the TODO file. If it is modified on the filesystem, the app will notice and give a visual indication. There could be a feature where we do a 'git'-style merge on the filesystem and the current state of the buffer.
6. We do not need full terminal functionality. However, Claude Code seems to be a pretty advanced terminal application, so I expect we need to do a lot. My assumption is that we can use whatever library Zed is using.
7. The mobile link will work by the desktop version showing a QR code which embeds an auth key. It will use TLS/etc for communication. It will be local only. I'll use some sort of tunnel tech like Tailscale to connect securely.
8. By "vim", I do NOT mean the full gambit of Vim features. I really just mean that it is primarily keyboard based. I tend to actually use more emacs style bindings.
9. The app has its own state somewhere (~/.config/jc) that has a file where the active projects are. The user can use a command in the app to add another project and then tasks. The user can also run a command-line tool like 'jc .' to add a project.
10. The app manages the git worktrees
### Message 2
Review @README.md and launch parallel sub-agents to perform all of the '### Research' tasks. Take their input and add commentary to README about those tasks and potentially modify the overall tasklist. If you need to ask questions for direction, do so.
### Message 3
start building the core infrastructure
### Message 4
I don't think it is necessary to build any of the git worktree management yet. That is a long-term feature. In the beginning, we'll just have tasks share the same worktree.
***
Adjust the README to note that the 'git worktree' functionality is not core and is coherent that will be implemented later
### Message 5
Review @README.md and identify the next task to work on. Check with me for what it is before making a detailed plan.
### Message 6
My comments on the design decisions
1. Own crate --- I agree
2. Why not the Zed fork? What's different about them? What is the downside of using it vs not? Is the Zed one old and they have stuck to it for stability or have they improved it in incompatible ways?
3. PTY --- Agree on all that
4. Batching --- Good
5. canvas --- Good
### Message 7
Update the README with the completed work

Add these things to the task list (some of them are minor and may need a special notation)
* The window doesn't get focused when it is created
* The theme should be in a file
* There should be a light theme
* The theme should automatically change with the system dark mode
* The window doesn't have any keybinding, such as for minimize or close

Commit
### WAIT XXX
