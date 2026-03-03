# TODO

# Claude
## Session parallel-singing-russell: New session
### WAIT
## Session xxx: 0
### Message 0
Review @README.md

Work on these issues:
  1. [H] On startup, if TODO.md has no valid sessions, discover the most recent JSONL session group and insert a ## Session heading into TODO.md
  2. [E] Skip sessions with invalid slugs during ProjectState::create instead of creating broken SessionState entries

### WAIT XXX

---
 
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
