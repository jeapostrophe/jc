# jc

A native macOS Rust application for orchestrating multiple Claude Code sessions across projects. It provides a keyboard-driven workflow for managing tasks, reviewing diffs, annotating code, and sending instructions to Claude --- all from a single app.

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

# Run the code editor demo (InputState code_editor widget)
cargo run -p jc-app --example code_editor_demo

# Run the standalone terminal emulator
cargo run -p jc-terminal --example terminal_window
```

Config and state live in `~/.config/jc/` (`config.toml`, `state.toml`, and `theme.toml`).

### Project Structure

```
Cargo.toml                          # workspace root
crates/
  jc-core/                          # data model + config persistence
    src/lib.rs, config.rs, model.rs, theme.rs
  jc-terminal/                      # embedded terminal emulator
    src/lib.rs, colors.rs, input.rs, terminal.rs, pty.rs, render.rs, view.rs
    examples/terminal_window.rs
  jc-app/                           # binary: CLI + GPUI app
    src/main.rs, app.rs, outline.rs, views/{workspace,pane,picker,project_view,diff_view,code_view,todo_view}.rs
    src/outline_queries/{rust,markdown,python,go,javascript,typescript}.scm
    examples/basic_window.rs, code_editor_demo.rs
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

### Tasks

Each project has one or more tasks. A task represents a unit of work being done, sharing the project's main working tree. Git worktree support (giving each task its own isolated tree) is planned for later. Each task has:
- A Claude Code session (terminal)
- A general-purpose terminal
- Access to the shared project TODO.md

### TODO.md

Each project has a single TODO.md file shared across all its tasks. The app is the sole writer; if the file changes on disk, the app detects it and shows a visual indicator (with optional git-style merge).

The format tracks messages sent to each agent:

```markdown
# TODO
(freeform notes, task planning)

# Claude
## Agent 1
### Message 0
first instruction sent to claude
### Message 1
second instruction
### WAIT
Future notes go here while Claude is working.
These become the basis for the next message.
```

The `### WAIT` marker separates what has been sent from what is being drafted. When the user selects text above `### WAIT` and presses the send key, that text is wrapped in a new `### Message N` heading and sent to the Claude terminal. The `### WAIT` marker moves below it. Unselected text remains as future notes.

### Comment Format

From any view (diff, terminal, code, reply), the user can press a comment keybinding to annotate a region. Comments are appended to the current agent's future notes (below `### WAIT`) in these formats:

- **From diff or code view:** `* <file>:<start_line>-<end_line> --- Comment text`
- **From terminal:** `* TERMINAL\n\`\`\`\n[selected content]\n\`\`\`\nComment text`
- **From a Claude reply:** `* ./reply/<id>.md:<line> --- Comment text`

## Views & Panels

The window has a **left pane** and a **right pane**. Any view can appear in either pane with independent scroll positions. A **Quake-style terminal** can be toggled at the bottom of the window via a keybinding for quick commands.

Multiple windows can be open simultaneously. Windows share sessions: if two windows show the same task's Claude terminal, scrolling or changes in one are reflected in the other.

### Claude Terminal

The primary view. Shows the Claude Code CLI running in a terminal emulator. The app detects when Claude stops producing output and shows a notification (desktop notification + in-app visual indicator).

### General Terminal

A separate terminal per task for running arbitrary commands. Tied to the task's working directory (worktree or project root).

### TODO Editor

Source-mode Markdown editing with light rendering (bold, highlights, heading formatting --- not full WYSIWYG). This is where the user drafts notes, reviews accumulated comments, and sends instructions to Claude.

### Git Diff View

Shows `git diff` output for the task's working tree. The user scrolls through changes, highlights regions to comment on, and marks files as reviewed (collapsing them). Comments flow into TODO.md. Should use syntax highlighting, but does not need to perform full LSP-style type annotation.

### Code Viewer

