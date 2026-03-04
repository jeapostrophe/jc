# jc

A native macOS Rust application for orchestrating multiple Claude Code sessions across projects. It provides a keyboard-driven workflow for managing sessions, reviewing diffs, annotating code, and sending instructions to Claude --- all from a single app.

## Getting Started

```bash
# Build
cargo build -p jc-app

# Run (opens GUI with existing state)
cargo run -p jc-app

# Register a project directory and open GUI
cargo run -p jc-app -- .

# Run the minimal GPUI example
cargo run -p jc-app --example basic_window

# Run the standalone terminal emulator
cargo run -p jc-terminal --example terminal_window
```

Config and state live in `~/.config/jc/` (`config.toml`, `state.toml`, and `theme.toml`).

### Project Structure

```
Cargo.toml                          # workspace root
data/
  dark_theme.toml                   # unified dark theme: terminal palette, UI chrome, syntax (Tomorrow Night)
  light_theme.toml                  # unified light theme: terminal palette, UI chrome, syntax (Tomorrow)
  fonts/                            # bundled Lilex font (Regular, Bold, Italic, BoldItalic)
jc-core/                            # data model + config persistence
  src/lib.rs, config.rs, model.rs, problem.rs, session.rs, theme.rs
jc-terminal/                        # embedded terminal emulator
  src/lib.rs, colors.rs, input.rs, terminal.rs, pty.rs, render.rs, view.rs
  examples/terminal_window.rs
jc-app/                             # binary: CLI + GPUI app
  src/main.rs, app.rs, outline.rs, language.rs, views/{workspace,pane,picker,project_state,session_state,project_view,diff_view,code_view,todo_view,reply_view}.rs
  src/outline_queries/{rust,markdown,python,go,javascript,typescript}.scm
  examples/basic_window.rs
```

## Design Principles

- **macOS only.** No cross-platform concerns.
- **Rust.** Follow Zed's GUI practices (GPUI) where possible.
- **Keyboard-first.** Single key, Emacs-style bindings with modal ideas. Not a full vim emulator, just efficient keyboard-driven navigation.
- **Claude Code directly.** Run the real Claude Code CLI in an embedded terminal so we get upstream improvements for free. Use Claude Code's hooks system (HTTP hooks) and session JSONL files for structured status events and reply capture alongside the raw terminal.
- **Minimal but functional.** Not a full IDE. Opens files in Zed (or another editor) for serious editing.

## Core Concepts

### Projects

A project corresponds to a code repository. The app tracks active projects in `~/.config/jc/`. Projects are added via an in-app command or from the command line with `jc .`.

### Sessions

Each project has one or more sessions. A session represents an ongoing Claude Code conversation, identified by a **slug** --- a human-readable identifier assigned by Claude Code (e.g., `"encapsulated-swimming-firefly"`). Sessions are defined in the project's TODO.md file via `## Session <slug>: <label>` headings. Each session has:
- A Claude Code terminal, resumed via `--resume` using the most recent JSONL file UUID from the slug group
- A general-purpose terminal
- Its own message history and notes within TODO.md

The slug links a session to a group of Claude Code JSONL files in `~/.claude/projects/<encoded-path>/`. When Claude forks a session (e.g., transitioning from planning to execution, or after `/clear` + resume), the new JSONL file shares the same slug as the original. This makes the slug a stable identifier across forks, unlike `session_id` which changes on each fork.

**Session forking mechanism:** When Claude forks, it creates a new JSONL file. The first `user` entry has `sessionId` set to the **parent** session's ID and `parentUuid` pointing to a message UUID in the parent file (the fork point). Subsequent entries use the new file's own `sessionId`. All entries share the same `slug`.

**State model:** `state.toml` holds only a project registry (list of project paths). All session state is derived from TODO.md files. When the user picks between projects and sessions, the picker reads `## Session` headings from each project's TODO.md.

**Creating sessions:** Slugs are assigned by Claude Code, not invented by the user. The app creates sessions programmatically:
- **Project init:** When a project is first added to jc, the app scans for existing JSONL files. If any exist, the most recent session's slug is adopted and a `## Session <slug>: <slug>` heading is written into TODO.md (using the slug as the initial label). If none exist, the app launches Claude Code fresh, waits for the JSONL file to appear, extracts the slug, and writes the heading.
- **New session:** From the slug picker (Cmd-Shift-P), the user selects "NEW." The app launches a fresh Claude Code instance, polls for the new JSONL file, discovers the resulting slug, and inserts a `## Session <slug>: <slug>` heading into TODO.md.
- **Adopt existing:** The slug picker lists discovered slugs from the JSONL directory that aren't yet in TODO.md as "Attach" entries. Selecting one adopts the session, inserting a `## Session <slug>: <slug>` heading.

