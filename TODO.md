# TODO

# Claude
## Task 0
### Message 0
Review @README.md and identify the next task to work on. Check with me for what it is before making a detailed plan.
### Message 1
* Let's add a second terminal option and the ability to actually select what appears in the panes.
* Initially have two terminals, they should be referred to as 'Claude' and 'Terminal' in the code, but they will actually just be two terminals for now.
* Make it possible to see the terminals in either pane, including the same terminal in both panes.
### Message 2
Yes, that is right. However, we can drop the project list pane altogther. That will be a picker eventually.

I want a keybinding based system. I imagine that the bindings feel like:
* Cmd-[] --- Switch the focus betwen the right and left pans
* Cmd-1234 --- Switch between the different views inside of a pane

---

Update the readme and commit

---

Here are some small UI tweaks:
* Remove 'ctrl-`' as cycle; I don't need it
* Add a bar at the top that shows the active project and which will be the repository for the usage information. I think it will look like:

+--------------------------------------------------------------------------------+
| Project > Task   <center>Left Pane Label   | Right Pane Label</center>   Usage |

* I don't see any visual indication of which pane is in focus. Zed uses a blinking cursor and there's a highlighting in the pane label. 
* Close the app when the last window closes

---

Zed has some interesting things in its title bar. On the left, it shows the project, git branch. On the right it has an update button and user profile picture.

---

I don't see the accent-color left border, it is too subtle for me. I do notice that the width slightly changes so the panes look like they are "wobbling" when I switch views. That is a problem and it should be fixed. I also don't see the blinking cursor in either terminal.

---

Read @README.md
Implement the diff, TODO, and code displays in a basic version. I assume that they will share code because they all display text with some editting capability and tree-sitter highlighting.

---

Add these things to the README.md Task Checklist:
* The code viewers should use a different font, especially monospace
* I don't want line numbers in the code viewers
* I want a theme system that ties together the UI, the terminal, and the code viewers.

---

* Read @README.md
* I want to implement these items:
- [ ] Implement Claude usage algorithm 
- [ ] Implement configurable working hours for par calculation
* In the initial version, I want to have a separately runnable example (`cargo run --example claude_usage --- 38 thu 2159`) that can just run the weekly algorithm with information delivered by hand.
* This should print out something like:
    Limit Usage: 38%
     Week Usage: 50% [assuming the week is 50% over]
  Working Usage: 60% [assuming the working hours are 60% over]
* These %s should be given as numbers and as an ASCII art graph
* If week/working usage is significantly over it should be one color and if it is significantly under it should be a different color
* The configuration of working hours should be in the configuration file in `~/.config/jc` and should be its own TOML section and should be something like:
  mon = [8, 18]
  tue = [10, 12]
  etc
* Each day is given the start work hour and the end work hour. If the hours are ill-formatted (greater than 24 or not increasing), then the entire is ignored (treated as [0,0]) and a warning is emitted.
---
You misinterpreted the arguments. The "thu 2159" are not the current time; it is the time that the weekly limit resets for Claude.

### WAIT XXX
