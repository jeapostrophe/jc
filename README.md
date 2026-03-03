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
- **Project init:** When a project is first added to jc, the app scans for existing JSONL files. If any exist, the most recent session's slug is adopted and a `## Session` heading is written into TODO.md. If none exist, the app launches Claude Code fresh, waits for the JSONL file to appear, extracts the slug, and writes the heading.
- **New session:** From the picker, the user selects "New session." The app launches a fresh Claude Code instance, discovers the resulting slug from the new JSONL file, and inserts a `## Session <slug>: <label>` heading into TODO.md (prompting for a label).
- **Adopt existing:** The picker also lists discovered slugs from the JSONL directory that aren't yet in TODO.md, so the user can adopt an orphaned or external session.

To find all JSONL files for a slug, scan the session directory and match on the `slug` field. The most recently modified file in the group is the "active" one.

### Session Architecture

The app uses an `App -> Projects -> Sessions` hierarchy. Each `ProjectState` owns a TODO file, diff view, code view, and a list of `SessionState` entries. Each `SessionState` has a slug and owns a Claude terminal (resumed via `--resume <uuid>`), a general terminal, and a reply view pre-bound to the slug. The workspace has an active project with an active session; the active session drives which terminals and reply view are shown in the panes. Switching sessions swaps the pane contents without disconnecting terminals.

Key design points:
- Separate terminal instances per session (switching sessions does not disconnect terminals)
- Session state derived from TODO.md `## Session` headings, not persisted separately
- `Problem { rank, description }` tracks validation issues (invalid slugs, dirty working directory) — extensible for future checks
- Session picker (Cmd-P) shows all sessions across all projects with `>` for active and `!` for sessions with problems
- Title bar shows `project > session` with problem indicators

Checklist:
- [x] [D] Implement App -> Projects -> Sessions hierarchy (active project, active session tracking)
- [x] [H] Per-session terminal pairs with shared TODO file
- [x] [H] Session picker to switch active session within a project
- [x] [E] Track and display "problems" (invalid slugs, dirty buffers, dirty working directories, etc.) in status bar

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
2. Create additional sessions from the picker ("New session" launches Claude Code, discovers the slug, inserts the heading). Or adopt an existing orphaned session.
3. Use the fuzzy picker to switch between projects and sessions.

## Research Findings

### GPUI (GUI Framework)

**Verdict: Go.** Published on crates.io as `gpui` v0.2.2. Usable standalone --- 38+ community projects exist (Loungy, Longbridge Pro, etc.). Metal rendering on macOS targets 120 FPS.

**Key asset:** `gpui-component` (Longbridge, 10.3k GitHub stars) provides 60+ production-ready widgets including a code editor component, 20+ themes, buttons, inputs, lists, tables. This dramatically reduces UI work.

**Risks:** Pre-1.0 with breaking changes. Zed deprioritized standalone GPUI development in late 2025 --- a community fork `gpui-ce` exists (480 stars) as a hedge. Pin to a specific version and use `gpui-component` for standard widgets.

**Alternatives considered:** Iced (safer but less performant), egui (simpler but immediate-mode), Slint (stable but less flexible).

### Terminal Emulator

**Verdict: Use `alacritty_terminal`.** It is the only production-quality Rust terminal emulator state machine available as a standalone library. Zed uses a fork pinned to a specific commit (`github.com/zed-industries/alacritty`).

**Claude Code compatibility:** Full support for all escape sequences Claude Code uses (truecolor, alternate screen, SGR styling, synchronized updates, mouse modes, Kitty keyboard protocol).

**GPUI integration proven:** The zTerm project (github.com/zerx-lab/zTerm) demonstrates standalone GPUI + alacritty_terminal integration. Key lesson: use a ~4ms batching interval to coalesce PTY events before rendering. Integration requires ~500-1000 lines of glue code (custom `Element` that paints terminal cells, input routing, PTY management).

**Current approach:** Using crates.io `alacritty_terminal` v0.25.1 + `portable-pty` v0.9.0 in the `jc-terminal` crate. PTY reading runs on a dedicated std::thread, with bytes forwarded via flume channel to a GPUI async task that processes VTE escape sequences and triggers re-renders. Rendering uses GPUI's `canvas()` element to paint terminal cells directly.

