# Design

## Principles

- **macOS only.** No cross-platform concerns.
- **Rust + GPUI.** Follow Zed's GUI practices where possible.
- **Keyboard-first.** Single-key Emacs-style bindings. Not a vim emulator — just efficient keyboard-driven navigation.
- **Claude Code directly.** Run the real Claude Code CLI in an embedded terminal. Upstream improvements come for free. Hooks provide structured events alongside the raw terminal.
- **Minimal but functional.** Not a full IDE. Serious editing happens in your editor of choice, on the same files.

## What Was Removed, and Why

jc used to carry a lot more: a four-layer problem/priority system with a Cmd-; "jump to the most urgent thing" rotation, a Git diff view with per-file "mark reviewed", a Cmd-K comment system that turned a selection into an annotation below WAIT, a Cmd-Shift-K snippet picker over `~/.claude/jc.md`, a Cmd-Shift-C reply capture via `/copy`, a Cmd-Shift-E "open in Zed", and per-project `status.sh` scripts feeding the problem list.

All of it is gone. After using jc for a long time, these turned out to be features that had been built but were not useful in practice. Machinery nobody exercises still has to be maintained, still constrains every refactor, and still has to be understood by whoever reads the code next. Cutting it makes the program simpler, and that is worth more than the options it removed.

Notifications narrowed for the same reason: an app notification is worth it when Claude is *blocked* — a permission prompt or an API error — and not when it merely finished or went idle.

What survives is the part that earns its keep: TODO.md as durable per-session state, cheap switching between sessions across projects, and scheduled sends.

## Assigned Session UUIDs

*This is jc's lifecycle. jcc (**Two Apps** below) launches no `claude` itself; the label-is-not-an-address reasoning holds there too, with the `> cira=` id as the address.*

jc used to *detect* a session's identity: launch a bare `claude`, write a blank `> uuid=` line into TODO.md, and wait for the first hook event to tell it which UUID the CLI had picked. That single choice paid for a surprising amount of machinery — an `Option<String>` UUID threaded through every session code path, a "one pending UUID-less session per project" constraint so hook events could be matched by `cwd`, and a window where a session existed but could not be addressed by anything.

Claude Code will take the UUID as an argument. So jc mints a v4 UUID itself and launches `claude --session-id <uuid>`, writing the heading and the UUID at the same instant (`SessionState::create`, `Launch::New`). Existing sessions launch `claude --resume <uuid>` (`Launch::Resume`) when their transcript is still on disk. A session is identified from the first moment it exists, `SessionState::uuid` is a plain `String`, and a hook event for an unknown UUID is simply not ours.

The second consequence is that expiry disappears. jc used to mark a session `[X]` when Claude had garbage-collected its `<uuid>.jsonl` transcript, on the theory that it could no longer be resumed, and once marked the session was gone for good.

The theory was half right. Measured 2026-08-27: `claude --resume <uuid>` with no transcript on disk prints `No conversation found with session ID: <uuid>` and exits — so that session genuinely cannot be *resumed*. But `claude --session-id <uuid>` on the very same UUID starts a fresh conversation under it. The transcript being gone costs you the conversation, not the session: the heading, its message log, and its WAIT block are all still in TODO.md, and jc can put a live Claude back behind them.

So the transcript check survives — it is `ProjectState::launch_for`, and it chooses the flag rather than deciding whether to retire a session. `SessionStatus::Expired` and the `[X]` marker are gone: there is no state a session can reach that jc cannot relaunch it from. That also defuses the old failure mode where a live worktree session was mis-detected as expired and retired; the worst a wrong answer costs now is one bad launch flag. The parser still *reads* a legacy `## [X] Label` heading — as `[D]` (disabled), since that is what such a session actually is — and `toggle_session_disabled_at` rewrites the heading, so re-enabling one normalises it. jc never writes `[X]` again.

`/clear` handling is untouched, and is now the only place a session's UUID changes after launch: `SessionEnd(clear)` + `SessionStart(clear)` pair into a `SessionClear` event and `Workspace::handle_session_clear` rewrites `> uuid=` in place.

The third consequence is that the heading *label* stops being an address. TODO.md's text operations — sends, WAIT insertion and cursor placement, `@jc(...)` scheduled delivery, the active-session highlight — all used to resolve a session by label, and every label lookup takes the first heading that matches. That was made safe by keeping labels unique: a `unique_label` at each creation point, plus a `dedupe_labels` repair pass over older files. The repair ran **once, at startup**, over a file the user edits by hand — so pasting a second `## New Session` heading mid-session left every send and every scheduled delivery addressing the wrong heading until the next restart. The machinery existed to prevent exactly that failure and did not cover it.

