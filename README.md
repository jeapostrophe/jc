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

### App Icon

`icon.png` is the app icon (1024x1024). Use it for the `.app` bundle's `AppIcon.icns`.

### Project Structure

```
Cargo.toml                          # workspace root
data/
  dark_theme.toml                   # unified dark theme: terminal palette, UI chrome, syntax (Tomorrow Night)
  light_theme.toml                  # unified light theme: terminal palette, UI chrome, syntax (Tomorrow)
  fonts/                            # bundled Lilex font (Regular, Bold, Italic, BoldItalic)
jc-core/                            # data model + config persistence
  src/lib.rs, config.rs, model.rs, problem.rs, theme.rs
jc-terminal/                        # embedded terminal emulator
  src/lib.rs, colors.rs, input.rs, terminal.rs, pty.rs, render.rs, view.rs
  examples/terminal_window.rs
jc-app/                             # binary: CLI + GPUI app
  src/main.rs, app.rs, outline.rs, language.rs, views/{workspace,pane,picker,project_state,session_state,diff_view,code_view,todo_view}.rs
  src/outline_queries/{rust,markdown,python,go,javascript,typescript}.scm
  examples/basic_window.rs
```

## Design Principles

- **macOS only.** No cross-platform concerns.
- **Rust.** Follow Zed's GUI practices (GPUI) where possible.
- **Keyboard-first.** Single key, Emacs-style bindings with modal ideas. Not a full vim emulator, just efficient keyboard-driven navigation.
- **Claude Code directly.** Run the real Claude Code CLI in an embedded terminal so we get upstream improvements for free. Use Claude Code's hooks system (HTTP hooks) for structured status events alongside the raw terminal. Reply capture uses Claude's built-in `/copy` command with clipboard polling.
- **Minimal but functional.** Not a full IDE. Opens files in Zed (or another editor) for serious editing.

## Core Concepts

### Projects

A project corresponds to a code repository. The app tracks active projects in `~/.config/jc/`. Projects are added via an in-app command or from the command line with `jc .`.

### Sessions

Each project has one or more sessions. A session represents an ongoing Claude Code conversation, identified by a **UUID** --- the session ID assigned by Claude Code. Sessions are defined in the project's TODO.md file via headings with a UUID metadata line:

```markdown
## Refactor auth module
> uuid=abc123-def456-...
```

Each session has:
- A Claude Code terminal, resumed via `--resume <uuid>`
- A general-purpose terminal
- Its own message history and notes within TODO.md

**State model:** `state.toml` holds only a project registry (list of project paths). All session state is derived from TODO.md files. The session picker reads session headings from each project's TODO.md.

**UUID assignment:** When a new session is created, it starts without a UUID. The UUID is assigned automatically when Claude Code's first hook event arrives (which reports the session ID). The hook system updates both the in-memory session state and the TODO.md file.

**Session lifecycle:**
- **Project init:** When a project is first added to jc, the app reads TODO.md for existing session headings with UUIDs. Each is resumed via `claude --resume <uuid>`. If no sessions exist, a plain `claude` instance is launched.
- **New session:** From the session picker (Cmd-P), the user can create a new session. The app launches a fresh Claude Code instance; the UUID is auto-detected from the first hook event.
- **Disable session:** From the session picker (Cmd-P), press Cmd-Shift-Backspace to toggle the `[D]` (disabled/dormant) prefix on a session. Disabled sessions are not auto-attached on startup but remain in the picker's adopt list (shown as "(disabled)"). If adopted, they detach and return to the disabled list. To permanently delete a session, manually change `[D]` to `[DELETED]` in the TODO.md heading.
- **`/clear` handling:** When the user runs `/clear` in a Claude terminal, the hook system detects the session clear event and automatically updates the session's UUID to the new session ID.

### Session Architecture

