# Architecture

## Project Structure

```
Cargo.toml                          # workspace root
data/
  dark_theme.toml                   # unified dark theme (Tomorrow Night)
  light_theme.toml                  # unified light theme (Tomorrow)
  fonts/                            # bundled Lilex font (Regular, Bold, Italic, BoldItalic)
scripts/
  bundle.sh                         # release build + macOS .app bundle + icon + codesign
  update-outline-queries.sh         # fetch outline.scm files from Zed repo
  update-gpui-component.sh          # re-vendor gpui-component from cargo cache + apply patches
jc-core/                            # data model + config persistence
  src/lib.rs, config.rs, model.rs, problem.rs, theme.rs, todo.rs,
      hooks.rs, hooks_settings.rs, snippets.rs, status_script.rs
jc-terminal/                        # embedded terminal emulator
  src/lib.rs, colors.rs, input.rs, terminal.rs, pty.rs, render.rs, view.rs
  examples/terminal_window.rs
jc-app/                             # binary: CLI + GPUI app
  src/main.rs, app.rs, outline.rs, language.rs, ipc.rs, file_watcher.rs, notify.rs
  src/views/
    workspace/{mod,pickers,problems,render}.rs
    pane.rs, picker.rs, project_state.rs, session_state.rs
    diff_view.rs, code_view.rs, todo_view.rs
    comment_panel.rs, close_confirm.rs, keybinding_help.rs
  src/outline_queries/{rust,markdown,python,go,javascript,typescript}.scm
  examples/basic_window.rs
vendor/
  gpui-component/                   # vendored + patched Longbridge GPUI component library
```

## Components

| Component | Approach |
|---|---|
| GUI framework | `gpui` 0.2.x (from Zed) + `gpui-component` (Longbridge, vendored + patched) |
| Terminal emulator | `alacritty_terminal` 0.25 + `portable-pty` 0.9 — 3-thread pipeline with off-main-thread VTE parsing |
| Markdown editor | `gpui-component` editor widget + `ropey` + `tree-sitter-md`, custom TODO.md highlight pass |
| Syntax highlighting | `tree-sitter` 0.25.x + `tree-sitter-highlight` + per-language grammar crates (18 languages) |
| Symbol navigation | tree-sitter custom `outline.scm` queries (sourced from Zed) |
| Git diff | `git2` 0.20.x (vendored libgit2) + `similar`/`imara-diff`; diff generation on background thread |
| Problem tracking | Typed enums per view + wrapper enums at session/project level; push (hooks, BEL) + poll (diff, TODO, status.sh) merged every 2s |
| Claude reply capture | `/copy` command + clipboard polling (`arboard` crate) → `.jc/replies/<uuid>.md` |
| IPC | Unix socket (`~/.config/jc/jc.sock`) — multiple `jc .` invocations route to one running instance |
| File watching | `notify` 7.x with debouncing for TODO.md, code files, snippet file changes |
| Snippets | `~/.claude/jc.md` parsed into named snippets, insertable via picker |
| Status scripts | Optional `./status.sh` per project → `ScriptProblem` objects for problem navigation |
| Desktop notifications | macOS native: `UNUserNotificationCenter` banners (bundled .app) + dock bounce fallback |
| Persistent state | `~/.config/jc/` — project registry, window layout; session state in TODO.md |

## Session Model

### Hierarchy

`Workspace → ProjectState[] → SessionState[]`

Each `ProjectState` owns a TODO view, diff view, code view, and a `HashMap<SessionId, SessionState>` keyed by numeric ID. Each `SessionState` owns a Claude terminal, a general terminal, and an optional UUID.

The workspace has an active project with an active session. The active session drives which terminals appear in the panes. Switching sessions swaps pane contents without disconnecting terminals.

### Session Lifecycle