With the UUID always populated, every one of those callers already holds a unique address, so they key on it and the uniqueness machinery is gone. The one heading a UUID cannot name is one an older jc left unbound (empty `> uuid=`), which is addressed by *verified position* instead: the picker carries the document index, and the writer re-checks against the live document that the heading at that index is still unbound and still carries the label the key was taken with, refusing otherwise. That is a continuous check rather than a startup pass, and it is what makes the address survive the window between a picker snapshot and its confirm. `jc-core/src/todo.rs`'s `SessionKey` doc comment is the canonical statement.

## Two Apps, One Codebase

jc is built twice from one codebase (Cira PLAN TMUX-07; Jay, Q-196: *"Try to make it so the code is as shared as possible"*): **jc**, which runs each session's `claude` itself as above, and **jcc**, the client of Cira described next. The binary decides once at start from its own file name, or `JC_FLAVOR` (`jc_core::flavor::Flavor`), and that one value chooses everything the two must not share: the config directory, the hook URL prefix, the TODO file and the id line of its headings, and who runs a session. Everything else (TODO.md editing, sends, schedules, panes, pickers, the terminal) is the same code.

The two exist so Jay can move his projects to Cira one at a time while jc keeps running the rest, so they must run side by side on the same projects without ever acting on one session twice. **Each keeps its own TODO file** (Jay, 2026-11-12: *"We could have jcc use new TODO.md files in the .cira directory adjacent to the scratch file to prevent this conflict problem"*): jc the project's `TODO.md`, jcc `todo.md` beside the Cira project's scratch pad (`$CIRA_HOME/projects/<name>/todo.md`, the store directory as the status read reports it). Moving a session is cutting its block from one file and pasting it into the other; jcc migrates a pasted `> uuid=` heading to Cira only when no `claude` process still runs that conversation, because jc keeps a session running when its heading leaves its file.

