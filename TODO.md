# TODO

# Claude
## Session tender-enchanting-willow: How does a session get removed from jc's internal tracking? Is it just when it is...
### WAIT
## Session DELETED delightful-wibbling-pond: Revise the session attach picker to inspect something about the files to give con...
### WAIT
Test
# Notes

Review @README.md

Code:
- I think the session state should change from Vec<SessionState> to HashMap<Slug, SessionState>; that would be cleaner and less error prone when making this change
UI:
- Yes, session picker is the place with Cmd-backspace as the key (remember, it is a picker, so delete is already available in the filter)
TODO:
- I think "Remove drives TODO" is best, but it should not delete it, it should mark it in a way that future invocations will ignore it, like "DELETED" or something. This same marking would be undone if the session were attached again.

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
