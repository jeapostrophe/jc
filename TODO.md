# TODO

# Claude
## Session delightful-wibbling-pond: Revise the session attach picker to inspect something about the files to give con...
### WAIT
Test
# Notes

Review @README.md

Plan the iOS part of phase 1 of the mobile app.

---

Two things:
* The paired mode should have an unpair path

*  Ideally, I will have a build script that will make it so I don't have to use Xcode at all. I don't know if this is possible. If it is, the interface I want is:

./make.sh 
--> calls ./jc-mobile/make.sh
[eventually it will also do the desktop bundling, but not yet]

./jc-mobile/make.sh
--> calls all the command line versions of xcode build /etc

./jc-mobile/make.sh deploy
--> builds and uploads to my connected phone (I already do this in Xcode with a different app, but I don't like opening up Xcode for it)

Is this possible? Can it be included in the plan?

---

I deployed, restarted the app, and the "disconnected" changed to "connection failed" with no message. The desktop console read

mobile client error: IO error: unexpected end of file
mobile client error: IO error: unexpected end of file

Then I unpaired and paired again, the mobile had "Connecting..." spinning for a long time and the desktop printed

mobile client error: IO error: unexpected end of file

Then eventually the mobile showed a new error.

---

I ran and saw the same things in the desktop and app, the logs were:
❯     xcrun devicectl device process launch --console --device FB7C82EB-5C1A-549D-B981-8D054CF9A584 dev.jc.jc-mobile

03:12:04  Acquired tunnel connection to device.
03:12:04  Enabling developer disk image services.
03:12:04  Acquired usage assertion.
Launched application with dev.jc.jc-mobile bundle identifier.
Waiting for the application to terminate…

---

Review @README.md

Optimize the plan for context window usage and parallelization. Branch and fork the context/tasks into parallel jobs as appropriate.
Limit scope relative to an expectation of how the context window will grow.

XXX focus on one panel only
XXX the Cmd-N buttons are hard to remember
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