An earlier version shared the project's TODO.md and split it by section (`# Claude` for jc, `# Cira` for jcc). Both apps save that file routinely and on their own schedules (a send, a new heading, a label Cira changed), so each save risked writing over the other's: the review of that version found five ways to lose a heading (a save over a change the watcher had not reported yet; the 200 ms after each save in which the watcher ignored every change, the other app's included; a save over a known merge conflict; an in-place write a reader could catch half done; and a signal handler that could hang). Those are fixed for any writer (Saving, below), since Jay, a second jc or any editor can still save the same file; with one writer per file per app, a conflict needs a person editing it.

**Saving** (`jc_core::shared_file`). Every save re-reads the file and three-way merges in whatever someone else saved since the pane last read it; a conflict writes nothing to the file and keeps the pane's text in `<file>.conflict-<secs>` beside it, at the conflict and at every save while it lasts, so an automatic edit made meanwhile (a send) is never only in memory; it ends at a reload, or when the disk returns to what the pane last read. Every failed save shows in the pane, and a save waits a bounded time for another's lock (a filesystem without locks saves unlocked). A change is recognised as the pane's own when the disk holds what the pane last wrote, never by timing. The file is replaced whole (`jc_core::replace`: a temporary file, synced, renamed over it, through a symlink, keeping permissions), and a check just before the rename starts the save over if the file changed after its read; saves by this code also hold a lock on the file's directory.

## jcc: Background Sessions

In jcc each Claude pane is a client of a Claude Code background session, and **Cira owns the session** (Cira DESIGN 12; Jay, Q-154: *"claude <-> claude daemon <-> cira <-> jc & web"* — jc is the native client). Cira starts the job (`claude --bg`), keeps it in the desired mode, stops and removes it; jc asks for each change with `cira session new|mode|stop|rm|label` and attaches the pane with `claude attach <short>` to the job `cira status --json` names. The session outlives jc, so quitting jc stops nothing, and a jc restart attaches again. The terminal pane attaches to the session's shell, which Cira keeps in its own tmux server and which outlives jc (`cira session shell`, asked again at each attach, so a shell ended by `exit` is made again); the pane asks only once it is first shown, so a jc start never resizes the shell on the page. When the ask fails, or tmux keeps failing to attach, jc runs its own shell, with `CIRA_SESSION`, headed "Terminal · not shared", until Jay exits it (replacing a shell at its prompt would lose its directory and variables).

jcc's first version ran the lifecycle itself (start, the `> dangerous` fork, stop, rm), as jc does. With Cira also running sessions that made two processes run `claude` on one job with nothing ordering them — jc forking a session while Cira respawned the old job for a delivery could leave two live jobs on one conversation (read from the code, not measured) — and an edit to `> dangerous` made while jc was closed was never applied (Cira Q-61). With one owner both go: Cira runs one operation per session at a time, and the mode lives in Cira, not in TODO.md (Q-155).

What that leaves jcc, and why each piece is shaped as it is:

- **The heading's address is the id Cira mints** (`> cira=`), which is also `CIRA_SESSION`. The job's short id changes on every mode change, so jc never stores it (no `> bg=` cache, Q-158); it reads it from Cira each time it attaches.
- **Attaching follows Cira, never a remembered short id.** The attach client exits on Ctrl+Z, on Esc from the ← agents view, and when Cira stops or forks the job; attaching resumes a stopped job. So after an exit the pane attaches again only once a status read *begun after the exit* names the job, waits while an operation runs or a fork is due (attaching then would resume the job being replaced), and does not undo a stop made through Cira — it follows the session when something wakes it, or when Jay asks.
- **`[D]` and `[DELETED]` are requests, sent when the marker changes** (remembered per id in jc's `sessions.toml`), so an edit made while jc is closed applies at its next start, and a stale `[D]` never stops a session started again from Cira's page. A level-triggered marker would make jc fight the page: the two-writer bug again.
- **The label is Cira's (Q-159);** jc writes it into the heading when it changes on the page and sends a heading rename to Cira, remembering the label both last agreed on to tell which side moved.
- **Sends wait for a live session.** Typing into a client that is still connecting is lost, so sends wait until the session is live rather than for a fixed grace.

## jcc's Session Marker Comes From State

jc's picker marker counts the batches the Claude terminal parsed since the session was last on screen, with a launch-settle window discounting `claude`'s own startup output. Any output counts, so a ticking "3m 42s" or a spinner frame marks a session, and an attach client repaints the whole screen whenever it connects (Cira SP-03 confirmed the repaint sets the marker exactly as a spinner does). Jay, 2026-11-11: a ticking timer is not activity (Cira PLAN TMUX-06). jc keeps its marker, since it has no Cira to ask.

In jcc the marker comes from the session's state: `!` a permission dialog, `*` a turn finished since the session was last on screen, `~` waiting on background agents, `x` gone. Cira records that state from its own hooks for every session, including the hours jc is not running, and `cira status --json` lists it with when it began, so jc compares that time with when it last switched away from the session. jc's own hooks remain the fallback: for a session Cira does not know (Cira unreachable, or a session started without a `CIRA_SESSION`), and for an API-error stop (`StopFailure`), which Cira has no state for — a stop failure newer than Cira's state wins.

## Why Not an Editor Plugin

It's tempting to decompose jc into a Zed or nvim plugin + tmux + scripts. Editors already provide editing, syntax highlighting, and terminals.

But jc's value is the *opinionated workflow orchestration* — the thing that makes managing 5 concurrent Claude sessions tractable. The session picker model (Cmd-P across all projects, with per-session state markers), per-session pane/scroll state that survives a switch, and terminal-as-first-class-view are hard to replicate in an editor that thinks in terms of files and buffers, not Claude conversations. You'd end up rebuilding half of jc inside the editor.

## Remote Workflow

Rather than building a custom mobile app, jc relies on Claude Code Remote Control for mobile access. The question is how deeply jc should integrate with Claude Code's extension points (hooks, skills, bang commands) to expose its workflow remotely.

### Why Not a Custom Mobile App

Claude Code Remote Control provides the mobile transport layer — a polished, first-party mobile client that Anthropic will keep improving. Building a custom iOS app + TLS WebSocket server + QR pairing protocol is a large maintenance surface for one developer. Remote Control handles notifications, terminal access, and permission approvals out of the box.

### The Skills & Bang Command Problem

Claude Code offers two extension mechanisms for user-invoked commands:

1. **Skills** (`/skill-name`) — Claude executes a prompt that can include shell commands. The problem: skills cause Claude to *think*. For deterministic operations ("show me the session list"), thinking is pure waste — tokens spent interpreting intent and generating prose around data you just want printed.

2. **Bang commands** (`!command`) — Run shell commands directly. Closer to what we want, but: (a) namespace collision (`!status` is valuable `$PATH` space), so you'd need `!jc-status` which gets tiresome across 7+ commands; (b) all output enters Claude's context window, consuming tokens. Checking status 10 times in a session fills context with repetitive tabular data Claude doesn't need to see.

The fundamental gap: **there is no Claude Code mechanism for "show the user something without putting it in context."** jc's desktop app solves this by being a separate viewport — you see session state, notes, and code without Claude ever knowing you looked. Any pure-Claude-Code solution loses this property.

### What's Worth Implementing Anyway

Despite the limitations, a small subset of CLI subcommands would be useful for scripting, interop, and the occasional Remote Control check-in:

```
jc status              # JSON: projects and sessions
jc note <text>         # Append text below WAIT marker
```

These would cover the most common remote needs. **Currently not implemented** — see [PLAN.md](PLAN.md). The only CLI subcommand available today is `jc clean-hooks`.

### The Missing Primitive

The right solution is a Claude Code feature: **user-side commands that produce output the user sees but Claude does not.** A sideband display channel. This would let tools like jc expose rich status dashboards and session state inside the Claude Code experience without polluting context.

If Claude Code is meant to be the primary developer environment, users need a way to see ambient information (build status, test results, project dashboards) without paying for it in tokens. The analogy is an IDE's status bar or panel — always visible, never in the conversation.

### Hooks

Hooks are the one extension point that works well today. Claude Code fires events on prompt submit, stop, permission prompt, idle, and API error. jc's hook server receives these and updates each session's `busy` state (jcc's learns from them that a session is live and which uuid it is on now); a permission prompt or an API error also raises a desktop notification, but only while the jc window is inactive — those are the two states where Claude is blocked on you and you are not looking. `Stop` and `IdlePrompt` are ambient and deliberately silent: the session picker's activity marker already says which sessions moved, without interrupting whatever you switched to. The same hooks can trigger external notification services (e.g., ntfy, Pushover) for phone alerts when away from the desktop. No skills or context pollution required — hooks are push-only and invisible to the conversation.

## Scheduled Messages

Claude Code enforces usage limits that reset at a wall-clock time. When you hit one
mid-task, the natural move is to queue the next instruction and have it submitted the
moment the limit lifts — otherwise you either babysit the clock or lose the overnight
window entirely. jc handles this with a `@jc(HH:MM)` marker at the top of a WAIT draft:
Cmd-Enter records the message as a `### Message N @jc(<datetime>)` and defers delivery to
that time.

Design choices follow from the use case:

- **The marker lives in TODO.md, not a side table.** jc already writes TODO.md and
  treats it as the durable session store, so a scheduled send is just a
  Message heading with an extra token. This gives persistence, restart re-arming, and
  cancel/edit (delete or retype the marker) for free, with no new state file.
- **The body is live, delivered at fire time.** Because the queued text sits in an
  editable block, you refine the instruction right up to delivery — you're scheduling an
  *intent*, not freezing a string.
- **One at a time, all sends blocked while pending.** Two queued messages racing into the
  same Claude is undefined, so a second Cmd-Enter beeps rather than guessing an order.
- **Absolute-time storage + catch-up.** Resolving `HH:MM` to a concrete datetime removes
  "which 07:30?" ambiguity and lets a send that came due while jc was closed fire on the
  next launch — which is exactly the limit-reset scenario.

## Hook Opportunities

Currently used hooks: `prompt-submit`, `stop`, `stop-failure`, `notification` (idle/permission/auth/elicitation), `permission`, `session-start` (source: clear/startup/resume/compact), `session-end` (reason: clear/logout/prompt_input_exit). The hook server correlates `SessionEnd(clear)` + `SessionStart(clear)` pairs to emit a unified `SessionClear` event for `/clear` handling.

Hooks worth exploring:

- **`PreToolUse`** — Show real-time tool activity in the session status (e.g., "Reading src/main.rs", "Running tests"). Could also enforce project-specific tool policies.
- **`PostToolUse`** — Auto-refresh the code view when Claude writes/edits a file in the current project.
- **`SubagentStart`/`SubagentStop`** — Track concurrent subagent work. Show a count of active subagents in the session status bar.
- **`PostCompact`** — Display a notification or marker when context was compacted. Could log the compact summary to the TODO.
- **`TaskCompleted`** — Surface completed tasks in the TODO view or as notifications. Could auto-check items in PLAN.md.
- **`PreCompact`** — Inject custom instructions before compaction to preserve project-specific context.
- **`InstructionsLoaded`** — Track which CLAUDE.md files are active, useful for debugging instruction precedence.
