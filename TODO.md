# TODO

# Claude
## Session delightful-wibbling-pond: Revise the session attach picker to inspect something about the files to give con...
### WAIT
Test
# Notes

Review @README.md

I want to work on the '### Problems & Status' section

Build a plan for these problems.
Optimize the plan for context window usage and parallelization. Branch and fork the context/tasks into parallel jobs as appropriate.
Limit scope relative to an expectation of how the context window will grow.

---

* The ! in the title bar doesn't need a hover; it is okay if only the numbers have one
* Make a "problem picker" (propose a keybinding) that lets you select the problems in the current session and jump to them
* Make the Cmd-; jump to the problem in whatever the active pane is, rather than the left
* The Cmd-; needs to jump to the next problem if you are pressing it over and over again --- how to do this well?

---

Find all direct color values in the code (e.g. Hsla) and ensure that they are using the colors from the theme set

---

Review @README.md

Optimize the plan for context window usage and parallelization. Branch and fork the context/tasks into parallel jobs as appropriate.
Limit scope relative to an expectation of how the context window will grow.

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
