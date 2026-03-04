# TODO

# Claude
## Session delightful-wibbling-pond: Revise the session attach picker to inspect something about the files to give con...
### Message 0
This is a test, print out the word "banana"
### WAIT
## Session parallel-singing-russell: New session
### WAIT
# Notes

The content got insert including the new line, it wasn't actually submitted.

Read @README.md

There are two problems:
* The session we are in should be highlighted differently in the TODO view, so you can more easily identify which ### WAIT is relevant. Perhaps the existing formatting that overrides the markdown rules should ONLY apply to *this* session
* Shift-Up Arrow doesn't select what I expect, when you go down or up a line, it also moves over a character

---

XXX Clicking on a pane (e.g. the TODO list) focuses the pane, but it doesn't change the "active" pane, which means keybindings like changing the view don't work

---

Review @README.md

Optimize the plan for context window usage and parallelization. Branch and fork the context/tasks into parallel jobs as appropriate.

---

> **Difficulty labels** — applied to each unchecked task:
> - **[T]** Trivial — All trivial tasks can be done together in one Claude invocation
> - **[E]** Easy — All easy tasks in the same sub-list can be done together in one Claude invocation
> - **[H]** Hard — Each hard task needs its own Claude invocation but requires no human design input
> - **[D]** Design — Subtle design issues that need to be resolved with a human first
> - **[?]** Unclassified — Needs triage. When you encounter a `[?]` task, read the task description, examine the relevant code, and replace `[?]` with the correct label (`[T]`/`[E]`/`[H]`/`[D]`). Do this before starting any other work.
>
> *When adding new checklist items, always include a `[T]`/`[E]`/`[H]`/`[D]`/`[?]` label after the checkbox.*

---


Make a plan that will do as many [E] tasks as possible in parallel sub-agents scoped appropriately. There may some interaction between tasks, but there is likely not. You can't rely on the [E] marker as a guarantee there will be absolutely no overlap.
