# TODO

# Claude
## Task 0
### Message 0
Review @README.md

I want to add a difficulty labeling system to the checklist. I want the following labels:
- T --- Trivial --- All trivial tasks could be done at the same time in one Claude invocation
- E --- Easy --- All easy tasks in the same sub-list could be done at the same time in one Claude invocation
- H --- Hard --- Each hard task needs its own Claude invocation, but it doesn't require human design input
- D --- Design Required --- Each design task has subtle design issues that need to be resolved with a human.

Are there other labels I should use? If so, tell me. If not, apply these labels to all of the tasks and add a note at the top of the checklist what they mean and include instructions to future Claude instances that insert into the checklist to use this system when inserting.

---

Review @README.md

Make a plan that will do as many [E] tasks as possible in parallel sub-agents scoped appropriately. There may some interaction between tasks, but there is likely not. You can't rely on the [E] marker as a guarantee there will be absolutely no overlap.

---

  - Diff view (C): Removed similar-based word highlighting entirely since it was being rendered as wavy underlines via the diagnostics system (wrong abstraction). The diff
  prefix chars (+/-) and syntax highlighting remain.
  - Even split (D): Uses element ID generation invalidation to reset ResizableState since gpui_component doesn't expose mutable panel sizes.
  - Terminal dims (E): Font size keybindings (Cmd-+/-/0) which trigger canvas re-render, automatically recalculating cols/rows and notifying PTY.
  - Themes (H): Uses window.observe_appearance() for macOS dark mode detection + manual Cmd-Shift-T toggle.

  - Terminal size change was confused

- Terminal and editor themes don't match

### WAIT XXX


 
Optimize the plan for context window usage and parallelization. Branch and fork the context/tasks into parallel jobs as appropriate.

---

I want to add a difficulty labeling system to the checklist. I want the following labels:
- T --- Trivial --- All trivial tasks could be done at the same time in one Claude invocation
- E --- Easy --- All easy tasks in the same sub-list could be done at the same time in one Claude invocation
- H --- Hard --- Each hard task needs its own Claude invocation, but it doesn't require human design input
- D --- Design Required --- Each design task has subtle design issues that need to be resolved with a human.

Are there other labels I should use? If so, tell me. If not, apply these labels to all of the tasks and add a note at the top of the checklist what they mean and include instructions to future Claude instances that insert into the checklist to use this system when inserting.

---


Make a plan that will do as many [E] tasks as possible in parallel sub-agents scoped appropriately. There may some interaction between tasks, but there is likely not. You can't rely on the [E] marker as a guarantee there will be absolutely no overlap.