To find all JSONL files for a slug, scan the session directory and match on the `slug` field. The most recently modified file in the group is the "active" one.

### Session Architecture

The app uses an `App -> Projects -> Sessions` hierarchy. Each `ProjectState` owns a TODO file, diff view, code view, and a list of `SessionState` entries. Each `SessionState` has a slug and owns a Claude terminal (resumed via `--resume <uuid>`), a general terminal, and a reply view pre-bound to the slug. The workspace has an active project with an active session; the active session drives which terminals and reply view are shown in the panes. Switching sessions swaps the pane contents without disconnecting terminals.

Key design points:
- Separate terminal instances per session (switching sessions does not disconnect terminals)
- Session state derived from TODO.md `## Session` headings, not persisted separately
- `Problem { rank, description }` tracks validation issues (invalid slugs, dirty working directory) — extensible for future checks
- **Session picker (Cmd-P):** Shows all adopted sessions across all projects. Format: `project / label    (slug) recency`. The `(slug)` is only shown when the label is ambiguous (appears on more than one session). Markers: `>` for active, `!` for problems.
- **Slug picker (Cmd-Shift-P):** Shows all discovered sessions for the current project plus a "NEW" entry. Format: `project / label    (slug) recency`. Adopted sessions show their TODO label; orphaned sessions show "Attach". Selecting "NEW" launches a fresh Claude instance and auto-detects the slug.
- Title bar shows `project > session` with problem indicators

### TODO.md

Each project has a single TODO.md file. The app is the sole writer; if the file changes on disk, the app detects it and shows a visual indicator (with optional git-style merge).

The file serves dual purposes: freeform project notes and session state. Sessions are defined by `## Session` headings under `# Claude`:

```markdown
# TODO
(freeform notes, project-level planning)

# Claude
## Session encapsulated-swimming-firefly: Refactor auth module
### Message 0
first instruction sent to claude
### Message 1
second instruction
### WAIT
Future notes go here while Claude is working.
These become the basis for the next message.
```

The `### WAIT` marker separates what has been sent from what is being drafted. When the user selects text above `### WAIT` and presses the send key, that text is wrapped in a new `### Message N` heading and sent to the Claude terminal. The `### WAIT` marker moves below it. Unselected text remains as future notes.

The `## Session <slug>: <label>` heading format is parsed by the app. The slug portion must match a valid Claude Code session slug; the app highlights invalid slugs as errors. The label is a freeform description.

### Comment Format

From any view (diff, terminal, code, reply), the user can press a comment keybinding to annotate a region. Comments are appended to the current session's future notes (below `### WAIT`) in these formats:

- **From diff or code view:** `* <file>:<start_line>-<end_line> --- Comment text`
- **From terminal:** `* TERMINAL\n\`\`\`\n[selected content]\n\`\`\`\nComment text`
- **From a Claude reply:** `* .jc/replies/<turn_file>.md:<line> --- Comment text`

## Research

### macOS Notifications

