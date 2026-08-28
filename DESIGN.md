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

jc used to *detect* a session's identity: launch a bare `claude`, write a blank `> uuid=` line into TODO.md, and wait for the first hook event to tell it which UUID the CLI had picked. That single choice paid for a surprising amount of machinery — an `Option<String>` UUID threaded through every session code path, a "one pending UUID-less session per project" constraint so hook events could be matched by `cwd`, and a window where a session existed but could not be addressed by anything.

Claude Code will take the UUID as an argument. So jc mints a v4 UUID itself and launches `claude --session-id <uuid>`, writing the heading and the UUID at the same instant (`SessionState::create`, `Launch::New`). Existing sessions launch `claude --resume <uuid>` (`Launch::Resume`) when their transcript is still on disk. A session is identified from the first moment it exists, `SessionState::uuid` is a plain `String`, and a hook event for an unknown UUID is simply not ours.

The second consequence is that expiry disappears. jc used to mark a session `[X]` when Claude had garbage-collected its `<uuid>.jsonl` transcript, on the theory that it could no longer be resumed, and once marked the session was gone for good.

The theory was half right. Measured 2026-08-27: `claude --resume <uuid>` with no transcript on disk prints `No conversation found with session ID: <uuid>` and exits — so that session genuinely cannot be *resumed*. But `claude --session-id <uuid>` on the very same UUID starts a fresh conversation under it. The transcript being gone costs you the conversation, not the session: the heading, its message log, and its WAIT block are all still in TODO.md, and jc can put a live Claude back behind them.

So the transcript check survives — it is `ProjectState::launch_for`, and it chooses the flag rather than deciding whether to retire a session. `SessionStatus::Expired` and the `[X]` marker are gone: there is no state a session can reach that jc cannot relaunch it from. That also defuses the old failure mode where a live worktree session was mis-detected as expired and retired; the worst a wrong answer costs now is one bad launch flag. The parser still *reads* a legacy `## [X] Label` heading — as `[D]` (disabled), since that is what such a session actually is — and `toggle_session_disabled_at` rewrites the heading, so re-enabling one normalises it. jc never writes `[X]` again.

`/clear` handling is untouched, and is now the only place a session's UUID changes after launch: `SessionEnd(clear)` + `SessionStart(clear)` pair into a `SessionClear` event and `Workspace::handle_session_clear` rewrites `> uuid=` in place.

## Why Not an Editor Plugin

It's tempting to decompose jc into a Zed or nvim plugin + tmux + scripts. Editors already provide editing, syntax highlighting, and terminals.

But jc's value is the *opinionated workflow orchestration* — the thing that makes managing 5 concurrent Claude sessions tractable. The session picker model (Cmd-P across all projects, with per-session activity markers), per-session pane/scroll state that survives a switch, and terminal-as-first-class-view are hard to replicate in an editor that thinks in terms of files and buffers, not Claude conversations. You'd end up rebuilding half of jc inside the editor.

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

Hooks are the one extension point that works well today. Claude Code fires events on prompt submit, stop, permission prompt, idle, and API error. jc's hook server receives these and updates each session's `busy` state; a permission prompt or an API error also raises a desktop notification, but only while the jc window is inactive — those are the two states where Claude is blocked on you and you are not looking. `Stop` and `IdlePrompt` are ambient and deliberately silent: the session picker's activity marker already says which sessions moved, without interrupting whatever you switched to. The same hooks can trigger external notification services (e.g., ntfy, Pushover) for phone alerts when away from the desktop. No skills or context pollution required — hooks are push-only and invisible to the conversation.

## Scheduled Messages

Claude Code enforces usage limits that reset at a wall-clock time. When you hit one
mid-task, the natural move is to queue the next instruction and have it submitted the
moment the limit lifts — otherwise you either babysit the clock or lose the overnight
window entirely. jc handles this with a `@jc(HH:MM)` marker at the top of a WAIT draft:
Cmd-Enter records the message as a `### Message N @jc(<datetime>)` and defers delivery to
that time.

Design choices follow from the use case:

- **The marker lives in TODO.md, not a side table.** jc is already the sole writer of
  TODO.md and treats it as the durable session store, so a scheduled send is just a
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
