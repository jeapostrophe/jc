# Design

## Principles

- **macOS only.** No cross-platform concerns.
- **Rust + GPUI.** Follow Zed's GUI practices where possible.
- **Keyboard-first.** Single-key Emacs-style bindings. Not a vim emulator — just efficient keyboard-driven navigation.
- **Claude Code directly.** Run the real Claude Code CLI in an embedded terminal. Upstream improvements come for free. Hooks provide structured events alongside the raw terminal. Reply capture uses Claude's `/copy` command with clipboard polling.
- **Minimal but functional.** Not a full IDE. Opens files in Zed for serious editing.

## Why Not an Editor Plugin

It's tempting to decompose jc into a Zed or nvim plugin + tmux + scripts. Editors already provide editing, diffs, syntax highlighting, and terminals.

But jc's value is the *opinionated workflow orchestration* — the thing that makes managing 5 concurrent Claude sessions tractable. Problem-driven navigation that crosses terminal/diff/code/TODO boundaries, the session picker model (Cmd-P across all projects with problem badges), and terminal-as-first-class-view are hard to replicate in an editor that thinks in terms of files and buffers, not Claude conversations. You'd end up rebuilding half of jc inside the editor.

## Remote Workflow

Rather than building a custom mobile app, jc relies on Claude Code Remote Control for mobile access. The question is how deeply jc should integrate with Claude Code's extension points (hooks, skills, bang commands) to expose its workflow remotely.

### Why Not a Custom Mobile App

Claude Code Remote Control provides the mobile transport layer — a polished, first-party mobile client that Anthropic will keep improving. Building a custom iOS app + TLS WebSocket server + QR pairing protocol is a large maintenance surface for one developer. Remote Control handles notifications, terminal access, and permission approvals out of the box.

### The Skills & Bang Command Problem

Claude Code offers two extension mechanisms for user-invoked commands:

1. **Skills** (`/skill-name`) — Claude executes a prompt that can include shell commands. The problem: skills cause Claude to *think*. For deterministic operations ("show me the problem list"), thinking is pure waste — tokens spent interpreting intent and generating prose around data you just want printed.

2. **Bang commands** (`!command`) — Run shell commands directly. Closer to what we want, but: (a) namespace collision (`!status` is valuable `$PATH` space), so you'd need `!jc-status` which gets tiresome across 7+ commands; (b) all output enters Claude's context window, consuming tokens. Checking status 10 times in a session fills context with repetitive tabular data Claude doesn't need to see.

The fundamental gap: **there is no Claude Code mechanism for "show the user something without putting it in context."** jc's desktop app solves this by being a separate viewport — you see problems, diffs, and session state without Claude ever knowing you looked. Any pure-Claude-Code solution loses this property.

### What's Worth Implementing Anyway

Despite the limitations, a small subset of CLI subcommands would be useful for scripting, interop, and the occasional Remote Control check-in:

```
jc status              # JSON: projects, sessions, problems
jc problems            # JSON: all problems with ranks
jc note <text>         # Append text below WAIT marker
```

These would cover the most common remote needs. **Currently not implemented** — see [PLAN.md](PLAN.md). The only CLI subcommand available today is `jc clean-hooks`.

### The Missing Primitive

The right solution is a Claude Code feature: **user-side commands that produce output the user sees but Claude does not.** A sideband display channel. This would let tools like jc expose rich status dashboards, problem lists, and session state inside the Claude Code experience without polluting context.

If Claude Code is meant to be the primary developer environment, users need a way to see ambient information (build status, test results, project dashboards) without paying for it in tokens. The analogy is an IDE's status bar or panel — always visible, never in the conversation.

### Hooks

Hooks are the one extension point that works well today. Claude Code fires events on stop, permission prompt, idle, and API error. jc's hook server receives these and updates problem state. The same hooks can trigger external notification services (e.g., ntfy, Pushover) for phone alerts when away from the desktop. No skills or context pollution required — hooks are push-only and invisible to the conversation.

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
- **`PostToolUse`** — Auto-refresh the code view when Claude writes/edits a file in the current project. Auto-refresh diff view after git operations.
- **`SubagentStart`/`SubagentStop`** — Track concurrent subagent work. Show a count of active subagents in the session status bar.
- **`PostCompact`** — Display a notification or marker when context was compacted. Could log the compact summary to the TODO.
- **`TaskCompleted`** — Surface completed tasks in the TODO view or as notifications. Could auto-check items in PLAN.md.
- **`PreCompact`** — Inject custom instructions before compaction to preserve project-specific context.
- **`InstructionsLoaded`** — Track which CLAUDE.md files are active, useful for debugging instruction precedence.