**Verdict: Use `objc2-user-notifications` (modern API).** Since the app will be a bundled `.app` anyway (required by GPUI/Metal), the code-signing requirement is not a burden. This gives us action buttons ("Switch to Session"), notification grouping via `threadIdentifier`, async delegate callbacks, and future-proofing (Apple's current API, not deprecated).

**Fallback:** `mac-notification-sys` v0.6.9 works today without code signing but uses deprecated `NSUserNotificationCenter`. Fine for prototyping.

**Simplest possible:** `osascript -e 'display notification ...'` for zero-dependency MVP.

### Claude Code Idle Detection

**Verdict: Use Claude Code's hooks system.** This is purpose-built for exactly our use case.

**Primary mechanism:** Configure HTTP hooks in `~/.claude/settings.json` that POST to a local port our app listens on:
- `Stop` hook fires when Claude finishes responding (definitive completion signal)
- `Notification` hook with `idle_prompt` matcher fires when waiting for input
- `Notification` hook with `permission_prompt` matcher fires when tool approval needed
- `PermissionRequest` hook fires with full tool details for permission dialogs

**Secondary signal (status unknown):** The `preferredNotifChannel = terminal_bell` setting mentioned in earlier research does not exist in the current Claude Code settings schema (as of March 2026). Claude Code may already emit BEL (`\x07`) on task completion in terminal mode --- needs testing. The `jc-terminal` crate already detects `TerminalEvent::Bell` via alacritty, so if BEL is emitted, no extra configuration is needed.

**Combine with:** Brief silence heuristic as a tertiary fallback (2+ seconds of PTY silence AND a Stop hook = confident idle state).

**Known issues:** `idle_prompt` fires after EVERY response, not just genuine wait states (bug tracked in Claude Code issues). The `Stop` hook is more reliable.

### Claude Usage Dashboard

**Verdict: Poll the undocumented OAuth usage API.** `GET https://api.anthropic.com/api/oauth/usage` returns exactly what we need:

```json
{
  "five_hour": { "utilization": 37.0, "resets_at": "2026-02-08T04:59:59Z" },
  "seven_day": { "utilization": 26.0, "resets_at": "2026-02-12T14:59:59Z" },
  "extra_usage": { "is_enabled": true, "monthly_limit": 5000, "used_credits": 1234, "utilization": 24.68 }
}
```

**Token access:** Read from macOS Keychain via `security find-generic-password -s "Claude Code-credentials" -w`, parse JSON, extract `claudeAiOauth.accessToken`. Poll every 30-60 seconds.

**Par calculation:** Compare `seven_day.utilization` against `(elapsed_working_seconds / total_working_seconds_in_window) * 100`, adjusted for configured working hours.

**Risk:** Undocumented endpoint, could change. A `claude-usage` Rust crate already exists on crates.io. Community tools (bash scripts, Python dashboards) demonstrate the pattern is stable.

## Views & Panels

The window has a **left pane** and a **right pane**. Any view can appear in either pane with independent scroll positions. A **Quake-style terminal** can be toggled at the bottom of the window via a keybinding for quick commands.

Multiple windows can be open simultaneously. Windows share state: if two windows show the same session's Claude terminal, scrolling or changes in one are reflected in the other.

### Claude Terminal

The primary view. Shows the Claude Code CLI running in a terminal emulator. The app detects when Claude stops producing output and shows a notification (desktop notification + in-app visual indicator).

### General Terminal

A separate terminal per session for running arbitrary commands. Tied to the session's working directory (worktree or project root).

### TODO Editor

Source-mode Markdown editing with light rendering (bold, highlights, heading formatting --- not full WYSIWYG). This is where the user drafts notes, reviews accumulated comments, and sends instructions to Claude.

### Git Diff View

Shows `git diff` output for the project's working tree. The user scrolls through changes, highlights regions to comment on, and marks files as reviewed (collapsing them). Comments flow into TODO.md. Should use syntax highlighting, but does not need to perform full LSP-style type annotation.

### Code Viewer

Syntax-highlighted source viewer with light editing capability (not a full editor). Features:
- Fuzzy file picker (Cmd-O, searches repository files)
- Context picker (Cmd-T) powered by tree-sitter `outline.scm` queries with hierarchy-preserving filter --- shows symbols with their parent context (e.g., which `impl` block a function belongs to). Also works in Diff view (pick modified files) and TODO view (pick markdown headers).
- Comment keybinding works here too
- Keybinding to open the file in an external editor (Zed)

### Claude Reply Viewer

Reads the session's Claude Code JSONL files (`~/.claude/projects/<encoded-path>/<session-id>.jsonl`) and presents the conversation as a sequence of **turns**. A turn groups a user request with all subsequent messages (assistant responses, tool use, thinking) until the next user request.

Each turn is rendered as a Markdown document with headings for each message type. Initially only `text` content blocks are rendered:

```markdown
# Request
<user message text>

# Reply
<assistant text content>
```

Full conversation rendering (tool use summaries, thinking blocks as additional headings) is planned as future work.

This rendering approach means Cmd-T (context picker) works automatically via tree-sitter markdown heading queries, and the existing comment keybinding can annotate any region.

When a turn is viewed, its rendered Markdown is written to `.jc/replies/<turn_file>.md` in the project directory (gitignored). Comments reference this file path so Claude can read the file to understand the context. These files are written automatically --- no manual extraction step.

Navigation:
- **Cmd-Shift-O** opens a turn picker (newest first) showing the user's request text as a preview
- **Cmd-T** picks headings within the current turn

The view monitors the JSONL file for changes and reloads when updated (e.g., while Claude is working). JSONL files are append-only so even large sessions (multi-MB) reload quickly.

## Status & Usage Tracking

### Session Status Indicators

A persistent indicator shows which sessions have Claude responses waiting for review. The app detects "waiting" via Claude Code's hooks system (HTTP hooks fire on `Stop` and `Notification` events) with BEL character detection as a lightweight backup signal (if Claude Code emits it).

A fuzzy picker (keybinding) lets the user jump between projects and sessions. A modifier key filters to only waiting sessions or same-project sessions.

### Claude Usage Dashboard

Always visible in a corner. Shows:
- **5-hour window usage %** and time remaining
- **Weekly usage %** and time remaining
- **Par indicator:** compares usage % against elapsed working time %. "Under par" means you have budget to spare; "over par" means you're consuming faster than your pace.
- Configurable working hours (e.g., exclude Sunday, reduce Saturday) so par calculations reflect actual work schedule, not raw calendar time.

## Mobile App

A lightweight companion app that connects to the desktop app over the local network.

### Connection

The desktop app displays a QR code embedding an auth key. The mobile app scans it to pair. Communication uses TLS. For remote access, the user handles tunneling externally (e.g., Tailscale).

### Capabilities

The mobile app is optimized for:
- **Dashboard:** See project/session status, which sessions are waiting, usage stats
- **Reviewing:** Read Claude responses and diffs (not optimized for code editing)
- **Note-taking:** Add comments and notes to TODO.md
- **Permissions:** Handle Claude permission requests (approve/deny tool use)
- **Sending commands:** Send instructions to Claude from the TODO

It is deliberately *not* a full code editor on mobile.

## Architecture & Components

| Component | Approach |
|---|---|
| GUI framework | `gpui` 0.2.x (from Zed) + `gpui-component` (Longbridge, 60+ widgets) |
| Terminal emulator | `alacritty_terminal` 0.25 + `portable-pty` 0.9 --- full escape sequence support for Claude Code's TUI |
| Markdown editor | `gpui-component` editor widget + `ropey` + `tree-sitter-md`, custom TODO.md highlight pass |
| Syntax highlighting | `tree-sitter` 0.25.x (via `gpui-component`) + `tree-sitter-highlight` + per-language grammar crates |
| Symbol navigation | tree-sitter custom `outline.scm` queries (sourced from Zed, updated via `scripts/update-outline-queries.sh`) |
| Git diff | `git2` 0.20.x (vendored libgit2) + `similar`/`imara-diff` for word-level highlighting |
| Git worktrees | `git2` worktree API (create/list/prune) |
| Claude idle detection | Claude Code hooks system (HTTP endpoint) + BEL detection (if emitted) + silence heuristic fallback |
| Claude reply viewer | Parse session JSONL from `~/.claude/projects/`, group into turns, render as Markdown in read-only editor |
| Claude usage dashboard | Poll `GET https://api.anthropic.com/api/oauth/usage` (OAuth token from macOS Keychain) |
| Persistent state | `~/.config/jc/` --- project registry, window layout; session state in TODO.md |
| Mobile server | `axum` + `axum-server` (tls-rustls) + `rcgen` (self-signed certs) |
| Mobile QR pairing | `fast_qr` (ECL Q, render as GPUI quads) |
| Mobile app | Separate project (Swift/native iOS likely) |
| Desktop notifications | `objc2-user-notifications` (modern UNUserNotificationCenter, requires app bundling) |

## Workflow Walkthrough

### Reviewing Claude's Output

1. Claude finishes working. A desktop notification fires and the in-app indicator lights up.
2. Press a keybinding to switch the left pane to Claude's terminal.
3. Press a keybinding to switch to the diff view. Scroll through changes.
4. Highlight a region (with mouse or keybindings) in the diff, press the comment key, type a note. It appears in TODO.md under `### WAIT`.
5. Mark reviewed files as done (they collapse).
6. Switch to the general terminal to run tests or inspect behavior. Highlight output, press comment key.
7. Switch to the reply viewer (Cmd-6). Use Cmd-Shift-O to pick a turn. Scroll through the rendered conversation and annotate.

### Navigating Code

1. Press a keybinding to open the file picker. Fuzzy-search for a filename.
2. The file opens in the code viewer with syntax highlighting.
3. Press a keybinding to open the symbol picker. Type to filter (e.g., "new" shows all functions with "new" in the name, with their `impl` context).
4. Browse the code. Press the comment key to annotate a line --- it flows into TODO.md.
5. Press a keybinding to open the file in Zed for real editing.

### Sending Instructions to Claude

1. Navigate to the TODO editor. Review accumulated comments and notes below `### WAIT`.
2. Rearrange, elaborate, or trim the notes.
3. Select the text to send. Press the send keybinding.
4. The selected text becomes `### Message N`, is sent to the Claude terminal, and `### WAIT` moves below it. Remaining unselected text stays as future notes.

### Managing Projects and Sessions

1. Add a project: run `jc .` from a repo, or use an in-app command. The app discovers or creates a Claude Code session and writes the `## Session` heading into TODO.md.
2. Use the session picker (Cmd-P) to switch between adopted sessions across all projects.
3. Use the slug picker (Cmd-Shift-P) to manage the current project's sessions: switch to an adopted session, attach an orphaned one, or select "NEW" to launch a fresh Claude instance.

## Task Checklist

> **Difficulty labels** — applied to each unchecked task:
> - **[T]** Trivial — All trivial tasks can be done together in one Claude invocation
> - **[E]** Easy — All easy tasks in the same sub-list can be done together in one Claude invocation
> - **[H]** Hard — Each hard task needs its own Claude invocation but requires no human design input
> - **[D]** Design — Subtle design issues that need to be resolved with a human first
> - **[?]** Unclassified — Needs triage. When you encounter a `[?]` task, read the task description, examine the relevant code, and replace `[?]` with the correct label (`[T]`/`[E]`/`[H]`/`[D]`). Do this before starting any other work.
>
> *When adding new checklist items, always include a `[T]`/`[E]`/`[H]`/`[D]`/`[?]` label after the checkbox. If the item doesn't fit under an existing section, create a new `###` section for it.*

### Terminal Emulator
- [x] [T] Verify whether Claude Code emits BEL (`\x07`) on task completion in terminal mode — verdict: skip, using hooks instead
- [x] [H] Configure Claude Code hooks (HTTP endpoint) for idle/permission detection

### TODO.md System
- [ ] [D] Implement conflict resolution (git-style merge of buffer vs disk)
- [ ] [D] Have a shared place outside of all repositories to have a skill/pattern reference (like the "optimize plan" thing) [Perhaps it shows ~/.claude/jc.md]

### Claude Reply Viewer
- [ ] [H] Full conversation rendering: tool use summaries, thinking blocks as additional headings

### Git Diff View
- [ ] [H] Add word-level inline highlighting via `similar` with background highlights NOT diagnostics

### Window & Pane Management
- [ ] [E] Implement independent scroll positions per pane
- [ ] [H] Implement Quake-style bottom terminal overlay
- [ ] [H] Implement multi-window with shared session state

### Navigation & Pickers
- [ ] [D] Implement keybinding system (configurable, emacs-style defaults)

### Notifications & Status
- [x] [H] Implement local HTTP server to receive Claude Code hook events (Stop, Notification, PermissionRequest)
- [ ] [E] Implement in-app status bar showing waiting sessions (driven by hook events)
- [ ] [E] Jump to next problem keybinding on Cmd-;
- [ ] [H] Implement macOS desktop notifications via `objc2-user-notifications` (action buttons: "Switch to Session")
- [ ] [D] Expand the concept of "problems" (Claude is asking for permission, a session is idle, there are messages in the wait section that haven't been sent, the project has non-filled-in-checklist items; maybe require new type)

### Mobile App
- [ ] [D] Design mobile app protocol (WebSocket messages: status updates, TODO edits, permission requests, commands)
- [ ] [H] Implement TLS server (`axum` + `axum-server` + `rcgen` self-signed certs)
- [ ] [H] Implement QR code pairing flow (`fast_qr`, encode host + port + cert fingerprint)
- [ ] [H] Build mobile app: dashboard view
- [ ] [H] Build mobile app: TODO review and note-taking
- [ ] [H] Build mobile app: Claude permission handling (relay from hooks)
- [ ] [H] Build mobile app: send commands to Claude

### Git Worktrees
- [ ] [H] Implement git worktree creation/deletion via `git2` worktree API

### Polish & Integration
- [ ] [H] End-to-end test: full workflow from project creation to Claude review cycle
- [ ] [H] Persistent state: survive app restart without losing session state or terminal sessions [perhaps use 'tmux' behind the scenes]
- [ ] [H] Performance: handle multiple concurrent terminal sessions smoothly
- [ ] [H] Error handling: graceful recovery from Claude crashes, terminal failures, disk issues
- [ ] [H] App bundling: `.app` bundle with `Info.plist`, ad-hoc code signing for notifications + distribution
- [ ] [H] Allow projects to have a special `./status.sh` script that reports problems in the form `file:line - problem`

### Code Quality
- [ ] [E] Lazy-highlight `LineSearchPickerDelegate::build()` — currently does O(N) syntax highlighting of every line on each Cmd-F; will lag on very large files
- [ ] [E] Collapse the four `LineSearchPickerDelegate::for_*_view` factories into one generic method via a shared trait (editor_text + scroll_to_line + language_name)

### Automation
- [ ] [D] Manage automations; i.e. creating sessions and running them automatically