The app uses an `App -> Projects -> Sessions` hierarchy. Each `ProjectState` owns a TODO file, diff view, code view, and a `HashMap<SessionId, SessionState>` keyed by numeric ID. Each `SessionState` owns a Claude terminal (resumed via `--resume <uuid>`), a general terminal, and an optional UUID. The workspace has an active project with an active session; the active session drives which terminals are shown in the panes. Switching sessions swaps the pane contents without disconnecting terminals. Sessions can be disabled at runtime via the session picker (Cmd-Shift-Backspace), which toggles the `[D]` prefix on the TODO heading. Disabled sessions are skipped during auto-attach but remain visible in the adopt list. To permanently delete, manually change `[D]` to `[DELETED]` in TODO.md.

Key design points:
- Separate terminal instances per session (switching sessions does not disconnect terminals)
- Session state derived from TODO.md headings with `> uuid=...` metadata, not persisted separately
- Per-view typed problem enums (`SessionProblem`, `ProjectProblem`) track actionable conditions; see [Problems & Status](#problems--status)
- **Session picker (Cmd-P):** Shows all sessions across all projects. Format: `project / label`. Markers: red problem count for sessions with problems, green `>` for active session, blue `+` for empty projects. Problem counts include both session-level and project-level problems.
- Title bar shows `project > session` with `!` dirty marker and problem count when the active session has problems

### TODO.md

Each project has a single TODO.md file. The app is the sole writer; if the file changes on disk, the app detects it and shows a visual indicator (with optional git-style merge).

The file serves dual purposes: freeform project notes and session state. Sessions are defined by `##` headings with a `> uuid=...` metadata line under `# Claude`:

```markdown
# TODO
(freeform notes, project-level planning)

# Claude
## Refactor auth module
> uuid=abc123-def456-...
### Message 0
first instruction sent to claude
### Message 1
second instruction
### WAIT
Future notes go here while Claude is working.
These become the basis for the next message.
```

The `### WAIT` marker separates what has been sent from what is being drafted. When the user selects text above `### WAIT` and presses the send key, that text is wrapped in a new `### Message N` heading and sent to the Claude terminal. The `### WAIT` marker moves below it. Unselected text remains as future notes.

The `## Label` heading followed by `> uuid=...` is parsed by the app. The label is a freeform description. The UUID links the session to a Claude Code session for `--resume`.

Session headings support two special prefixes:
- `## [D] Label` — **disabled/dormant**. The session is parsed and appears in the adopt list but is not auto-attached on startup. Toggle via Cmd-Shift-Backspace in the session picker.
- `## [DELETED] Label` — **deleted**. The session is completely skipped by the parser and does not appear anywhere. This is a manual edit; the app never writes this prefix directly.

### Comment Format

From any view (diff, terminal, code), the user can press a comment keybinding to annotate a region. Comments are appended to the current session's future notes (below `### WAIT`) in these formats:

- **From diff or code view:** `* <file>:<start_line>-<end_line> --- Comment text`
- **From terminal:** `* TERMINAL\n\`\`\`\n[selected content]\n\`\`\`\nComment text`

## Research

### macOS Notifications

**Current implementation:** Dock bounce via `NSApplication::requestUserAttention` (`objc2-app-kit`, already linked through GPUI). No app bundling or code signing required. Fires on hook events (Claude stop, permission prompt, idle) when the window is not active. Critical events (permission prompts) bounce repeatedly until the user focuses the app.

**Notification banners** (`osascript`, `UNUserNotificationCenter`) require a bundled `.app` with a bundle ID — they silently fail for unbundled binaries. Banners will work once the app is bundled for distribution.

## Views & Panels

The window supports **1, 2, or 3 panes** (Cmd-1/2/3). Any view can appear in any pane via the view picker (Cmd-.). Cmd-[/] cycle focus between visible panes. When reducing pane count, the focused pane swaps into a visible position so you never lose your place. A **Quake-style terminal** can be toggled at the bottom of the window via a keybinding for quick commands.

Multiple windows can be open simultaneously. Windows share state: if two windows show the same session's Claude terminal, scrolling or changes in one are reflected in the other.

### Claude Terminal

The primary view. Shows the Claude Code CLI running in a terminal emulator. The app detects when Claude stops producing output and shows a notification (desktop notification + in-app visual indicator).

### General Terminal

A separate terminal per session for running arbitrary commands. Tied to the session's working directory (worktree or project root).

### Terminal Architecture

The terminal emulator (`jc-terminal/`) uses `alacritty_terminal` as a crate for VT parsing and grid state management only — not its GPU renderer. Rendering is handled by a custom gpui bridge in `render.rs`.

**Data flow:**
1. **PTY reader thread** reads raw bytes in 4KB chunks, sends via flume channel
2. **Async coalescing task** batches received bytes (up to 64KB cap) and calls `processor.advance()` on the alacritty `Term` grid, then `cx.notify()` to trigger a repaint
3. **Canvas paint closure** locks the `Term`, reads the grid, and calls `paint_terminal()`

**Render pipeline** (`paint_terminal` in `render.rs`):
- Pass 1: Cell backgrounds — `paint_quad()` per cell with non-default bg color
- Pass 1.5: Selection highlight — `paint_quad()` per selected cell
- Pass 2: Text — one `shape_line()` call per row (batches all characters + style runs), painted at row position
- Pass 3: Cursor — a few `paint_quad()` calls for the cursor shape

**Performance optimizations:**
- **Dirty tracking**: A `content_generation` counter (incremented after each `advance()`) is compared against `last_painted_generation`. When content hasn't changed (cursor blink, mouse events, focus), Passes 1 and 2 are skipped entirely — only cursor and selection are repainted.
- **Row-based shaping**: Pass 2 accumulates each row into a single `String` with `Vec<TextRun>` entries split at style boundaries (color/weight/style changes). This produces ~25 `shape_line()` calls per frame instead of ~2000. gpui's `LineLayoutCache` caches shaped lines across frames, so unchanged rows are free on subsequent paints.
- **Buffer coalescing cap**: The async task caps coalesced PTY data at 64KB to prevent frame stalls from large output bursts.

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

### Claude Reply Capture

Replies are captured via Claude Code's built-in `/copy` command. Pressing **Cmd-Shift-C** sends `/copy` to the active Claude terminal, polls the clipboard for a change (via the `arboard` crate for native clipboard access), and writes the result to `.jc/replies/<uuid>.md` (or `.jc/replies/<label>.md` if no UUID is assigned yet). The file is then opened in the code viewer pane for review and annotation.

This approach avoids parsing JSONL files entirely --- Claude Code handles the extraction, and the app just captures the clipboard result. The saved reply files can be referenced by Claude in future instructions (via file path comments).

## Problems & Status

The app tracks **problems** — actionable conditions that need the user's attention. Problems drive the notification system, status indicators, and navigation.

### Problem Sources

Each view defines its own problem types. Problems are scoped to the level that owns them:

**Session-level problems** (owned by `SessionState`):

| Source | Problem | Layer | Trigger | Resolution |
|---|---|---|---|---|
| Claude terminal | `ClaudeProblem::Permission` | L0 | Hook event: permission prompt | User interacts with the session |
| Claude terminal | `ClaudeProblem::StopFailure` | L0 | Hook event: API error (StopFailure) | User interacts with the session |
| General terminal | `TerminalProblem::Bell` | L1 | BEL character detected | User focuses the terminal |
| TODO view | `AppTodoProblem::UnsentWait` | L2 | Content exists below `### WAIT` | Content is sent or removed |
| Session state | *(synthetic L3)* | L3 | `!busy && has_ever_been_busy` | User starts new work |

**Project-level problems** (owned by `ProjectState`):

| Source | Problem | Layer | Trigger | Resolution |
|---|---|---|---|---|
| Diff view | `DiffProblem::UnreviewedFile(PathBuf)` | L1 | Dirty working tree files | File marked as reviewed |
| Script | `ScriptProblem { rank, file, line, message }` | L1 | `./status.sh` output | Script stops reporting it |

### Problem Type Design

Each view defines its own enum. Aggregation uses wrapper enums at the session and project level:

```rust
// Per-view leaf enums (jc-core/src/problem.rs)
enum ClaudeProblem { Permission, StopFailure }
enum TerminalProblem { Bell }
enum DiffProblem { UnreviewedFile(PathBuf) }
enum AppTodoProblem { UnsentWait { label } }

// Script problems use the format: {rank:}?file{:line}? - message
struct ScriptProblem { rank: Option<i8>, file: PathBuf, line: Option<usize>, message: String }

// Session-level wrapper
enum SessionProblem { Claude(ClaudeProblem), Terminal(TerminalProblem), Todo(AppTodoProblem) }

// Project-level wrapper
enum ProjectProblem { Diff(DiffProblem), Script(ScriptProblem) }

// Problem layers (lower = higher priority)
enum ProblemLayer { L0, L1, L2, L3 }
```

Note: `AppTodoProblem` is distinct from the parser-level `TodoProblem` in `jc-core/src/todo.rs`. The parser detects raw conditions (unsent wait); `SessionState::refresh_problems()` converts them into `AppTodoProblem` variants filtered to the session's label.

### Problem Layers

Problems are organized into 4 priority layers. Cmd-; (next problem) processes them in layer order:

| Layer | Problems | Meaning | Scope |
|---|---|---|---|
| L0 | Permission, StopFailure | Claude blocked/failed | Cross-session — always handled first |
| L1 | Terminal Bell, Unreviewed Diffs, Script | Review work | Current session/project |
| L2 | UnsentWait (suppressed if busy or L1 exists) | Send new work | Current session |
| L3 | Idle + has_ever_been_busy (synthetic) | Start new work | Current session |

**Cmd-; behavior:**
1. If any L0 problems exist anywhere, jump to them (cross-session, project-index order). Stores "home session" on first L0 jump.
2. When all L0 cleared, return to home session and cycle L1/L2/L3.
3. Within a layer, cycle through individual problems; when a layer is exhausted, advance to the next.
4. L2 is suppressed when Claude is busy or L1 problems exist (review before sending).

**Corner indicator:** Shows per-layer counts (e.g., `1 / 3 / 0 / 2`) with layer-specific colors: L0=red, L1=yellow, L2=blue, L3=muted. Zero-count layers are omitted. Future work: clickable segments.

Each problem type has a `rank()`, `layer()`, and `description()` method. Ranks are used for intra-layer ordering: permission (1) > StopFailure (2) > BEL (5) > unsent wait (6) > unreviewed file (10). Script problems use an explicit optional rank; unranked ones default to 20.

### Refresh Model

Problems are recomputed on a unified refresh cycle rather than managed individually:

- **Push sources** (hooks, BEL): Write into a `pending_events` set on the session. Events persist in this set until their resolution condition is met.
- **Poll sources** (diff, TODO, status.sh): Computed fresh each cycle by querying the relevant view. Diff generation runs on a background thread (via `std::thread::spawn`) to avoid blocking the UI; `refresh_data()` kicks off a job and picks up results on the next poll cycle.
- A single `refresh_problems()` method on session/project merges both: it converts pending events into problems, queries poll sources, and replaces the full problem list. It returns whether the problem count changed.
- Refresh runs on a 2-second timer and on demand when the user switches sessions, reviews a diff file, or interacts. The timer only triggers a re-render (`cx.notify()`) when problems actually change.

**Resolution** has two flavors:
- **Implicit**: The condition no longer holds on next poll (diff is clean, WAIT is empty, script stops reporting). These resolve automatically via the refresh-replaces-all model.
- **Acknowledgment**: The user does something that clears pending event flags. `SessionState` stores a `pending_events: HashSet<PendingEvent>` set. Push sources (hooks, BEL) insert events; `session.acknowledge()` clears all events when the user switches to a session. Additionally, switching to the Claude terminal clears the `TerminalBell` event specifically.

### Display

Problems surface in multiple locations:

- **Title bar**: `"! Project > Session N"` — a `!` dirty marker on the left and a problem count on the right when the active session + project has problems. The count is session problems + project problems.
- **Session picker** (Cmd-P): Red problem count replaces the green `>` marker for sessions with problems. Count is session + project combined.
- **Global indicator** (upper right, left of usage): Count of *other* sessions (not the active one) that have problems.

All marker columns use a fixed-width right-aligned layout (`picker_marker_base()`) so single-character markers and multi-digit counts align cleanly.

**Not yet implemented:**
- Clickable corner indicator segments (jump directly to a specific layer)

### Script Problems (`status.sh`)

Projects can optionally include a `./status.sh` script. The app runs it periodically and parses stdout lines in the format:

```
file:line - message
file - message
3:file:line - message
```

Where:
- `file` is required (relative to project root)
- `line` is optional (for jump-to-source)
- The leading number before the first `:file` is an optional rank (lower = more important)
- Everything after ` - ` is the message

The script runs with the project root as cwd. Non-zero exit is not an error — it just means no problems. The app ignores stderr.

## Remote Workflow

Rather than building a custom mobile app, jc relies on Claude Code Remote Control for mobile access. The question is how deeply jc should integrate with Claude Code's extension points (hooks, skills, bang commands) to expose its workflow remotely.

### Why Not a Custom Mobile App

Claude Code Remote Control provides the mobile transport layer --- a polished, first-party mobile client that Anthropic will keep improving. Building a custom iOS app + TLS WebSocket server + QR pairing protocol is a large maintenance surface for one developer. Remote Control handles notifications, terminal access, and permission approvals out of the box.

### Why Not an Editor Plugin

It's tempting to decompose jc into a Zed or nvim plugin + tmux + scripts. Editors already provide editing, diffs, syntax highlighting, and terminals. But jc's value is the *opinionated workflow orchestration* --- the thing that makes managing 5 concurrent Claude sessions tractable. Problem-driven navigation that crosses terminal/diff/code/TODO boundaries, the session picker model (Cmd-P across all projects with problem badges), and terminal-as-first-class-view are hard to replicate in an editor that thinks in terms of files and buffers, not Claude conversations. You'd end up rebuilding half of jc inside the editor.

### The Skills & Bang Command Problem

Claude Code offers two extension mechanisms for user-invoked commands:

1. **Skills** (`/skill-name`) — Claude executes a prompt that can include shell commands. The problem: skills cause Claude to *think*. For deterministic operations ("show me the problem list"), thinking is pure waste --- tokens spent interpreting intent and generating prose around data you just want printed.

2. **Bang commands** (`!command`) — Run shell commands directly. Closer to what we want, but: (a) namespace collision (`!status` is valuable `$PATH` space), so you'd need `!jc-status` which gets tiresome across 7+ commands; (b) all output enters Claude's context window, consuming tokens. Checking status 10 times in a session fills context with repetitive tabular data Claude doesn't need to see.

The fundamental gap: **there is no Claude Code mechanism for "show the user something without putting it in context."** jc's desktop app solves this by being a separate viewport --- you see problems, diffs, usage, and session state without Claude ever knowing you looked. Any pure-Claude-Code solution loses this property.

### What's Worth Implementing Anyway

Despite the limitations, a small subset of CLI subcommands are useful for scripting, interop, and the occasional Remote Control check-in:

```
jc status              # JSON: projects, sessions, problems, usage
jc problems            # JSON: all problems with ranks
jc note <text>         # Append text below WAIT marker
```

These three cover the most common remote needs: "what's happening?", "what needs attention?", and "add a quick note." They're useful from any terminal --- Remote Control, SSH, scripts, cron jobs --- without needing skills or bang commands as wrappers.

The remaining operations (`wait`, `send`, `turns`, `diff`) are better performed in the desktop app where you have the full viewport. Wrapping them as skills would burn tokens for a worse experience.

### The Missing Primitive

The right solution is a Claude Code feature: **user-side commands that produce output the user sees but Claude does not.** A sideband display channel. This would let tools like jc expose rich status dashboards, problem lists, and session state inside the Claude Code experience without polluting context.

This is worth a feature request to Anthropic. If Claude Code is meant to be the primary developer environment, users need a way to see ambient information (build status, test results, project dashboards) without paying for it in tokens. The analogy is an IDE's status bar or panel --- always visible, never in the conversation.

### Hooks

Hooks are the one extension point that works well today. Claude Code fires events on stop, permission prompt, idle, and API error. jc's hook server already receives these and updates problem state. The same hooks can trigger external notification services (e.g., ntfy, Pushover) for phone alerts when away from the desktop. No skills or context pollution required --- hooks are push-only and invisible to the conversation.

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
| Problem tracking | Per-view typed enums (`ClaudeProblem`, `TerminalProblem`, `DiffProblem`, `AppTodoProblem`) + wrapper enums (`SessionProblem`, `ProjectProblem`); push via hooks + BEL into `pending_events`; poll via diff/TODO every 2s; `refresh_problems()` merges both and skips re-render when unchanged |
| Claude reply capture | `/copy` command + clipboard polling, writes to `.jc/replies/<uuid>.md` |
| Claude usage dashboard | Poll `GET https://api.anthropic.com/api/oauth/usage` (OAuth token from macOS Keychain) |
| Persistent state | `~/.config/jc/` --- project registry, window layout; session state in TODO.md |
| Desktop notifications | Dock bounce via `objc2-app-kit` (`NSApplication::requestUserAttention`); no bundling required. Banners need `.app` bundle. |

## Keybindings

### Global (Workspace)

| Key | Action | Context |
|---|---|---|
| Cmd-1 | 1-pane layout (focused pane goes full-screen) | |
| Cmd-2 | 2-pane layout (equal widths) | |
| Cmd-3 | 3-pane layout (equal widths) | |
| Cmd-. | View picker (place view in focused pane) | |
| Cmd-[ | Focus previous pane | |
| Cmd-] | Focus next pane | |
| Cmd-P | Session picker | |
| Cmd-O | File picker | |
| Cmd-T | Context picker (symbols / headings / modified files) | |
| Cmd-Shift-O | Git log picker | |
| Cmd-F | Search lines | |
| Cmd-K | Open comment panel | |
| Cmd-Shift-K | Snippet picker (`~/.claude/jc.md`) | |
| Cmd-S | Save file | |
| Cmd-Enter | Send to terminal | |
| Cmd-Shift-C | Copy reply (/copy → clipboard → .jc/replies/) | |
| Cmd-; | Next problem (current project) / jump to WAIT if none | |
| Cmd-. | Jump to WAIT section of active session | |
| Cmd-: | Urgency-sorted session picker | |
| Cmd-? | Keybinding help overlay | |
| Cmd-Shift-E | Open in external editor | |
| Cmd-W | Close window | |
| Cmd-M | Minimize window | |
| Cmd-Q | Quit | |

### View-Specific

| Key | Action | View |
|---|---|---|
| Cmd-R | Reload from disk | Code viewer |
| Cmd-R | Mark file reviewed | Diff view |
| Cmd-C | Copy selection | Terminal |
| Cmd-V | Paste | Terminal |
| Cmd-= / Cmd-+ | Increase font size | Terminal |
| Cmd-- | Decrease font size | Terminal |
| Cmd-0 | Reset font size | Terminal |

### Picker

| Key | Action |
|---|---|
| Enter | Confirm |
| Escape / Ctrl-C | Cancel |
| Down / Ctrl-N | Next item |
| Up / Ctrl-P | Previous item |
| Page Down / Page Up | Page navigation |
| Cmd-Shift-Backspace | Remove session (session/problem picker only) |

### Comment Panel

| Key | Action |
|---|---|
| Cmd-Enter | Submit comment |
| Escape / Cmd-W | Dismiss |

## Workflow Walkthrough

### Reviewing Claude's Output

1. Claude finishes working. A desktop notification fires and the in-app indicator lights up.
2. Use the view picker (Cmd-.) to show Claude's terminal in a pane.
3. Press a keybinding to switch to the diff view. Scroll through changes.
4. Highlight a region (with mouse or keybindings) in the diff, press the comment key, type a note. It appears in TODO.md under `### WAIT`.
5. Mark reviewed files as done (they collapse).
6. Switch to the general terminal to run tests or inspect behavior. Highlight output, press comment key.
7. Press Cmd-Shift-C to capture Claude's reply via `/copy`. The reply is saved to `.jc/replies/` and opened in the code viewer for annotation.

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

1. Add a project: run `jc .` from a repo, or use an in-app command. The app reads TODO.md for existing sessions or launches a fresh Claude instance.
2. Use the session picker (Cmd-P) to switch between sessions across all projects. Sessions with problems are highlighted with a red count.
3. Create new sessions from the session picker. UUIDs are assigned automatically via hooks.

## Task Checklist

> **Difficulty labels** — applied to each unchecked task:
> - **[T]** Trivial — All trivial tasks can be done together in one Claude invocation
> - **[E]** Easy — All easy tasks in the same sub-list can be done together in one Claude invocation
> - **[H]** Hard — Each hard task needs its own Claude invocation but requires no human design input
> - **[D]** Design — Subtle design issues that need to be resolved with a human first
> - **[?]** Unclassified — Needs triage. When you encounter a `[?]` task, read the task description, examine the relevant code, and replace `[?]` with the correct label (`[T]`/`[E]`/`[H]`/`[D]`). Do this before starting any other work.
>
> *When adding new checklist items, always include a `[T]`/`[E]`/`[H]`/`[D]`/`[?]` label after the checkbox. If the item doesn't fit under an existing section, create a new `###` section for it.*

### Git Diff View
- [ ] [H] Add word-level inline highlighting via `similar` with background highlights NOT diagnostics

### Window & Pane Management
- [ ] [H] Implement multi-window with shared session state

### Remote Workflow (CLI & Hooks)
- [ ] [H] `jc status` subcommand: JSON output of projects, sessions, problems, usage
- [ ] [H] `jc problems` subcommand: JSON list of all problems with ranks
- [ ] [E] `jc note` subcommand: append text below WAIT marker
- [ ] [E] External notification hook (push problem events to ntfy/Pushover)

### Git Worktrees
- [ ] [H] Implement git worktree creation/deletion via `git2` worktree API

### Polish & Integration
- [ ] [H] End-to-end test: full workflow from project creation to Claude review cycle
- [ ] [H] Error handling: graceful recovery from Claude crashes, terminal failures, disk issues

### Automation
- [ ] [D] Manage automations; i.e. creating sessions and running them automatically

---

## Claude Code Hook Opportunities

Currently used hooks: `UserPromptSubmit`, `Stop`, `Notification` (idle/permission), `PermissionRequest`, `SessionStart`, `SessionEnd`.

Hooks worth exploring:

- **`PreToolUse`** — Show real-time tool activity in the session status (e.g., "Reading src/main.rs", "Running tests"). Could also enforce project-specific tool policies.
- **`PostToolUse`** — Auto-refresh the code view when Claude writes/edits a file in the current project. Auto-refresh diff view after git operations.
- **`SubagentStart`/`SubagentStop`** — Track concurrent subagent work. Show a count of active subagents in the session status bar. Could detect when a subagent modifies files in the project.
- **`PostCompact`** — Display a notification or marker when context was compacted, so the user knows Claude's memory was trimmed. Could log the compact summary to the TODO.
- **`TaskCompleted`** — Surface completed tasks in the TODO view or as notifications. Could auto-check items in PLAN.md.
- **`PreCompact`** — Inject custom instructions before compaction to preserve project-specific context.
- **`InstructionsLoaded`** — Track which CLAUDE.md files are active, useful for debugging instruction precedence.
