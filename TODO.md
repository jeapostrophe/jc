# TODO

# Claude
## Session delightful-wibbling-pond: Revise the session attach picker to inspect something about the files to give con...
### WAIT
Test
# Notes

Review @README.md

Make a plan for these issues:
- [ ] [T] Cmd-; problem cycling: navigate problems within current project, end at WAIT-has-content as the submit signal, fall through to TODO editor at WAIT position when no problems remain ("show me what to do next")
- [ ] [E] Cmd-: urgency-sorted session picker: same sessions as Cmd-P but sorted by oldest-unattended-problem first, so Enter jumps to the neediest session

---

Review @README.md

Optimize the plan for context window usage and parallelization. Branch and fork the context/tasks into parallel jobs as appropriate.
Limit scope relative to an expectation of how the context window will grow.

XXX make a skill that uses DrawThings? --- use the DT mcp server other places

---
Review @README.md
I'm working on the mobile app. I know that you cannot do image generation, but can you generate a prompt that I can manually give to a Stable Diffusion based generator to get a high quality mobile and desktop app icon?

---

Review @README.md
We're in the middle of working on the mobile app.
I just learned about Claude Code Remote Control.
It does 90% of what I care about for the mobile side of this and I'm sure that Anthropic will just make it better and better over time.
Convince me that I should cancel the mobile app and just do the desktop version.
Then argument against that argument, so I can weigh the options and decide.

---

Let's focus on
## Remote Workflow: Hooks, Skills & Bang Commands
### Design

I want adversarially think about one issue and one big departure.

Issue:
- Skills cause Claude to think, but we have deterministic jobs to do, so it seems like there is a mismatch there... we want something really explicit to happen, why not just run a command? Because of that assumption, we have the separate desktop app; the only reason we're thinking about skills is because on mobile we have no choice. Is there a way to force Claude to just cheaply run something without thinking?
- Bang commands are the way to do this, because they just run normal shell commands. We can't do things like `!status` because that is desireable $PATH space. We could !jc-status, but will we get tried of that?
- Bang commands also have the problem that they produce output that Claude sees, which may waste token/context space. The jc model of a parallel interaction environment is great. We really want a way to see things in the context of Claude but not have Claude see it. Is that "wastefulness" unwarranted pre-mature optimization?

Departure:
- If these issues could be alleviated, how radical would it be to take everything about JC and make it an external program/data source that is managed by Claude? Most of what JC does is stores content inside of files anyways and is about gathering it and directing to the right place (e.g. the TODO.md file); the main thing that is "resident" is the session multiplexing and the problem jumping... can we imagine (for example) jc as a nvim or Zed plugin that does all of the jumping/etc and manages the terminal interaction (such as it) using nvim/Zed's embedded terminals/tmux?

---

Revise the README to incorporate this debate and its conclusion. I interpret the conclusion as...
+ Skills and bang commands are not perfect and so should not be implemented for EVERY jc pattern.
+ But there _may_ be a small, reasonable subset of commands/skills that should be included; what are they?
+ The nvim/zed plugin is tempting but wrong UNLESS the Zed plugin API could radically change how Zed works, but that's highly doubtful
+ We should produce a really compelling Github Issue requesting that the Claude Code team add "precise commands written from inside of Claude Code that Claude doesn't see"; presumably they (Anthropic) want people to "live" inside of Claude Code and this identifies a gap

---

Read @README.md

Focus on
- [ ] [D] Draft feature request: Claude Code user-side sideband display (output user sees but Claude does not)

Do web research on the actual Claude Code github issue tracker. See if there are issues that Anthrophic has actually responded to and implemented.
Write something that is courteous and clear. Flatter and say that we *want* to live in the Code ui but are missing this one thing; don't be obsequious or absurd.

XXX unified picker for everything with quick filtering once it is open

## Task - New notes

Read @README.md

I have some notes on this project that I want to discuss and add to the checklist. You may not understand these notes, because they are vague; clarify before adding to the checklist.

* Show Claude on submit
* Project vs global next problem
* No problem jump to wait for submit
* Three pane on desktop (vs laptop)
* Cmd help display

---

This is good, we have context, so I want to clarify the details of some of these


  ### Navigation & Pickers
  - [ ] [D] View picker: replace Cmd-1..6 with a picker that places views in the focused pane; This is bound to Cmd-. and shows the names of the different panes and replaces the currently shown pane
  - [ ] [D] Pane layout: support 1/2/3 pane configurations (auto-detect or manual) ; This is bound to Cmd-1/Cmd-2/Cmd-3. If Cmd-1 is pressed, then the current pane takes over the screen. If Cmd-2 or Cmd-3 is pressed, then the other panes appear with equal widths
  - [ ] [H] Keybinding help overlay (Cmd-?): context-sensitive list of available bindings; XXX first draft should be a list that shows everything; a different task will focus to the current context

  ### Problems & Status
  - [ ] [H] Cmd-; problem cycling: navigate problems within current project, end at WAIT-has-content, fall through to TODO WAIT when no problems remain
  - [ ] [H] Cmd-: urgency-sorted session picker: session picker sorted by oldest-unattended-problem first

  ### Workflow
  - [ ] [E] Auto-show Claude terminal on submit: switch focused pane to Claude terminal when sending from TODO; XXX I think it will be better to focus the "other" pane. If Cmd-1 is active, it switches; if Cmd-2, then the other; if Cmd-3, then the left-most not this one

## Task - Usage

Read @README.md

Look at the 'usage' command and its implementation

This seems like an independently usable thing that might be better put in my Claude 'statusline.sh' script --- just shows the par number, the multiplier, and the expected work time.

Does 'statusline.sh' have access to the usage information without an extra API call? If it doesn't could we make a script that does this but does file-based mtime caching so it doesn't constantly make API calls?

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