### tree-sitter

**Verdict: Go.** v0.26.6, actively maintained. Rich grammar ecosystem with crates for all major languages (Rust, Python, JS/TS, Go, C/C++, Java, Markdown, etc.).

**Architecture:** Follow Zed's query-driven model. Define `highlights.scm` and `outline.scm` per language. Use `tree-sitter-highlight` for syntax highlighting event streams, custom `outline.scm` queries for symbol navigation with hierarchy context (e.g., which `impl` block a method belongs to). Skip `tree-sitter-tags` --- custom queries give more control.

**Performance:** Initial parse ~80ms for a 6K-line file. Incremental re-parse sub-millisecond. Fast enough for keystroke-level responsiveness.

**Gotcha:** Version coupling between `tree-sitter` core and grammar crates. Pin all tree-sitter-related crates carefully. Use `tree-sitter-language` `LanguageFn` pattern to avoid conflicts.

### Markdown Editor

**Verdict: Use `gpui-component` editor widget.** This provides a rope-backed (`ropey`) code editor with tree-sitter integration, line numbers, search, soft/hard wrap, and multi-cursor support out of the box.

**Approach:** Configure with `tree-sitter-md` for markdown highlighting, then add a custom post-processing pass for TODO.md-specific constructs (`### WAIT` markers, `### Message N` headers, `* <file>:<line> --- comment` annotations). Estimated effort: 2-4 weeks vs 6-12 weeks for building from scratch.

**Prior art:** Aster (github.com/kumarUjjawal/aster) is a GPUI-based markdown editor using the same ropey + tree-sitter-md stack. Good reference implementation.

### git2

**Verdict: Go.** v0.20.4, actively maintained (used by Cargo itself). Vendored libgit2 avoids macOS system library conflicts.

**Diff:** Full line-level diff API via `diff_tree_to_workdir_with_index()`. Supports unified, raw, name-only formats. Context lines, whitespace options, patience/minimal algorithms, rename detection all available. For **word-level inline highlighting** within changed lines, supplement with `similar` (ergonomic) or `imara-diff` (30x faster, used by Helix).

**Worktrees:** Complete API --- create (`repo.worktree()`), list (`repo.worktrees()`), validate, lock/unlock, prune. Sufficient for our session management model.

**Shell out to git CLI for:** clone, fetch, gc, shallow clones, sparse checkout, hook execution. These operations are either unsupported or significantly slower in libgit2.

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

### Claude Reply Viewer

**Verdict: Read session JSONL files directly.** No extraction step needed --- render in-app from the JSONL source of truth.

**JSONL format:** Claude Code persists all conversations to `~/.claude/projects/<encoded-path>/<session-uuid>.jsonl`. Each line is a JSON object with a `type` field (`user`, `assistant`, `file-history-snapshot`, etc.). Messages link via `parentUuid`/`uuid` fields. Assistant messages contain content block arrays with `type: "text"`, `type: "thinking"`, and tool use blocks. Every message carries a `slug` field (e.g., `"encapsulated-swimming-firefly"`) that is stable across session forks.

