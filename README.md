# jc

A native macOS Rust application for orchestrating multiple Claude Code sessions across projects. It provides a keyboard-driven workflow for managing tasks, reviewing diffs, annotating code, and sending instructions to Claude --- all from a single app.

## Design Principles

- **macOS only.** No cross-platform concerns.
- **Rust.** Follow Zed's GUI practices (GPUI) where possible.
- **Keyboard-first.** Single key, Emacs-style bindings with modal ideas. Not a full vim emulator, just efficient keyboard-driven navigation.
- **Claude Code directly.** Run the real Claude Code CLI, not the Agent SDK, so we get upstream improvements for free.
- **Minimal but functional.** Not a full IDE. Opens files in Zed (or another editor) for serious editing.

## Core Concepts

### Projects

A project corresponds to a code repository. The app tracks active projects in `~/.config/jc/`. Projects are added via an in-app command or from the command line with `jc .`.

### Tasks

Each project has one or more tasks. A task represents a unit of work being done, possibly with its own git worktree (managed by the app) or sharing the project's main tree. Each task has:
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
- Fuzzy file picker (searches repository files)
- Symbol picker (tree-sitter or LSP powered, shows hierarchy context --- e.g., which `impl` block a function belongs to)
- Comment keybinding works here too
- Keybinding to open the file in an external editor (Zed)

### Claude Reply Viewer

When Claude produces a long response, a keybinding sends `/copy` to Claude and writes the Markdown result to a temporary file (`./reply/<id>.md`). The user scrolls through it and can annotate inline. Modifications are tracked and written as comments into TODO.md.

## Status & Usage Tracking

### Task Status Indicators

A persistent indicator shows which tasks have Claude responses waiting for review. The app detects "waiting" by observing that the Claude terminal has stopped producing output.

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
| GUI framework | GPUI (from Zed) |
| Terminal emulator | Zed's terminal crate (alacritty_terminal or similar) --- needs enough fidelity for Claude Code's TUI |
| Markdown editor | Custom source editor with light rendering |
| Syntax highlighting | tree-sitter |
| Symbol navigation | tree-sitter (with optional LSP for richer info) |
| Git diff | libgit2 via `git2` crate, custom rendering |
| Git worktrees | Managed by the app via `git2` or CLI |
| Persistent state | `~/.config/jc/` --- project list, task state, window layout |
| Mobile server | Built-in TLS server, QR-code pairing |
| Mobile app | Separate project (Swift/native iOS likely) |
| Desktop notifications | macOS native APIs |

## Workflow Walkthrough

### Reviewing Claude's Output

1. Claude finishes working. A desktop notification fires and the in-app indicator lights up.
2. Press a keybinding to switch the left pane to Claude's terminal.
3. Press a keybinding to switch to the diff view. Scroll through changes.
4. Highlight a region (with mouse or keybindings) in the diff, press the comment key, type a note. It appears in TODO.md under `### WAIT`.
5. Mark reviewed files as done (they collapse).
6. Switch to the general terminal to run tests or inspect behavior. Highlight output, press comment key.
7. If Claude's textual response was long, press a key to capture it via `/copy` into a reply file. Scroll and annotate.

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

## Task Checklist

### Research
- [ ] Evaluate GPUI: can we use it outside of Zed? What's the extraction story?
- [ ] Evaluate Zed's terminal emulator crate: can it be used standalone? Does it handle Claude Code's output correctly?
- [ ] Survey tree-sitter Rust bindings and available grammars
- [ ] Determine the best approach for the Markdown editor (custom on GPUI vs adapting an existing component)
- [ ] Research `git2` crate capabilities for diff display and worktree management
- [ ] Investigate macOS notification APIs from Rust (e.g., `mac-notification-sys` or direct objc calls)
- [ ] Determine how to detect Claude Code "idle" state (terminal output silence heuristic, PTY monitoring)
- [ ] Research how Claude Code `/copy` works and how to programmatically invoke it. [Could just be literally sending `/copy\n` using the terminal and then `pbpaste` to get the content.]
- [ ] Evaluate TLS server crates for the mobile connection (rustls, native-tls)
- [ ] Research QR code generation crates
- [ ] Investigate Claude usage API or scraping approach for the usage dashboard [Could just be having another hidden `claude` session where you send `/usage` periodically]

