# TODO

# Claude
## Session delightful-wibbling-pond: Revise the session attach picker to inspect something about the files to give con...
### WAIT
Test
# Notes

Review @README.md

Look at this problem:

- [ ] [D] Have a shared place outside of all repositories to have a skill/pattern reference (like the "optimize plan" thing) [Perhaps it shows ~/.claude/jc.md]

Basically, I have my own personal repository of skills and common phrases that I send to Claude. Right now, I store them in my various TODO files but they aren't consistent across repos. I probably should just make skills of them, but I'm pretty sure that I would forget the name of the command.

I'm thinking of having a file in my ~/.claude that has a list of this notes and /command references. I'm thinking that maybe it would be shown inside the TODO view in the bottom portion of the screen or maybe there would be a picker for the contents of this file which would be formatted like:

```
# Heading
<content>
```

and I'd "pick" the Header and it would splice in the content. An example might be

```
# Commit and then simplify
Commit the recent changes and then run /simplify
```

This "picking" version that would be visible in either Claude or the TODO seems like the best option.


---

Review @README.md

Optimize the plan for context window usage and parallelization. Branch and fork the context/tasks into parallel jobs as appropriate.
Limit scope relative to an expectation of how the context window will grow.

XXX focus on one panel only
XXX the Cmd-N buttons are hard to remember

banana

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
