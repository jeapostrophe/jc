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
- Per-view typed problem enums (`SessionProblem`, `ProjectProblem`) track actionable conditions; see [Problems & Status](#problems--status)
- **Session picker (Cmd-P):** Shows all adopted sessions across all projects. Format: `project / label    (slug) recency`. The `(slug)` is only shown when the label is ambiguous (appears on more than one session). Markers: red problem count for sessions with problems, green `>` for active session, blank otherwise. Problem counts include both session-level and project-level problems.
- **Slug picker (Cmd-Shift-P):** Shows all discovered sessions for the current project plus a "NEW" entry. Format: `project / label    (slug) recency`. Adopted sessions show their TODO label; orphaned sessions show "Attach". Markers: red problem count or green `✓` for adopted sessions, blue `+` for attach, yellow `*` for new. Selecting "NEW" launches a fresh Claude instance and auto-detects the slug.
- Title bar shows `project > session` with `!` dirty marker and problem count when the active session has problems

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

**Current implementation:** Dock bounce via `NSApplication::requestUserAttention` (`objc2-app-kit`, already linked through GPUI). No app bundling or code signing required. Fires on hook events (Claude stop, permission prompt, idle) when the window is not active. Critical events (permission prompts) bounce repeatedly until the user focuses the app.

**Notification banners** (`osascript`, `UNUserNotificationCenter`) require a bundled `.app` with a bundle ID — they silently fail for unbundled binaries. Banners will work once the app is bundled for distribution.

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

## Problems & Status

The app tracks **problems** — actionable conditions that need the user's attention. Problems drive the notification system, status indicators, and navigation.

### Problem Sources

Each view defines its own problem types. Problems are scoped to the level that owns them:

**Session-level problems** (owned by `SessionState`):

| Source | Problem | Trigger | Resolution |
|---|---|---|---|
| Claude terminal | `ClaudeProblem::Stop` | Hook event: Claude finishes | User interacts with the session |
| Claude terminal | `ClaudeProblem::Permission` | Hook event: permission prompt | User interacts with the session |
| Claude terminal | `ClaudeProblem::Idle` | Hook event: idle prompt | User interacts with the session |
| Claude terminal | `ClaudeProblem::ApiError` | Hook event: API error | User interacts with the session |
| General terminal | `TerminalProblem::Bell` | BEL character detected | User focuses the terminal |
| TODO view | `TodoProblem::UnsentWait` | Content exists below `### WAIT` | Content is sent or removed |
| TODO view | `TodoProblem::InvalidSlug` | `## Session` slug has no JSONL | Slug is corrected or JSONL appears |

**Project-level problems** (owned by `ProjectState`):

| Source | Problem | Trigger | Resolution |
|---|---|---|---|
| Diff view | `DiffProblem::UnreviewedFile(PathBuf)` | Dirty working tree files | File marked as reviewed |
| Script | `ScriptProblem { rank, file, line, message }` | `./status.sh` output | Script stops reporting it |

### Problem Type Design

Each view defines its own enum. Aggregation uses wrapper enums at the session and project level:

```rust
// Per-view leaf enums (jc-core/src/problem.rs)
enum ClaudeProblem { Stop, Permission, Idle, ApiError }
enum TerminalProblem { Bell }
enum DiffProblem { UnreviewedFile(PathBuf) }
enum AppTodoProblem { UnsentWait { slug }, InvalidSlug { slug, line } }

// Script problems (scaffolded, not yet wired) use the format: {rank:}?file{:line}? - message
struct ScriptProblem { rank: Option<i8>, file: PathBuf, line: Option<usize>, message: String }

// Session-level wrapper
enum SessionProblem { Claude(ClaudeProblem), Terminal(TerminalProblem), Todo(AppTodoProblem) }

// Project-level wrapper
enum ProjectProblem { Diff(DiffProblem), Script(ScriptProblem) }
```

Note: `AppTodoProblem` is distinct from the parser-level `TodoProblem` in `jc-core/src/todo.rs`. The parser detects raw conditions (invalid slug, unsent wait); `SessionState::refresh_problems()` converts them into `AppTodoProblem` variants filtered to the session's slug.

Each problem type has a `rank()` and `description()` method. Built-in ranks: permission (1) > API error (2) > stop (3) > idle (4) > BEL (5) > unsent wait (6) > invalid slug (7) > unreviewed file (10). Script problems use an explicit optional rank; unranked ones default to 20.

### Refresh Model

Problems are recomputed on a unified refresh cycle rather than managed individually:

- **Push sources** (hooks, BEL): Write into a `pending_events` set on the session. Events persist in this set until their resolution condition is met.
- **Poll sources** (diff, TODO, status.sh): Computed fresh each cycle by querying the relevant view.
- A single `refresh_problems()` method on session/project merges both: it converts pending events into problems, queries poll sources, and replaces the full problem list. It returns whether the problem count changed.
- Refresh runs on a 2-second timer and on demand when the user switches sessions, reviews a diff file, or interacts. The timer only triggers a re-render (`cx.notify()`) when problems actually change.

**Resolution** has two flavors:
- **Implicit**: The condition no longer holds on next poll (diff is clean, WAIT is empty, script stops reporting). These resolve automatically via the refresh-replaces-all model.
- **Acknowledgment**: The user does something that clears pending event flags. `SessionState` stores a `pending_events: HashSet<PendingEvent>` set. Push sources (hooks, BEL) insert events; `session.acknowledge()` clears all events when the user switches to a session. Additionally, switching to the Claude terminal clears the `TerminalBell` event specifically.

### Display

Problems surface in multiple locations:

- **Title bar**: `"! Project > Session N"` — a `!` dirty marker on the left and a problem count on the right when the active session + project has problems. The count is session problems + project problems.
- **Session picker** (Cmd-P): Red problem count replaces the green `>` marker for sessions with problems. Count is session + project combined.
- **Slug picker** (Cmd-Shift-P): Red problem count replaces the green `✓` for adopted sessions with problems.
- **Global indicator** (upper right, left of usage): Count of *other* sessions (not the active one) that have problems.

All marker columns use a fixed-width right-aligned layout (`picker_marker_base()`) so single-character markers and multi-digit counts align cleanly.

**Not yet implemented:**
- Hover tooltips listing individual problems on title bar and global indicator
- Problem navigation (jump to the view/file that can address each problem kind)
- Jump to next problem keybinding (Cmd-;)

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

### Claude Usage Dashboard

Always visible in a corner. Shows:
- **5-hour window usage %** and time remaining
- **Weekly usage %** and time remaining
- **Par indicator:** compares usage % against elapsed working time %. "Under par" means you have budget to spare; "over par" means you're consuming faster than your pace.
- Configurable working hours (e.g., exclude Sunday, reduce Saturday) so par calculations reflect actual work schedule, not raw calendar time.

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
| Claude reply viewer | Parse session JSONL from `~/.claude/projects/`, group into turns, render as Markdown in read-only editor |
| Claude usage dashboard | Poll `GET https://api.anthropic.com/api/oauth/usage` (OAuth token from macOS Keychain) |
| Persistent state | `~/.config/jc/` --- project registry, window layout; session state in TODO.md |
| Desktop notifications | Dock bounce via `objc2-app-kit` (`NSApplication::requestUserAttention`); no bundling required. Banners need `.app` bundle. |

## Keybindings

### Global (Workspace)

| Key | Action | Context |
|---|---|---|
| Cmd-1 | Show Claude terminal | |
| Cmd-2 | Show general terminal | |
| Cmd-3 | Show git diff | |
| Cmd-4 | Show code viewer | |
| Cmd-5 | Show TODO editor | |
| Cmd-6 | Show reply viewer | |
| Cmd-[ | Focus left pane | |
| Cmd-] | Focus right pane | |
| Cmd-\| | Even split panes | |
| Cmd-P | Session picker | |
| Cmd-Shift-P | Slug picker (current project) | |
| Cmd-O | File picker | |
| Cmd-T | Context picker (symbols / headings / modified files) | |
| Cmd-Shift-O | Git log picker | |
| Cmd-F | Search lines | |
| Cmd-K | Open comment panel | |
| Cmd-Shift-K | Snippet picker (`~/.claude/jc.md`) | |
| Cmd-S | Save file | |
| Cmd-Enter | Send to terminal | |
| Cmd-; | Jump to next problem | |
| Cmd-: | Show problem picker | |
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

### Comment Panel

| Key | Action |
|---|---|
| Cmd-Enter | Submit comment |
| Escape / Cmd-W | Dismiss |

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

### Problems & Status
- [ ] [H] Upgrade to `objc2-user-notifications` for action buttons ("Switch to Session") and notification grouping (requires app bundling)

### Remote Workflow (CLI & Hooks)
- [ ] [H] `jc status` subcommand: JSON output of projects, sessions, problems, usage
- [ ] [H] `jc problems` subcommand: JSON list of all problems with ranks
- [ ] [E] `jc note` subcommand: append text below WAIT marker
- [ ] [E] External notification hook (push problem events to ntfy/Pushover)
- [x] [D] Draft feature request: Claude Code user-side sideband display (output user sees but Claude does not)

### Git Worktrees
- [ ] [H] Implement git worktree creation/deletion via `git2` worktree API

### Polish & Integration
- [ ] [H] End-to-end test: full workflow from project creation to Claude review cycle
- [ ] [H] Persistent state: survive app restart without losing session state or terminal sessions [perhaps use 'tmux' behind the scenes]
- [ ] [H] Performance: handle multiple concurrent terminal sessions smoothly
- [ ] [H] Error handling: graceful recovery from Claude crashes, terminal failures, disk issues
- [ ] [H] App bundling: `.app` bundle with `Info.plist`, ad-hoc code signing via `scripts/bundle.sh` (prerequisite for `objc2-user-notifications` upgrade)
- [ ] [H] Single-instance guard: `NSRunningApplication` check on startup to prevent duplicate `.app` launches (activate existing window and exit)
- [ ] [H] CLI-to-GUI IPC: Unix domain socket (`~/.config/jc/jc.sock`) so `jc .` sends project path to running instance; startup binds socket or connects to existing one

### Code Quality

### Automation
- [ ] [D] Manage automations; i.e. creating sessions and running them automatically