### Core Infrastructure
- [ ] Set up Rust project structure (workspace with crates)
- [ ] Integrate GPUI and get a basic window rendering
- [ ] Implement `~/.config/jc/` config and state persistence
- [ ] Implement project and task data model
- [ ] Implement `jc` CLI for adding projects from the command line
- [ ] Implement git worktree creation/deletion for tasks

### Terminal Emulator
- [ ] Integrate terminal emulator library
- [ ] Run Claude Code inside the embedded terminal
- [ ] Run general-purpose shell in a second terminal per task
- [ ] Detect terminal output idle (for "Claude is waiting" notifications)
- [ ] Implement `/copy` capture: send command to Claude, intercept response, write to reply file
- [ ] Implement Quake-style drop-down terminal toggle

### TODO.md System
- [ ] Implement Markdown source editor with light rendering (bold, headings, highlights)
- [ ] Build library for managing TODO.md representation
- [ ] Parse TODO.md format (agents, messages, WAIT markers)
- [ ] Implement "select and send" flow: selection -> new Message heading -> send to terminal -> move WAIT
- [ ] Implement comment insertion from other views into the WAIT section
- [ ] Detect external file modifications and show visual indicator
- [ ] Implement conflict resolution (git-style merge of buffer vs disk)

### Git Diff View
- [ ] Render git diff with syntax-aware highlighting
- [ ] Implement region selection and comment keybinding
- [ ] Implement per-file "mark as reviewed" with collapse
- [ ] Support scrolling through multi-file diffs

### Code Viewer
- [ ] Implement syntax-highlighted file viewer (tree-sitter)
- [ ] Implement fuzzy file picker (search repo files)
- [ ] Implement symbol picker with hierarchy context
- [ ] Implement light editing (basic text modification, not full editor)
- [ ] Implement "open in external editor" keybinding

### Window & Pane Management
- [ ] Implement left/right two-pane layout
- [ ] Implement view switching per pane (Claude, terminal, diff, TODO, code, reply)
- [ ] Implement independent scroll positions per pane
- [ ] Implement multi-window with shared session state
- [ ] Implement Quake-style bottom terminal overlay

### Navigation & Pickers
- [ ] Implement generic fuzzy picker library shared by multiple pickers
- [ ] Implement fuzzy project/task picker with filtering (waiting tasks, same project)
- [ ] Implement file picker
- [ ] Implement symbol picker
- [ ] Implement keybinding system (configurable, emacs-style defaults)

### Notifications & Status
- [ ] Implement in-app status bar showing waiting tasks
- [ ] Implement macOS desktop notifications on Claude idle
- [ ] Implement Claude usage dashboard (5-hour window %, weekly %, par calculation)
- [ ] Implement configurable working hours for par calculation

### Mobile App
- [ ] Design mobile app protocol (what data is sent, what commands are available)
- [ ] Implement TLS server on desktop
- [ ] Implement QR code pairing flow
- [ ] Build mobile app: dashboard view
- [ ] Build mobile app: TODO review and note-taking
- [ ] Build mobile app: Claude permission handling
- [ ] Build mobile app: send commands to Claude

### Polish & Integration
- [ ] End-to-end test: full workflow from project creation to Claude review cycle
- [ ] Persistent state: survive app restart without losing task state or terminal sessions
- [ ] Performance: handle multiple concurrent terminal sessions smoothly
- [ ] Error handling: graceful recovery from Claude crashes, terminal failures, disk issues
