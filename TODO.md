# TODO

# Claude
## Task 0
### Message 0
Review @README.md

Look at these tasks
- [ ] [E] Implement view switching for reply view
- [ ] [H] Implement session JSONL reader for reply capture (extract assistant messages to `./reply/<id>.md`)
- [ ] [H] Show old replies and plans as well. Provide a picker to scroll through to view. (Cmd-Shift-O)

I want to model this on how the GitDiff viewer works.
1. For each project, there is a sequence of sessions that have a sense of "recency".
2. Inside of each session there is a sequence of messages / replies.
3. Each reply has content (that I think is Markdown)

In the original design, I assumed we'd have to proactive copy the markdown, but since learning that all the content is there, it will be easier.

So, following GitDiff, there will be a ReplyView. It shows a particular reply (the level #3) and allows you to do Markdown style picking with Cmd-T. Then you can use Cmd-Shift-O to open up a picker at the sequence level.

I'm not sure what to do about the session level. I think that each "task" should correspond to a session and if you want a different session you have to create a different task and bind it to that session. When a project is initialized for jc, it will "adopt" whatever the most recent session is. [This will affect how 'claude' is invoked in the ClaudeTerminal]

Please review this design, interview me about it, update the README.md as appropriate, and only after doing all that, we'll make an implementation plan.

--

Answers:
1. When jc runs in a project and there is no existing session, it creates one. If there are sessions, then it uses the most recent. If the user makes another task (which isn't implemented yet), then we will present them with a picker that has a "new" option. The task model will have 'session_id: String': there must always be a session.
2. I am conflicted about this. I assumed I would only show 'text' blocks. But if it easy it might be nice to render the entire conversation. I think the entire conversation should be added to the checklist as future work and right now only do text blocks.
3. My answer might be a resolution to the previous question. Maybe the right thing to do is combine a user request with all messages up until the next user request. So in the file we see "user tool tool text thinking tool text user" and so we batch "user tool tool text thinking tool text" into one block in the sequence and the next block only contains the last "user". (I'm writing these in the order they appear in the file, but remember we'll actually present them in the picker from newest to oldest).
4. I think I want to include each message (re #3) as a different level of the hierarchy, but maybe this should be "faked" by rendering the message stream literally as Markdwon in part 3 so it says "# Request\n<text># Thinking\n<explanation>" and so on.
5. I expect this to be historical, but given that we have file watching set up already, I think responding to those events is possible. I'm worried about performance, but you can set my mind at ease about that. I definitely don't want "full streaming and incremental parsing", but I'm not sure how big these files get.
6. This is related to 2, 3, and 4. Remember, this is not designed to be an alternative UI for Claude, it is specifically about commenting on the things that Claude wrote, said, and did.

---

On the comment format, is it likely that Claude will be able to understand what these 'reply:<turn_index>:line' numbers mean? Remember, we're going to be sending them to Claude. When there was the temporary file, we assumed it would be able to read it. What is likely to happen?

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
