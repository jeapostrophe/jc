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

### WAIT XXX
* I don't want a project list pane, that will be a picker
