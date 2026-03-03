# TODO

# Claude
## Session xxx: 0
### Message 0
Review @README.md

Add something to the checklist instructions about how if Claude is instructed to add something to the checklist it might require a new section

Now focus on these tasks:
- [ ] [H] Add custom highlight pass for TODO.md constructs (WAIT markers, Message headers, comment annotations)
- [ ] [H] Parse TODO.md format (sessions, messages, WAIT markers)
- [ ] [H] Build library for managing TODO.md representation (ropey-backed)
- [ ] [E] Read session slug from TODO.md; load turns from all JSONL files sharing the slug

This may require revisiting some of the design assumptions. It may require fleshing out some of the project/task-level state and app design.

---

Comment annotations in the ### WAIT section are not strict. They should be inserted like that from other contexts, but they are just free form text. I assume that the TODO system will provide a 'insert_comment(String)' functionality scoped to the current session to other components and they will deal with arranging the formatting of the comment themselves. Its job is to insert it safely into the right spot

I think that the wavy underlines are really ugly in gpui. I would prefer to use the syntax highlighting system. Tree-sitter provides bolding for headings, but we could supplement that with colors

I do think we should add a notion of a "problem" to the entire project session tracking data-structure where an example "problem" right now is just an invalid session slug.

In your design, the Todo is "in charge" of the active session via the slug, but I think it should be opposite. The overall design should be:

App -> Projects
Project -> TODO file + Sessions
Session -> slug + all the panes and views

And the app has an active project with an active session. That will influence what the various views are showing. The crucial detail is that the TODO file state is shared between all of the sessions (because we expect them all to be modifying it)

I don't know if there should be different instances of the various view objects all at the same time. I assume that there will be because we don't want the terminals to get disconnected.

These details may be beyond the scope of this particular plan. If so, then they should be added to the README before continuing with this plan.

No matter what, the plan should include updating the README with the completed checklist items.

---

The code and tests look correct, but when I load this project and look at its TODO, I don't see any of the custom highlighting, nor do I see any indication that it detects that 'xxx' is an invalid session slug.

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