- **Project init:** The app reads TODO.md for session headings with UUIDs. Each is resumed via `claude --resume <uuid>`. If no sessions exist, a plain `claude` instance is launched.
- **New session:** From the session picker (Cmd-P), launch a fresh Claude Code instance. The UUID is auto-detected from the first hook event.
- **UUID assignment:** New sessions start without a UUID. The first hook event carries `session_id`; the app matches it to the pending session by project `cwd` and assigns the UUID. Constraint: one pending (UUID-less) session per project at a time.
- **`/clear` handling:** `SessionEnd(reason=clear)` is stashed. When `SessionStart(source=clear)` arrives within 10s, the session's UUID is updated in-place. No terminal relaunch needed — the same Claude process continues.
- **Disable:** Cmd-Shift-Backspace toggles the `[D]` prefix on a session heading. Disabled sessions skip auto-attach on startup but remain in the picker.
- **Expire:** `[X]` prefix marks sessions whose JSONL was garbage-collected by Claude.
- **Delete:** Manually change `[D]` to `[DELETED]` in TODO.md. The parser skips these entirely.

### Session Picker

Shows all sessions across all projects. Format: `project / label`. Markers: red problem count for sessions with problems, green `>` for active session, blue `+` for empty projects. Title bar shows `project > session` with `!` dirty marker and problem count.

## TODO.md Format

Each project has a single TODO.md. The app is the sole writer; external changes are detected and flagged.

```markdown
# TODO
(freeform project notes)

# Claude
## Refactor auth module
> uuid=abc123-def456-...
### Message 0
first instruction sent to claude
### Message 1
second instruction
### WAIT
Future notes accumulate here.
Annotations from diff/terminal/code views are appended.
```

### Heading Prefixes

| Prefix | Status | Behavior |
|---|---|---|
| `## Label` | Active | Normal session |
| `## [D] Label` | Disabled | Skipped on startup, visible in picker |
| `## [X] Label` | Expired | JSONL garbage-collected, not attachable |
| `## [DELETED] Label` | Deleted | Skipped entirely by parser |

### Comment Formats

From any view, Cmd-K annotates a selection. Comments are appended below WAIT:

- **From diff or code view:** `* <file>:<start_line>-<end_line> --- Comment text`
- **From terminal:** `* TERMINAL\n\`\`\`\n[selected content]\n\`\`\`\nComment text`

## Terminal Architecture

The terminal emulator (`jc-terminal/`) uses `alacritty_terminal` for VTE parsing and grid state — not its GPU renderer. GPUI handles rendering via `render.rs`.

### Data Flow (3-Thread Pipeline)

1. **PTY reader thread** — blocking read loop on the PTY fd, 4KB chunks, sends via flume channel
2. **VTE parser thread** (`std::thread`) — receives bytes, coalesces with visibility-aware caps (64KB visible / 256KB hidden), runs `Processor::advance()` under `Mutex<Term>` lock, signals main thread
3. **Main-thread relay** — async task receives notifications, emits Bell events, calls `cx.notify()` for GPUI repaint (skipped when hidden)

### Render Pipeline (`paint_terminal`)

- **Pass 1:** Cell backgrounds — `paint_quad()` per cell with non-default bg color
- **Pass 1.5:** Selection highlight
- **Pass 2:** Text — one `shape_line()` per row (batches characters + style runs). ~25 calls/frame vs ~2000 per-cell.
- **Pass 3:** Cursor shape

### Performance

- **Off-main-thread VTE parsing:** The expensive `Processor::advance()` runs on a dedicated thread.
- **Dirty tracking:** `content_generation` counter skips bg+text passes when content hasn't changed.
- **Row-based shaping:** gpui's `LineLayoutCache` caches shaped lines across frames; unchanged rows are free.
- **Adaptive coalescing:** 64KB cap visible, 256KB hidden — prevents frame stalls while minimizing CPU for background terminals.

## Problem System

### Problem Sources

**Session-level** (owned by `SessionState`):

| Source | Problem | Layer | Trigger | Resolution |
|---|---|---|---|---|
| Claude terminal | `ClaudeProblem::Permission` | L0 | Hook: permission prompt | User interacts with session |
| Claude terminal | `ClaudeProblem::StopFailure` | L0 | Hook: API error | User interacts with session |
| General terminal | `TerminalProblem::Bell` | L1 | BEL character | User focuses terminal |
| TODO view | `AppTodoProblem::UnsentWait` | L2 | Content below WAIT | Content sent or removed |
| Session state | *(synthetic)* | L3 | Idle + has_ever_been_busy | User starts new work |

