# TODO

# Claude
## Task 1
> uuid=121d9f8f-f228-475c-ba12-f125050bbeaa
### Message 0
When jc tries to attach to session (uuid) that Claude has garbage collected, it is stuck in memory and can't be removed from the active session list. We have to restarted to get it to forget.

It is also surprising for the user when this happens. Given that we can check the presence of the uuid files, we can be proactive about this and (a) not show them in the picker even if they are in the TODO.md and (b) mark them differently (with an [X])
### Message 1
Is it sufficient to only mark [X] at project load? I don't anticipate restarting jc every day; do we every regularly look at the session dir?

Should 'disabled' and 'expired' be an enum rather than two bools?
### Message 2
Can we modify the Code view (which will be inherited by TODO and Global TODO) to use Treesitter to show an outline bread crumb in the pane label bar for here the user is..

For example, right now the bar says "TODO [+]" but it could say "TODO [+] > Claude > Task 1 > WAIT" ... I think this information is available. We're already displaying this outline with Cmd-Shift-O, but this is just exposing it in a different way.
### Message 3
How is it implemented when the outline text is very long? I want to ensure it fits on one line and that the file name is always shown. Any eliding should happen in the middle

"TODO [+] > Claude > Task 1 > Subtask 8 > Subsubtask 9 > WAIT"

becomes

"TODO [+] > Claude > Task 1 ... > WAIT"

Stylistically, we want only the filename to be bolded, not the entire label.
### Message 4
If I disable the last session in a project, we stay in the project rather than going to a new one. This is a bit odd.
### Message 5
Look at ../dump/published/20260306-context/README.md

I want to write an article that explains how and why jc works the way that it does for interacting quickly with many Claude sessions.

jc is consciously design to optimize the "prepare a plan -> review a plan -> talk to Claude -> review its work -> repeat" cycle without waiting for 7 minutes between Claude interactions or forgetting where you were in a particular project.

The goal of the article is 80% to explain a practice for implementing the context ideas and 20% to promote jc itself as something they should use.

I want it to be short, like that article, but clear.
### Message 6
Good; on "you select the notes", I think we want to say "just press Cmd-Enter and the message is sent"

Also, I think that we need to make it clear that we save the message so you can easily review what you previously sent so you know the context.

Also, the github username is jeapostrophe
### Message 7
- We should plan to have a screenshot, maybe?
- Before I had jc, I tried to do this but it was difficult to manually maintain the TODO.md files, switch between Zed windows without helps or notifications, and remember which session was which terminal tab in Zed. I kind of want to say "How in practice you really do this without jc" and "What the DIY thing fails to deliver"
- Looking at *this* repo... is there any reason not to make it public?
### Message 8
Let's improve the README of this project to make it better as a first read when people come into the project. I think it contains too much information and is not in the order a human would want to read it.

We might need ./ARCH.md and ./DESIGN.md or some other division(s)
### Message 9
Move the task checklist into ./PLAN.md
### Message 10
I put a screenshot in ./screenshot.png ; let's incorporate it into the README and the article.
### Message 11
Nitpick: the caption for the screenshot says "TODO, Claude, diff", but actually it is "Claude, TODO, diff"
### Message 12
Do a WebSearch for articles with similar content or tools that offer similar things. My priors are that most tools are trying to be Cursor, sell to enterprises, and manage "agent teams" rather than optimizing individual contributors. But maybe I'm wrong. In either case, how can we change or strengthen the article based on what we learn?
### Message 13
On the "DIY Version"... on the switching to other notes, I want to say something in the whole section... I optimized Zed with keybindings for rotating the focus, jumping between panes, I had the diff on the left, the TODO on the right, the terminal beneath with tabs for each. I was using the outline picker to jump to the right spot in the TODO, but it felt slow and brittle and I wasn't fast enough and made mistakes.
### Message 14
"So people don't switch" --- They also try to make Claude totally autonmous and interact with it through Github PR review.

Joke call to action in the article or README --- If you have improvements, have your Claude call my Claude. (I don't accept human authored code.)
### Message 15
1. Add a link in the project README to the article; it will be at

https://jeapostrophe.github.io/tech/jc-workflow/

2. How does Github README.md rendering deal with large images? The screenshot is 4064 x 2334
### Message 16
What is the best way to promote this? I have no meaningful Twitter --- https://x.com/jeapostrophe --- or LinkedIn --- https://www.linkedin.com/in/jeapostrophe/ --- presence; but I have a ton of stars on Github --- https://github.com/jeapostrophe --- do I just post it on HackerNews and /r/claudai ; fire and forget?
### WAIT

## M
xxx promotion
## [X] Task 2
> uuid=e662fbbb-b2eb-417c-8f62-5ca8be12e407
### WAIT
## [D] Task 3
> uuid=ce8dfa9f-7a3a-4068-985b-e703062d6cfb
### Message 0
The new terminal code is broken. The terminal defaults to blank and then when there's activity (like new characters) the content flashes in and then disappears
### Message 1
Good, now the git diff code that includes files not in the repo is broken. I have an entire .gitignore'd directory in ../gb.rs/doc/concept-art/db and all 1400+ files/directories in it are being listed as problems in that project
### Message 2
This change didn't work, ../gb.rs still says it has 1412 problems but there are really only about 20 files
### Message 3
I want to do some more performance analysis. I'm noticing what seems like frame drops while typing. I have 9 active sessions all with active Claudes doing things, printing out to the terminal, modifying files, etc.

I'm worried that we have a ton of background processing happening that is taking place in the UI thread and instead we should have a separate UI and background thread so that the UI can always modify and stay responsive, but I'm not positive how the architecture is working.
### Message 4
Review ../common/icons

I want to generate a new icon. I think our current one is bad.

We need to have some concepts. Read the docs and read ../dump/jc-workflow/README.md to get some inspiration

We want to communicate "efficiency" "human context management" etc
### Message 5
I like 1, 2, 5, and 8.
### Message 6
Let's go with key-2 but save switchboard-4 as a back up in this directory

Adjust the icon build process and files to use this image instead
### WAIT

