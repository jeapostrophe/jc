# TODO

# Claude
## Session DELETED dreamy-tumbling-alpaca: The terminal is not properly resizing, I don't know if the error is in jc-termina...
### WAIT
## Session tender-enchanting-willow: Fixes
### Message 0
Some UI fixes:
* The session labels are not being updated in the UI (i.e. the picker) if I change them in the TODO file.
  i.e. if I change "## Session xyz: ABC" into "## Session xyz: DEF", then I expect to see "DEF" in the picker.
* Also, I want the label in the upper-left, which current shows the slug to show the label.
  i.e. it shows "jc > slug", but should show "jc > label" when "## Session slug: label" is selected
* The dimensions of the terminal (and claude terminal) need to be communicated to the terminal widget so the app they are showing will redraw
### Message 1
* Cmd-Shift-Backspace is not working to delete sessions. They are marked "Deleted" in the TODO.md but they still appear in the session picker, so they must still be active in memory
### WAIT

* The terminal needs to be scrollable with the mouse
* Shift-Tab cannot be sent to the terminal, it is interpreted as something that causes the other pane to be focused.
* Reloading sessions typically does not go to the most recent one; I think that when I do /clear it creates a new one
* Tab cannot be sent to the terminal
* Shift-Enter cannot be sent to the terminal
* In the problem hover(s), it needs to be limited in size to the most top N for small N
* In the global problem hover, it needs to show the project-wide problems only once
# Notes

Review @README.md

XXX unified picker for everything with quick filtering once it is open