**Project-level** (owned by `ProjectState`):

| Source | Problem | Layer | Trigger | Resolution |
|---|---|---|---|---|
| Diff view | `DiffProblem::UnreviewedFile` | L1 | Dirty working tree | File marked reviewed |
| Script | `ScriptProblem` | L1 | `./status.sh` output | Script stops reporting |

### Type Design

```rust
// Per-view leaf enums (jc-core/src/problem.rs)
enum ClaudeProblem { Permission, StopFailure }
enum TerminalProblem { Bell }
enum DiffProblem { UnreviewedFile(PathBuf) }
enum AppTodoProblem { UnsentWait { label } }
struct ScriptProblem { rank: Option<i8>, file: PathBuf, line: Option<usize>, message: String }

// Wrapper enums
enum SessionProblem { Claude(ClaudeProblem), Terminal(TerminalProblem), Todo(AppTodoProblem) }
enum ProjectProblem { Diff(DiffProblem), Script(ScriptProblem) }
enum ProblemLayer { L0, L1, L2, L3 }
```

### Cmd-; Behavior

1. If L0 problems exist anywhere, jump to them (cross-session). Stores "home session" on first L0 jump.
2. When all L0 cleared, return to home session and cycle L1/L2/L3.
3. Within a layer, cycle individual problems; when exhausted, advance to next layer.
4. L2 is suppressed when Claude is busy or L1 problems exist (review before sending).

### Refresh Model

Problems are recomputed on a unified 2-second cycle:

- **Push sources** (hooks, BEL): Write into `pending_events: HashSet<PendingEvent>` on the session. Events persist until resolved.
- **Poll sources** (diff, TODO, status.sh): Computed fresh each cycle. Diff generation runs on a background thread.
- `refresh_problems()` merges both, replaces the full list, and only triggers `cx.notify()` when the count changes.

**Resolution:**
- *Implicit:* Condition no longer holds on next poll (diff clean, WAIT empty, script quiet).
- *Acknowledgment:* User switches to a session → `acknowledge()` clears pending events. Switching to Claude terminal specifically clears `TerminalBell`.

### Display

- **Title bar:** `! Project > Session` with problem count
- **Session picker:** Red count replaces green `>` marker for sessions with problems
- **Corner indicator:** Per-layer counts (e.g., `1 / 3 / 0 / 2`) with layer-specific colors

### Script Problems (`status.sh`)

Optional per-project script. The app runs it periodically and parses stdout:

```
file:line - message
file - message
3:file:line - message          # leading number = rank (lower = more important)
```

Runs with project root as cwd. Non-zero exit = no problems. Stderr ignored.

## Hook Server

Lightweight HTTP server on a random localhost port. Claude Code POSTs to `/jc-hook/<event>`:

| Route | Event |
|---|---|
| `prompt-submit` | User submitted prompt |
| `stop` | Claude stopped |
| `stop-failure` | API error |
| `notification` | idle_prompt, permission_prompt, auth_success, elicitation_dialog |
| `permission` | Permission prompt shown |
| `session-start` | Session started (source: clear/startup/resume/compact) |
| `session-end` | Session ended (reason: clear/logout/prompt_input_exit) |

The server correlates `SessionEnd(clear)` + `SessionStart(clear)` events within a 10-second window to emit a unified `SessionClear` event.

Project matching: hook payload includes `cwd`, matched against configured project paths. Session matching: `session_id` in payload matched against session UUIDs.

## Notifications

macOS native via `objc2`:

- **Banners:** `UNUserNotificationCenter` with sound. Requires bundled `.app` with bundle ID. Click routes to session via `session_id` in userInfo.
- **Dock bounce:** `NSApplication::requestUserAttention` as fallback for unbundled builds. Critical events (permission prompts) bounce repeatedly.

## IPC

Unix socket at `~/.config/jc/jc.sock`. Protocol: JSON messages. Primary use: `open_project` command so multiple `jc .` invocations converge to one running instance.
