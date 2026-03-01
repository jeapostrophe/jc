We are going to design a native Rust app for running multiple terminal sessions that are each running Claude Code and other tools. I want to start building a README.md and feature list.

The high-level goal:
* I have a particular workflow that I use with Claude and I want an app that supports it.
* I run on OS X and don't care about other platforms.
* I prefer to build things with Rust. In the Rust ecosystem, I am extremely impressed by the Zed project. We should follow its GUI practices if possible.

The high-level workflow:
* I run many different projects simulatenously
* Each project runs multiple Claude sessions at the same time
* Inside of each project, I track what I am doing in a Markdown file
* I have vague TODO lists in the Markdown and then I elaborate them and send them to Claude
* I always review Claude's changes before commit by using 'git diff'.
* I sometimes need to run arbitrary terminal commands.
* I sometimes need to look at specific parts of the code in larger contexts.
* I want to be able to build a separate app that runs on my phone and connects to this one and gets some status.
* I am used to vim style modal editting and primarily want to use keyboard control

A specific scenario:
* I am working on projects Alpha, Bravo, and Charlie
* Alpha has three active tasks.
* On the left of my screen, I see the Claude terminal working on task 1
* On the right of my screen, I see the Alpha TODO.md list and there's a section for task 1.
  It contains each thing I've sent to Claude. I wrote them in this file and sent them to Claude with command. It is formatted like
  # TODO
  # Claude
  ## Agent 1
     ### Message 0
     first command
     ### Message 1
     last command
     ### WAIT
     Future notes
* As Claude is running, I type in the "future notes" section some ideas. When it finishes, I am notified by a desktop notification and a visual indicator in the app.
* I press Cmd-Left to switch from the right-hand-side notes to Claude on the left
* I press a keybinding to go to the 'git diff' view and look at the changes
* I scroll through the changes and highlight regions and press a keybinding and a small box appears where I can write a comment about that line of code. My comments go into the 'Future notes' section of my TODO.md formatted as '* <file>:line-line --- Comment'
* I can press a key or click a button to mark that I'm done reviewing a file and it collapses
* I press a keybinding to go to the 'terminal' view and am in a separate terminal for Claude that is tied to this project and task where I can run commands. I highlight on the screen with my mouse and press the comment keybinding, this adds a comment to TODO.md like '* TERMINAL\n```[content]```\ncomment'
* Sometimes Claude produced a long textual response, I don't want to scroll in the terminal, so I press a keybinding and the app sends '/copy' to Claude and writes Markdown result into a temporary file, and I can scroll through it and write comments inline. If I write comments, the app tracks that it is different than the original '/copy' and adds annotations to TODO.md in the format '* ./reply/<id>.md:line --- Note'
* Likewise I can press a key and get a picker for the files in the repository and I can do a fuzzy search through their names to select a file
* When I navigate to a file, it is formatted appropriate for the kind of code it is and I can press a key to get a picker of the symbols of the file and jump to them. For example, if it is Rust, I type 'new' and it shows all the functions with 'new' in their name and shows the hierarchy context of them (what 'impl' block they are in).
* When navigating I can press the comment button and add a comment about that line into the TODO in the same format as when I was looking at a 'git diff'
* All of these views --- Claude, terminal, diff, TODO, code --- can appear on either the left or the right with independent scroll positions.
* When I'm ready, I navigate back to the TODO and rearrange or elaborate any of the notes. I then highlight a section of text and press a command, this command sends the content to the Claude terminal, and moves the '### WAIT' block beneath it. If I didn't select everything, that is preserved in future notes.
  For example:
  ```
    ...
    ### Message 5
    last command
    ### WAIT
    I found a few mistakes:
    * main.rs:20-25 --- This isn't efficient
    * ./reply/spurious-cat.md:50 --- I disagree with this implementation approach
    
    I think I need to implement something really different
  ```
  If I select the first three lines and press the send key, then the content is turned into
  ```
    ...
    ### Message 5
    last command
    ### Message 6
    I found a few mistakes:
    * main.rs:20-25 --- This isn't efficient
    * ./reply/spurious-cat.md:50 --- I disagree with this implementation approach
    ### WAIT    
    I think I need to implement something really different
  ```
* This one TODO.md file is shared by all Alpha tasks in memory. Using multiple tasks does not corrupt them file.
* I can create new tasks decide if they have independent git worktrees or work in the same tree.
* On the screen, I see a simple indicator that there are Claude responses ready for me to review
  - The app knows that there are responses ready because Claude has stopped producing terminal output
* I can press a keybinding and get a fuzzy picker of the projects and tasks. A simple difference when opening the picker limits it to waiting tasks or tasks in the same project
* There's a visual indicator of my Claude usage that is always displayed in one of the corners. It shows a visualization of my 5-hour window % and my weekly %, as well as how much time is remaining in the 5-hour or week and it makes it easy to evaluate my "par" time. For example, if it is 60% through the week and my weekly % is 55%, then I am "under par", but if my percentage is 75% then I am "over-par". There's a setting somewhere that I can use to mark that I don't work on Sunday or that I expect to do less work on Saturday, so my "weekly working hours" are not exactly the same as the actual "weekly hours" for the purpose of computing par.

Overall I imagine the project will have these components:
* I do NOT want to use the Claude Agent SDK, I want to run the Claude Code terminal app directly so I receive its improvements over time
* A terminal emulator so I can run Claude Code and other terminal applications directly
* A built-in network server that the mobile app connects to
* A persistent state store for when I restart (particularly important because I'll be using it while developing it)
* The mobile app itself with its slimmed down features
* A Markdown editor and renderer
* A git diff visualization
* Something that attaches to an LSP or uses tree-sitter to navigate source
* An existing pretty syntax renderer

Here are some tasks to do:
* Build a list of things we need to research
* What does the overall application look like
* How will it be structured and flow
* Should I run the terminal applications inside or should I also run tmux and have my terminal emulator connect to and manage a tmux instance
* What crates will we import and what will we implement ourselves