Syntax-highlighted source viewer with light editing capability (not a full editor). Features:
- Fuzzy file picker (Cmd-O, searches repository files)
- Context picker (Cmd-T) powered by tree-sitter `outline.scm` queries with hierarchy-preserving filter --- shows symbols with their parent context (e.g., which `impl` block a function belongs to). Also works in Diff view (pick modified files) and TODO view (pick markdown headers).
- Comment keybinding works here too
- Keybinding to open the file in an external editor (Zed)

### Claude Reply Viewer

When Claude produces a long response, a keybinding extracts it from the session JSONL transcript (`~/.claude/projects/.../<session>.jsonl`) and writes the Markdown result to a temporary file (`./reply/<id>.md`). The user scrolls through it and can annotate inline. Modifications are tracked and written as comments into TODO.md.

## Status & Usage Tracking

### Task Status Indicators

A persistent indicator shows which tasks have Claude responses waiting for review. The app detects "waiting" via Claude Code's hooks system (HTTP hooks fire on `Stop` and `Notification` events) with `terminal_bell` as a lightweight backup signal.

A fuzzy picker (keybinding) lets the user jump between projects and tasks. A modifier key filters to only waiting tasks or same-project tasks.

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
- **Dashboard:** See project/task status, which tasks are waiting, usage stats
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
| Claude idle detection | Claude Code hooks system (HTTP endpoint) + `terminal_bell` config + silence heuristic fallback |
| Claude reply capture | Read session JSONL from `~/.claude/projects/` (replaces fragile `/copy` approach) |
| Claude usage dashboard | Poll `GET https://api.anthropic.com/api/oauth/usage` (OAuth token from macOS Keychain) |
| Persistent state | `~/.config/jc/` --- project list, task state, window layout |
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
7. If Claude's textual response was long, press a key to extract it from the session JSONL transcript into a reply file. Scroll and annotate.

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

### Managing Projects and Tasks

1. Add a project: run `jc .` from a repo, or use an in-app command.
2. Create a task within a project. Choose whether it gets its own git worktree or shares the main tree.
3. Use the fuzzy picker to switch between projects and tasks.

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

**Worktrees:** Complete API --- create (`repo.worktree()`), list (`repo.worktrees()`), validate, lock/unlock, prune. Sufficient for our task management model.

**Shell out to git CLI for:** clone, fetch, gc, shallow clones, sparse checkout, hook execution. These operations are either unsupported or significantly slower in libgit2.

### macOS Notifications

