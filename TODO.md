# TODO

# Claude
## Session delightful-wibbling-pond: Revise the session attach picker to inspect something about the files to give con...
### WAIT
Test
# Notes

Review @README.md

Make a plan to solve these issues:
- [ ] [E] Implement independent scroll positions per project (go back to a project and everything is as you left it)

Optimize the plan for context window usage and parallelization. Branch and fork the context/tasks into parallel jobs as appropriate.
Limit scope relative to an expectation of how the context window will grow.

---

* The help-overlay should be taller and wider (two column) to show everything without scrolling
* 

---

XXX unified picker for everything with quick filtering once it is open

# Common

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
