# TODO

# Claude
## Session delightful-wibbling-pond: Revise the session attach picker to inspect something about the files to give con...
### WAIT
Test
# Notes

Review @README.md

I want to work on the "problems" and "status" concepts.

- [ ] [E] Implement in-app status bar showing waiting sessions (driven by hook events)
- [ ] [D] Expand the concept of "problems" (Claude is asking for permission, a session is idle, there are messages in the wait section that haven't been sent, the project has non-filled-in-checklist items; maybe require new type)
- [ ] [H] Allow projects to have a special `./status.sh` script that reports problems in the form `file:line - problem`

Here's the way I'm imagining the feature:
- Each session has a set of problems
- Each view has different problems that it can "report" or "contribute"
- The Claude view reports problems via notifications: a stop is a problem, waiting for permission, an API error, an idle, etc
- The Terminal view reports BEL as a problem
- The Diff view reports dirty working files that are not reviewed as problems
- The TODO view reports content in the WAIT region that isn't sent as a problem
- The Reply view can't report problems
- The Code view can't report problems
- In a future feature, we'll add the ability for projects to have their own project-level problems that come from a project-specific 'status.sh' script.

- Some of these problems are asynchronously reported, like the Terminal BEL or Claude issues.
- Others have to be regularly polled, like the Diff, Todo, and status.sh problems

- There needs to be a way to resolve problems.
- Some resolutions are "natural" and will be dealt with during the "next" poll.
- The Claude view problem gets resolved when the user interacts after the notification
- The Terminal view problem gets resolved when the user focuses on the terminal
- The Diff problem gets resolved by marking as reviewed
- The TODO problem gets resolved by moving the item out of WAIT
- The status.sh problems stop being returned from the script

I want problems to be displayed in multiple ways:
- A dirty flag on the left of the session label ("Project > Session") if there are any problems
- A count on the right of the session label with a hover to see a list
- A dirty flag and count in the project picker
- A dirty flag and count in the session picker for already-picked sessions
- A count in the upper right (on the left of the usage) of the number of dirty projects with a hover to see a list of the project sessions with problems and their count

In the first draft, problems are just a list of strings, but in a future version there will be a way to jump to the view that will allow that problem to be addressed (i.e. jump to Claude, jump to the TODO, jump to a file, etc.)

I am not sure if this is the best design so I'd like feedback and I'd like to clarify the README.md to explain this feature and its design in more detail.

---

1. Typed enum --- I agree that this is better. Is it better to have a single type of Enums at the JC/Workspace level or is it better to have each "view" have the kind of problems it can produce/handle?
2. Push vs pull --- I agree with your plan
3. Resolution model --- I agree
4. I think the SessionState + ProjectState is the way to go
5. Simplifying the first IMPLEMENTATION PLAN is fine, but the README should discuss and have checklist tasks for the whole design
6. I think this is confused, the point of the typed enum is to know what to do with that kind of problem and if we do per-view problems then the "destination" of it is implicit. The key idea of "external" problems is that they also report file locations to look at, so I don't think we should treat them abstractly or call them external, just call them "file:location"
7. The rank concept --- I think that we can have implicit ranks for everything except the external ones, for those, the format should be "{rank:}?file{:line}?" --- a required file, an optional rank, and an optional line to jump to on visit

---

Build a plan for these problems.
Optimize the plan for context window usage and parallelization. Branch and fork the context/tasks into parallel jobs as appropriate.
Limit scope relative to an expectation of how the context window will grow.

---

I noticed many problems:
* The '!' indicator is underneath the "Project > Session" label, not to its left
* I don't see the number indicator on the right of the session label OR on the left of the usage label
* I don't see the notes in the session or project pickers 

---

* I see

"! 1 jc > delightful-wibbling-pond" 

but I want to see

"! jc > delightful-wibbling-pond 1"

* In the full session picker Cmd-Shift-P, I see

"[check] <red>Unsent wait: delightful-wibbling-pond</red> (delightful-wibbling-pond) 1h ago"

but I want to see

"<red>1<red> delightful-wibbling-pond (delightful-wibbling-pond) 1h ago"

* In the project picker Cmd-P, I want to the '>'s to change to either green checks (no problems) or red numbers (problems)

* Every problem count for a session should show the "session + project" problem total. Right now, it shows the session total (1) but it shuld be the session + project (13)
* The problem count in the upper right hand corner should shown the number of sessions with problems, i.e. 1 (one project with problems), not 12

---

In the pickers, the problem count is '13' and that is too wide, so it gets displayed as
1
3

This should be fixed

--

* In the project picker view, the number should also be read
* I don't like that I have to wait 2s on start up for the first problem update; the first timer should go off right away
* I don't like that the Single-char markers (>, ✓, +, *) and the numbers don't line up, I'd like them to take the same amount of space so that the labels all appear in the same place

---

I don't see these reliably fixed.
* The project picker view (Cmd-P) doesn't show red numbers
* The session picker view (Cmd-Shift-P) has some single chars with no margin: " *jc / NEW" and " +jc / Read README.md"

===

 Scope notes

 - status.sh is deferred — the infrastructure (ScriptProblem type, ProjectProblem::Script variant) is defined but not wired. A future job adds the runner.
 - Hover tooltips and global indicator (upper-right count) are deferred — the problem data is available, display is a follow-up.
 - Problem navigation (jump to view) is deferred — enum variants carry enough info, but the keybinding/routing is future work.
 - Desktop notifications are a separate feature that can consume problem data once it exists.

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