**Verdict: Use `objc2-user-notifications` (modern API).** Since the app will be a bundled `.app` anyway (required by GPUI/Metal), the code-signing requirement is not a burden. This gives us action buttons ("Switch to Task"), notification grouping via `threadIdentifier`, async delegate callbacks, and future-proofing (Apple's current API, not deprecated).

**Fallback:** `mac-notification-sys` v0.6.9 works today without code signing but uses deprecated `NSUserNotificationCenter`. Fine for prototyping.

**Simplest possible:** `osascript -e 'display notification ...'` for zero-dependency MVP.

### Claude Code Idle Detection

**Verdict: Use Claude Code's hooks system.** This is purpose-built for exactly our use case.

**Primary mechanism:** Configure HTTP hooks in `~/.claude/settings.json` that POST to a local port our app listens on:
- `Stop` hook fires when Claude finishes responding (definitive completion signal)
- `Notification` hook with `idle_prompt` matcher fires when waiting for input
- `Notification` hook with `permission_prompt` matcher fires when tool approval needed
- `PermissionRequest` hook fires with full tool details for permission dialogs

**Secondary signal:** Set `preferredNotifChannel` to `terminal_bell` --- Claude emits BEL character (`\x07`) on task completion, easily detected from PTY monitoring.

**Combine with:** Brief silence heuristic as a tertiary fallback (2+ seconds of PTY silence AND a Stop hook = confident idle state).

**Known issues:** `idle_prompt` fires after EVERY response, not just genuine wait states (bug tracked in Claude Code issues). The `Stop` hook is more reliable.

### Claude Reply Capture

**Verdict: Read session JSONL files.** The `/copy` approach is fragile (interactive picker for code blocks, clipboard race conditions). Better alternatives:

**Primary:** Claude Code persists all conversations to `~/.claude/projects/<encoded-path>/<session-uuid>.jsonl`. Each line is a JSON object with types `user`, `assistant`, `tool_use`, `tool_result`. Extract the latest assistant message(s) and write as Markdown to `./reply/<id>.md`.

**Path encoding:** Slashes become hyphens, e.g., `/Users/jay/Dev/project` becomes `-Users-jay-Dev-project`.

**Alternative considered:** Agent SDK provides structured streaming output, but we chose to run the real Claude Code CLI for upstream improvements.

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

### Core Infrastructure
- [x] Set up Rust workspace (`jc-app`, `jc-core`)
- [x] Integrate `gpui` 0.2.x + `gpui-component` and get a basic window rendering
- [x] Implement `~/.config/jc/` config and state persistence
- [x] Implement project and task data model
- [x] Implement `jc` CLI for adding projects from the command line
- [ ] Remove code_editor_demo
- [ ] Stringly-typed language names vs enum
- [ ] Move the crates out of './crates' and into the top-level

### Terminal Emulator
- [x] Integrate `alacritty_terminal` with GPUI rendering (`jc-terminal` crate)
- [x] Implement PTY management (`portable-pty`) with background reader thread
- [x] Implement keystroke-to-bytes conversion (special keys, Ctrl, Alt, APP_CURSOR mode)
- [x] Implement terminal cell painting (backgrounds, text with bold/italic, cursor shapes)
- [x] Implement terminal resize detection and PTY resize propagation
- [x] Extract terminal color palette to a theme file (`~/.config/jc/theme.toml`)
- [x] Run general-purpose shell in a second terminal per task
- [ ] Configure `preferredNotifChannel = terminal_bell` as backup idle signal
- [ ] Change the dimensions of the terminal(s) and communicate to the terminal itself
- [ ] Configure Claude Code hooks (HTTP endpoint) for idle/permission detection
- [ ] Run Claude Code inside the embedded terminal
- [ ] Implement Quake-style drop-down terminal toggle
- [ ] Implement session JSONL reader for reply capture (extract assistant messages to `./reply/<id>.md`)
- [ ] Show old replies and plans as well. Provide a picker to scroll through to view. (Ctrl-Shift-O)

### TODO.md System
- [x] Integrate `gpui-component` editor widget with `tree-sitter-md` for markdown highlighting
- [x] Detect external file modifications (file watcher) and show visual indicator
- [ ] Automatically reload when the buffer is not dirty, rather than displaying a message
- [ ] Fix focus to center the target in the middle of the screen
- [ ] Word wrapping lines to fix the length of lines
- [ ] Add custom highlight pass for TODO.md constructs (WAIT markers, Message headers, comment annotations)
- [ ] Parse TODO.md format (agents, messages, WAIT markers)
- [ ] Build library for managing TODO.md representation (ropey-backed)
- [ ] Implement comment insertion from other views into the WAIT section
- [ ] Implement "select and send" flow: selection -> new Message heading -> send to terminal -> move WAIT
- [ ] Implement conflict resolution (git-style merge of buffer vs disk)
- [ ] Have a shared place outside of all repositories to have a skill/pattern reference (like the "optimize plan" thing) [Perhaps it shows ~/.claude/jc.md]

### Git Diff View
- [x] Render git diff via `git2` with `tree-sitter` syntax-aware highlighting
- [x] Add word-level inline highlighting via `similar`
- [ ] Fix word-level diff (strange annotations appearing instead of highlight
- [ ] Word wrapping lines to fix the length of lines
- [ ] Implement per-file "mark as reviewed" with collapses
- [ ] Annotate which files have been "marked" in the git diff picker
- [ ] Show one file at a time rather than raw multi-file output
- [ ] Implement region selection and comment keybinding
- [ ] Show git log as well to look at older diffs. Provide a picker to scroll through to view. (Ctrl-Shift-O)

### Code Viewer
- [x] Implement syntax-highlighted file viewer (`tree-sitter` + `tree-sitter-highlight`)
- [x] Implement fuzzy file picker (search repo files)
- [x] Implement symbol picker with hierarchy context (custom `outline.scm` queries)
- [x] Implement "open in external editor" keybinding (Cmd-Shift-E)
- [ ] Fix focus to center the target in the middle of the screen
- [ ] Word wrapping lines to fix the length of lines
- [ ] Implement light editing (basic text modification, not full editor)

### Window & Pane Management
- [x] Fix: Window doesn't get focused on creation
- [x] Add window keybindings (Cmd+W close, Cmd+M minimize, Cmd+Q quit)
- [x] Implement left/right two-pane layout (resizable, with pane focus tracking)
- [x] Implement pane view switching for terminals (Cmd-1 Claude, Cmd-2 General, Cmd-[/] focus)
- [x] Cmd-[/] overrides InputState indent/outdent when editor is focused (bound in Input context)
- [x] Implement view switching for diff, TODO, code views (Cmd-3/4/5)
- [x] Use a monospace font (Menlo) in code editor views (diff, code, TODO)
- [x] Disable line numbers in code editor views
- [ ] Change the size of the split by dragging with a keybinding to make even split
- [ ] Change the font to Lilex (https://lilex.myrt.co/); may require downloading and bundling in a 'data' directory
- [ ] Move default theme file to 'data' directory rather than embedding defaults in source
- [ ] Implement view switching for reply view
- [ ] Implement independent scroll positions per pane
- [ ] Unified theme system tying together UI chrome, terminal palette, and code editor highlighting
- [ ] Add a light theme
- [ ] Auto-switch themes with system dark mode
- [ ] Implement Quake-style bottom terminal overlay
- [ ] Implement multi-window with shared session state

### Navigation & Pickers
- [x] Implement generic fuzzy picker library shared by multiple pickers
- [x] Implement file picker
- [x] Implement symbol picker
- [ ] Improve open_file_picker to use show_picker
- [ ] Track most recently visited files and put them at the top of file picker
- [ ] Track modified files (from git) and mark them in the file picker
- [ ] Implement fuzzy project/task picker with filtering (waiting tasks, same project)
- [ ] Use syntax highlighting inside of the picker appropriate to the original language
- [ ] Implement keybinding system (configurable, emacs-style defaults)

### Notifications & Status
- [x] Implement Claude usage algorithm
- [x] Implement configurable working hours for par calculation
- [ ] Turn usage into a single number/visualization that shows "par"
- [ ] Implement Claude usage dashboard: poll OAuth usage API, display 5h/7d %, par calculation
- [ ] Implement local HTTP server to receive Claude Code hook events (Stop, Notification, PermissionRequest)
- [ ] Implement in-app status bar showing waiting tasks (driven by hook events)
- [ ] Implement macOS desktop notifications via `objc2-user-notifications` (action buttons: "Switch to Task")

### Mobile App
- [ ] Design mobile app protocol (WebSocket messages: status updates, TODO edits, permission requests, commands)
- [ ] Implement TLS server (`axum` + `axum-server` + `rcgen` self-signed certs)
- [ ] Implement QR code pairing flow (`fast_qr`, encode host + port + cert fingerprint)
- [ ] Build mobile app: dashboard view
- [ ] Build mobile app: TODO review and note-taking
- [ ] Build mobile app: Claude permission handling (relay from hooks)
- [ ] Build mobile app: send commands to Claude

### Git Worktrees
- [ ] Implement git worktree creation/deletion via `git2` worktree API

### Polish & Integration
- [ ] Reduce duplication between CodeView and TodoView (consider having TodoView wrap a CodeView)
- [ ] End-to-end test: full workflow from project creation to Claude review cycle
- [ ] Persistent state: survive app restart without losing task state or terminal sessions
- [ ] Performance: handle multiple concurrent terminal sessions smoothly
- [ ] Error handling: graceful recovery from Claude crashes, terminal failures, disk issues
- [ ] App bundling: `.app` bundle with `Info.plist`, ad-hoc code signing for notifications + distribution

### Unsorted
