# TODO

# Claude
## Session xxx: 0
### Message 0
Review @README.md

Work on the '### Session Architecture' issues. This is a substantial change that puts a lot of components into place for the future.

---

Step 1.
The problem enum isn't quite right. I think everything is too specific at that level. In particular, a file being dirty really just means the code or todo view is dirty. I think the simplest thing is to represent it as:
struct Problem {
  component: ViewEnum, // Is this handled by Claue, Terminal, Diff, File, TODO, etc?
  rank: i8, // A simple rating of importance
  custom: String // A description that the ViewLook at
}
Maybe it would be better to be:
struct Problem {
  rank: i8,
  kind: ViewProblem
}
where
enum ViewProblem {
  Todo(TodoProblem),
  Claude(ClaudeProblem),
  // etc
}

But that might be overkill for now.

Step 2.
The claude terminal needs to be bound to the slug's session too

Eventually sessions will have their own code views and their own unique scroll/selection state for todo and diff. Are we defering that or is the design a mistake?

Step 5.

The indicator for problems should be:
1. Shown in the session picker, like how file pickers show modifications
2. An indicator in the existing title bar next to the "Project > Session" display
3. An indicator in the upper right for how many sessions (other than the focused one) have a problem

Optimize the plan for context window usage and parallelization. Branch and fork the context/tasks into parallel jobs as appropriate.

---

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