**Session forking:** Claude Code forks sessions when transitioning between modes (e.g., plan to execution) or on `/clear` + resume. A forked session is a new JSONL file whose first `user` entry references the parent session via `sessionId` (parent's ID) and `parentUuid` (fork point in parent). All files in a fork group share the same `slug`. About half of all sessions have at least one fork.

**Turn grouping:** A turn = one `user` message + all subsequent non-user messages until the next `user` message. This groups the request with all of Claude's responses, tool calls, and thinking into a single reviewable unit. For forked sessions, turns are loaded from all JSONL files sharing the same slug, ordered by timestamp.

**Path encoding:** Slashes become hyphens, e.g., `/Users/jay/Dev/project` becomes `-Users-jay-Dev-project`.

**Session discovery:** Scan `.jsonl` files in the encoded-path directory, extract the `slug` from each, and group files by slug. The slug of the most recently modified file becomes the default session for a new project. Sessions are defined in TODO.md via `## Session <slug>: <label>` headings.

**Performance:** JSONL files are append-only. A long session might be a few MB. Full re-read on file change is well under 100ms. File watching via `notify` (same as CodeView) detects updates while Claude is working.

### TLS Server (Mobile Connection)

**Verdict: `axum` + `axum-server` (tls-rustls) + `rcgen`.** Pure Rust, no system dependencies, WebSocket-over-TLS built in.

- `rcgen` generates self-signed certs at pairing time with IP address SANs
- `axum-server` binds with `bind_rustls()` using the generated cert
- `axum` serves both REST and WebSocket over TLS from a single binding
- Mobile app pins the cert's SHA-256 public key fingerprint (received via QR code)

**QR pairing payload:** `{"host": "192.168.1.x", "port": 8443, "fp": "sha256/base64fingerprint=="}` --- fits easily in a Version 10-15 QR code.

### QR Code Generation

**Verdict: `fast_qr`.** 6-7x faster than alternatives (though all are fast enough). Clean API with direct matrix access via public `data` field --- ideal for rendering as GPUI quads. Default ECL Q (25% recovery) is right for screen-to-camera scanning. Zero image dependencies when used with `default-features = false`.

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

## Task Checklist

> **Difficulty labels** — applied to each unchecked task:
> - **[T]** Trivial — All trivial tasks can be done together in one Claude invocation
> - **[E]** Easy — All easy tasks in the same sub-list can be done together in one Claude invocation
> - **[H]** Hard — Each hard task needs its own Claude invocation but requires no human design input
> - **[D]** Design — Subtle design issues that need to be resolved with a human first
> - **[?]** Unclassified — Needs triage. When you encounter a `[?]` task, read the task description, examine the relevant code, and replace `[?]` with the correct label (`[T]`/`[E]`/`[H]`/`[D]`). Do this before starting any other work.
>
> *When adding new checklist items, always include a `[T]`/`[E]`/`[H]`/`[D]`/`[?]` label after the checkbox. If the item doesn't fit under an existing section, create a new `###` section for it.*

### Core Infrastructure
- [x] Set up Rust workspace (`jc-app`, `jc-core`)
- [x] Integrate `gpui` 0.2.x + `gpui-component` and get a basic window rendering
- [x] Implement `~/.config/jc/` config and state persistence
- [x] Implement project and task data model
- [x] Implement `jc` CLI for adding projects from the command line
- [x] [T] Remove code_editor_demo
- [x] [E] Stringly-typed language names vs enum (`Language` enum in `language.rs`)
- [x] [E] Move the crates out of './crates' and into the top-level
- [x] [E] Starting with no project should initialize to a project in the cwd

### Terminal Emulator
- [x] Integrate `alacritty_terminal` with GPUI rendering (`jc-terminal` crate)
- [x] Implement PTY management (`portable-pty`) with background reader thread
- [x] Implement keystroke-to-bytes conversion (special keys, Ctrl, Alt, APP_CURSOR mode)
- [x] Implement terminal cell painting (backgrounds, text with bold/italic, cursor shapes)
- [x] Implement terminal resize detection and PTY resize propagation
- [x] Extract terminal color palette to a theme file (`~/.config/jc/theme.toml`)
- [x] Run general-purpose shell in a second terminal per task
- [ ] [T] Verify whether Claude Code emits BEL (`\x07`) on task completion in terminal mode (replaces earlier `preferredNotifChannel` assumption)
- [x] [E] Change the font size of the terminal(s) via font size keybindings (Cmd-+/-/0)
- [ ] [H] Configure Claude Code hooks (HTTP endpoint) for idle/permission detection
- [x] [E] Run Claude Code inside the embedded terminal dedicated to Claude
- [x] [E] Fix the working directory of the terminals
- [x] [D] ~~Claude appears to create new session_ids "behind the scenes" when it enters and leaves planning mode~~ Resolved: Claude forks sessions on mode transitions. All forks share the same `slug`. Use slug as stable identifier.
- [x] [D] ~~Add `session_slug` to Task model and persist in `state.toml`~~ Superseded: session state lives in TODO.md
- [x] [D] ~~Consider having session state inside of TODO.md rather than having "Task 1"/etc~~ Resolved: sessions defined via `## Session <slug>: <label>` headings in TODO.md
- [x] [E] Implement session discovery by slug: scan JSONL files, extract slug, group by slug, adopt most recent slug on project init
- [x] [E] Invoke Claude Code with `--resume <session-id>` using the most recent file UUID from the session's slug group

### TODO.md System
- [x] Integrate `gpui-component` editor widget with `tree-sitter-md` for markdown highlighting
- [x] Detect external file modifications (file watcher) and show visual indicator
- [x] [E] Automatically reload when the buffer is not dirty, rather than displaying a message
- [x] [E] Fix focus to center the target in the middle of the screen
- [x] [E] Word wrapping lines to fix the length of lines
- [x] Add custom highlight pass for TODO.md constructs (WAIT markers, Message headers)
- [x] Parse TODO.md format (sessions, messages, WAIT markers)
- [x] Build library for managing TODO.md representation
- [ ] [H] On startup, if TODO.md has no valid sessions (all slugs are invalid or no `## Session` headings exist), discover the most recent JSONL session group and insert a `## Session <slug>: <label>` heading into TODO.md
- [ ] [E] Skip sessions with invalid slugs during `ProjectState::create` instead of creating broken `SessionState` entries (currently creates terminals that can't resume)
- [ ] [D] Implement interactive correction of invalid session slugs in TODO.md (e.g., picker showing discovered slugs to replace an invalid one)
- [ ] [H] Implement comment insertion from other views into the WAIT section
- [ ] [D] Implement "select and send" flow: selection -> new Message heading -> send to terminal -> move WAIT
- [ ] [D] Implement conflict resolution (git-style merge of buffer vs disk)
- [ ] [D] Have a shared place outside of all repositories to have a skill/pattern reference (like the "optimize plan" thing) [Perhaps it shows ~/.claude/jc.md]

### Claude Reply Viewer
- [x] Read session slug from TODO.md; load turns from all JSONL files sharing the slug
- [x] [H] Implement JSONL session parser in `jc-core` (parse messages, group into turns)
- [x] [H] Implement ReplyView (render a turn as Markdown in read-only editor, Cmd-6)
- [x] [E] Implement turn picker (Cmd-Shift-O, newest first, shows request text as preview)
- [x] [E] Implement Cmd-T heading picker within rendered turns
- [x] [E] File watching for JSONL changes (reload turns on update)
- [ ] [E] Watch all JSONL files in the slug group (not just one file) for changes
- [ ] [H] Full conversation rendering: tool use summaries, thinking blocks as additional headings

### Git Diff View
- [x] Render git diff via `git2` with `tree-sitter` syntax-aware highlighting
- [ ] [H] Add word-level inline highlighting via `similar` with background highlights NOT diagnostics
- [x] [E] Word wrapping lines to fix the length of lines
- [x] [H] Implement per-file "mark as reviewed" with collapses
- [x] [E] Annotate which files have been "marked" in the git diff picker
- [x] [H] Show one file at a time rather than raw multi-file output
- [ ] [H] Implement region selection and comment keybinding [should be sensitive to what diff (checksum) is shown]
- [x] [H] Show git log as well to look at older diffs. Provide a picker to scroll through to view. (Cmd-Shift-O)
- [x] [H] Implement language syntax highlighting inside of diffs

### Code Viewer
- [x] Implement syntax-highlighted file viewer (`tree-sitter` + `tree-sitter-highlight`)
- [x] Implement fuzzy file picker (search repo files)
- [x] Implement symbol picker with hierarchy context (custom `outline.scm` queries)
- [x] Implement "open in external editor" keybinding (Cmd-Shift-E)
- [x] [E] Automatically reload when the buffer is not dirty, rather than displaying a message
- [x] [E] Fix focus to center the target in the middle of the screen
- [x] [E] Word wrapping lines to fix the length of lines
- [ ] [D] Implement light editing (basic text modification, not full editor; mostly for inserting comments into documents)

### Window & Pane Management
- [x] Fix: Window doesn't get focused on creation
- [x] Add window keybindings (Cmd+W close, Cmd+M minimize, Cmd+Q quit)
- [x] Implement left/right two-pane layout (resizable, with pane focus tracking)
- [x] Implement pane view switching for terminals (Cmd-1 Claude, Cmd-2 General, Cmd-[/] focus)
- [x] Cmd-[/] overrides InputState indent/outdent when editor is focused (bound in Input context)
- [x] Implement view switching for diff, TODO, code views (Cmd-3/4/5)
- [x] Use a monospace font (Lilex) in code editor views (diff, code, TODO)
- [x] Disable line numbers in code editor views
- [x] [E] Change the size of the split with a keybinding to make even split (Cmd-|)
- [x] [E] Change the font to Lilex (https://lilex.myrt.co/); may require downloading and bundling in a 'data' directory
- [x] [T] Move default theme file to 'data' directory rather than embedding defaults in source
- [ ] [E] Implement independent scroll positions per pane
- [x] [D] Unified theme system tying together UI chrome, terminal palette, and code editor highlighting
- [x] [E] Add a light theme (Tomorrow palette in `light_theme.toml`)
- [x] [E] Auto-switch themes with system dark mode
- [x] [T] Remove Cmd-Shift-T manual theme toggle
- [x] [E] Ensure that the theme is used for all parts of the UI. (Right now, the terminal colors don't match the code view colors)
- [ ] [E] Labels on Diff/Reply/FileViews should be limited to one line, right now they can wrap
- [ ] [H] Implement Quake-style bottom terminal overlay
- [ ] [H] Implement multi-window with shared session state

### Navigation & Pickers
- [x] Implement generic fuzzy picker library shared by multiple pickers
- [x] Implement file picker
- [x] Implement symbol picker
- [x] [E] Improve open_file_picker to use show_picker
- [x] [E] Track most recently visited files and put them at the top of file picker
- [x] [E] Track modified files (from git) and mark them in the file picker
- [x] [E] Annotate which files are recently visited in file picker with an R.
- [x] [H] Implement fuzzy project/session picker with filtering (waiting sessions, same project)
- [x] [H] Use syntax highlighting inside of the picker appropriate to the original language
- [ ] [D] Implement keybinding system (configurable, emacs-style defaults)
- [ ] [E] Implement general searching in all views

### Notifications & Status
- [x] Implement Claude usage algorithm
- [x] Implement configurable working hours for par calculation
- [x] [D] Turn usage into a single number/visualization that shows "par" — `par()` returns `working_pct - limit_pct` differential; `par_status()` classifies Under/Over/On.
- [ ] [E] Usage: pace multiplier (`limit_pct / working_pct` as `0.7x`)
- [ ] [E] Usage: projected remaining working hours at current burn rate
- [ ] [H] Implement Claude usage dashboard: poll OAuth usage API, display 5h/7d %, par calculation
- [ ] [H] Implement local HTTP server to receive Claude Code hook events (Stop, Notification, PermissionRequest)
- [ ] [E] Implement in-app status bar showing waiting sessions (driven by hook events)
- [ ] [E] Jump to next waiting session keybinding
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
- [x] [H] Reduce duplication between CodeView and TodoView (consider having TodoView wrap a CodeView)
- [ ] [H] End-to-end test: full workflow from project creation to Claude review cycle
- [ ] [H] Persistent state: survive app restart without losing session state or terminal sessions [perhaps use 'tmux' behind the scenes]
- [ ] [H] Performance: handle multiple concurrent terminal sessions smoothly
- [ ] [H] Error handling: graceful recovery from Claude crashes, terminal failures, disk issues
- [ ] [H] App bundling: `.app` bundle with `Info.plist`, ad-hoc code signing for notifications + distribution
- [ ] [E] Garbage collect stale `.jc/replies/` files (e.g., on app startup, prune files older than N days)
- [ ] [H] Allow projects to have a special `./status.sh` script that reports problems in the form `file:line - problem`

### Automation
- [ ] [D] Manage automations; i.e. creating sessions and running them automatically

### Unsorted
- [x] [E] A ranking system for problems (Claude questions -> WAIT items on idle Claudes -> Unreviewed Git changes -> etc)
