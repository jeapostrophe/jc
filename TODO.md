# TODO

# Claude
## Task 0
### Message 0
Review @README.md

Work on
- [ ] [H] Implement language syntax highlighting inside of diffs
- [ ] [H] Use syntax highlighting inside of the picker appropriate to the original language

I don't know if this a good plan, but for git you could observe the line number metadata, syntax highlight the original file, and then extract the syntax-highlighted line from that and "reconstruct" the diff

Similarly, for syntax highlighting the picker, you could observe the line inside the editor and extract its highlighted version rather than the raw string version.

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
