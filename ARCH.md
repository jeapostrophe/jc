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
  backup-todos.sh                   # one-time TODO.md.bak snapshot before the log-bound sweep
jc-core/                            # data model + config persistence
  src/lib.rs, claude.rs, config.rs, model.rs, theme.rs, todo.rs,
      hooks.rs, hooks_settings.rs
jc-terminal/                        # embedded terminal emulator
  src/lib.rs, colors.rs, input.rs, terminal.rs, pty.rs, render.rs, view.rs
  examples/terminal_window.rs
jc-app/                             # binary: CLI + GPUI app
  src/main.rs, app.rs, outline.rs, language.rs, ipc.rs, file_watcher.rs, notify.rs
  src/views/
    workspace/{mod,pickers,render}.rs
    pane.rs, picker.rs, project_state.rs, session_state.rs
    code_view.rs, todo_view.rs
    close_confirm.rs, keybinding_help.rs
  src/outline_queries/{rust,markdown,python,go,javascript,typescript}.scm
  examples/basic_window.rs
vendor/
  gpui/                             # vendored + patched gpui (InputRateTracker)
  gpui-component/                   # vendored + patched Longbridge GPUI component library
  patches/                          # patch files re-applied by update-gpui-component.sh
```

## Components

| Component | Approach |
|---|---|
| GUI framework | `gpui` 0.2.x (from Zed) + `gpui-component` (Longbridge, vendored + patched) |
| Terminal emulator | `alacritty_terminal` 0.25 + `portable-pty` 0.9 — 3-thread pipeline with off-main-thread VTE parsing |
| Markdown editor | `gpui-component` editor widget + `ropey` + `tree-sitter-md`, custom TODO.md highlight pass |
| Syntax highlighting | `tree-sitter` 0.25.x + `tree-sitter-highlight` + per-language grammar crates (18 languages) |
| Symbol navigation | tree-sitter custom `outline.scm` queries (sourced from Zed) |
| Project file listing | `git2` 0.20.x (vendored libgit2) — index + status, read once per Cmd-O picker open |
| External-edit merge | `diffy` 0.4 three-way merge in `CodeView::try_merge` when a watched file changes under unsaved edits |
| Session UUIDs | `uuid` v4, minted by jc and passed to the CLI as `--session-id` |
| IPC | Unix socket (`~/.config/jc/jc.sock`) — multiple `jc .` invocations route to one running instance |
| File watching | `notify` 7.x with debouncing, per open `CodeView` (which is also what backs `TodoView`) |
| Desktop notifications | macOS native: `UNUserNotificationCenter` banners (bundled .app) + dock bounce fallback |
| Persistent state | `~/.config/jc/` — project registry, window layout; session state in TODO.md |

## Session Model

### Hierarchy

`Workspace → ProjectState[] → SessionState[]`

Each `ProjectState` owns a TODO view and a `HashMap<SessionId, SessionState>` keyed by numeric ID. Each `SessionState` owns a Claude terminal, a general terminal, a code view, and a UUID (`String`, always present). `ProjectState::code_view` is a convenience accessor for the active session's code view.

The workspace has an active project with an active session. The active session drives which terminals appear in the panes. Switching sessions swaps pane contents without disconnecting terminals.

**Active pane:** real keyboard focus is the source of truth for which pane is "active". The pane border and Cmd-[/Cmd-] navigation derive from `focused_pane_index()` (which queries gpui focus); `active_pane_index` is only a cache, kept in sync by an `on_focus_in` subscription per pane (resolved by pane *entity*, not a captured index, so `set_layout` reordering can't make it stale) and resynced before cache-dependent actions like Cmd-Enter/Cmd-S.

### Session Lifecycle

- **UUID assignment:** jc mints the UUID. `SessionState::create` takes a `Launch` discriminant: `Launch::New` spawns `claude --session-id <uuid>`, `Launch::Resume` spawns `claude --resume <uuid>`. The two are not interchangeable — `--resume` on a UUID with no transcript exits with "No conversation found", and `--session-id` needs the UUID to be free — so every adoption path picks between them with `ProjectState::launch_for`, which tests `jc_core::claude::transcript_in` over the project's buckets. `SessionState::uuid` is therefore always populated — there is no pending, UUID-less state and no `cwd`-based matching of a first hook event. A `> dangerous` metadata line appends `--dangerously-skip-permissions` to either form.
- **Project init:** `ProjectState::create` bounds every session's message log (`TodoView::truncate_logs`), then adopts **every** `SessionStatus::Active` session in TODO.md, each with the flag `launch_for` picks for it; `[D]` and `[DELETED]` are skipped, as is any heading repeating a `> uuid=` already adopted this pass (a copy-pasted block would otherwise put two `claude` processes on one transcript). The active session is the one with the most recent `> last=` timestamp (ties fall back to document order).
- **New session:** `Workspace::create_new_session` (Cmd-Shift-P → "New session", or confirming an empty project in Cmd-P) generates a UUID, spawns with `Launch::New`, and writes the heading plus `> uuid=` into TODO.md in the same update. The label comes from `todo::unique_label` — `New Session`, then `New Session 2`, and so on — because TODO.md's *text* operations are label-keyed (see the duplicate-labels bullet below), even though session identity is the UUID.
- **Legacy blank UUIDs:** an older jc created a heading with an empty `> uuid=` and filled it from the first hook event; if that hook never landed, the heading is unbound while its real conversation lives under a UUID Claude chose. jc does **not** mint one at startup — doing so would bind the heading to an empty conversation and orphan the real one, rewriting the file before the user saw anything. Such headings are simply not launched, and both pickers list them as adoptable; `Workspace::adopt_session` mints and records the UUID at that point, so it is a choice the user made. (The real transcript, if there is one, is still offered separately under "unattached".)
- **Duplicate labels:** `todo::dedupe_labels` renames any heading whose label repeats an earlier one (via `todo::rename_session_at`, preserving `[D]`) in a single back-to-front rewrite, applied once at startup by `ProjectState::create`. TODO.md addresses a session by label nearly everywhere — `ensure_wait`, `send_selection`, the `@jc(...)` scheduled-send timers — and each resolves to the *first* match, so a duplicate silently redirects a send onto the wrong session. Every creation path already routes its label through `todo::unique_label`; this repairs files written before that.
- **Adopt:** `Workspace::adopt_session` starts a TODO.md session that isn't running, with the flag `launch_for` picks for it. If the heading was `[D]`, the prefix is cleared first.
- **No expiry.** A session whose transcript Claude has garbage-collected is relaunched, not retired: `launch_for` sees the missing `<uuid>.jsonl` and claims the UUID with `--session-id`, so the heading, message log, and WAIT block stay usable behind a fresh conversation. There is no `SessionStatus::Expired`. The transcript check still exists, but it now only picks a launch flag — a wrong answer costs one bad spawn, not a permanently retired session.
- **Transcript buckets:** Claude stores each session at `~/.claude/projects/<bucket>/<uuid>.jsonl`, where `<bucket>` is the launch cwd with every non-`[A-Za-z0-9-]` char mapped to `-`. A git worktree at `<root>/.claude/worktrees/<branch>` therefore lands in its **own sibling bucket** `<encoded_root>--claude-worktrees-<branch>`. So a project's sessions can be spread across the root bucket and any worktree buckets. `jc_core::claude::session_dirs` enumerates all of them. Two callers depend on that: the Cmd-Shift-P scan for unattached JSONL sessions (a worktree conversation is invisible there otherwise), and `ProjectState::launch_for`, where missing a bucket makes `transcript_in` answer `false` and jc claims an already-live UUID with `--session-id` — so this is now load-bearing, not just discovery.
- **`/clear` handling:** `SessionEnd(reason=clear)` is stashed by the hook server. When `SessionStart(source=clear)` arrives within 10s for the same project, the pair is emitted as `HookEventKind::SessionClear` and `Workspace::handle_session_clear` rewrites the session's UUID in memory and in TODO.md. No terminal relaunch — the same Claude process continues. This is the only post-launch UUID change.
- **Disable:** Cmd-Shift-Backspace in the session picker carries a `todo::SessionKey` through `SessionPickerResult::ToggleDisabled` to `todo::toggle_session_disabled_at`, flipping the `[D]` prefix on that heading. The key is the UUID, or the label for a heading that has none — an empty UUID is not an address, since every unbound heading shares it. Disabled sessions skip auto-attach on startup but remain in the picker.
- **Delete:** Manually change `[D]` to `[DELETED]` in TODO.md. The parser skips these entirely.

### Session Picker

`SessionPickerDelegate` shows all sessions across all projects as `project / label`.

| Marker | Entry |
|---|---|
| red `*` | Running session with unseen Claude terminal output |
| green `>` | The active session |
| yellow `~` | In TODO.md but not running — confirming adopts it |
| grey `~` | Same, but disabled (`[D]`) |
| blue `+` | Registered project with no sessions |

**Activity marker.** `TerminalView` keeps two `Arc<AtomicUsize>` counters: `output_batches`, bumped by the VTE background thread once per coalesced batch it parses (hidden terminals still parse, so a backgrounded session still records activity), and `seen_batches`, the value `clear_output_seen` snapshotted. `TerminalView::has_unseen_output` is just the two being unequal; `output_batches()` is exposed separately so a caller can tell whether output is *still arriving*. The picker reads it on each session's *Claude* terminal only. `TerminalView::clear_output_seen` is called from two places.

The first is `Workspace::switch_to_session`, on the session being switched *away from*. Clearing on the way out rather than the way in is what makes the marker mean "since you last had it on screen" no matter how the switch was made (picker, Cmd-\`, notification click). The session currently on screen is excluded by the picker itself (`is_active`), since you are looking at it.

The second is the **startup baseline**. A freshly spawned `claude` prints a banner and, when resuming, replays its transcript; that is jc launching the session, not work done while you were away. `Workspace::step_activity_baselines` runs on the 2 s reconcile tick and advances a per-session `ActivityBaseline` state machine: a session is baselined once its child has printed *something* and then held still for two consecutive ticks, or unconditionally after 15 ticks if it never settles. Requiring a non-zero count first keeps a slow-starting child from being baselined before its banner ever appears. The state lives on `SessionState`, not on the workspace, for two reasons: `Workspace::open_project` can restore a project's sessions at any point in the run, so a startup-only pass would mark every session of a later-opened project; and a session is baselined exactly once, so real output arriving after it settles is never silently swallowed.

**Sort order.** Groups, in order: current project's sessions, other projects' sessions, unadopted sessions (current project first), empty projects, and finally the active session, which always sorts last since you are already there. Within both session groups — this project's and the others' — sessions with activity come first; "where did the work move while I was elsewhere" is mostly a cross-project question. Every sub-group then sorts by `> last=` descending.

The title bar shows `project > session` only. Per-pane headers carry the `[+]` dirty marker.

### Project Actions Picker

`ProjectActionsPickerDelegate` (Cmd-Shift-P) is scoped to the active project and lists, in order:

1. **Dormant** (`*`, cyan) — TODO.md sessions that aren't currently running, most recent `> last=` first. Includes headings an older jc left unbound (empty `> uuid=`); adopting one mints and records its UUID.
2. **New session** (`+`, green) — mint a UUID and launch, under a `todo::unique_label` name.
3. **Unattached** (`~`, yellow) — `<uuid>.jsonl` files in any of the project's transcript buckets whose UUID appears nowhere in TODO.md, newest mtime first. The label is the first informative user message in the transcript (`extract_first_user_summary`), plus a relative age; adopting one runs that summary through `todo::unique_label`, since two transcripts often open with the same prompt.

## TODO.md Format

Each project has a single TODO.md, and jc keeps it open in an editor pane, so you write to it as freely as jc does. External changes are picked up by the `CodeView` file watcher and three-way merged against unsaved edits; `ProjectState::sync_sessions_from_todo` then pulls any renamed label back into the running sessions. It matches on UUID alone, so hand-editing a `> uuid=` line does *not* re-point a running session (see Periodic Work).

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
The next instruction is drafted here.
```

Everything below `### WAIT` is draft, not log: `parse` will not record a `### Message N`
heading that appears there (the user is quoting an old message). Reading one as a real
message would make the next send take its index, would let a quoted `@jc(...)` marker
block sends, and would put draft text inside the range the log truncator may delete.

### Message Log Bound

Each send keeps the most recent `todo::MAX_MESSAGES` (25) `### Message N`
entries in that session and drops the older ones, so TODO.md cannot grow without
limit. Indices are never renumbered, so a session settles into a sliding window
— `### Message 76` through `### Message 100`. A send truncates only the session
it writes to; the rest of the document is untouched.

The first launch after this landed removes a lot of text at once, and the sweep
is not undoable. `TodoView::truncate_logs` writes a one-time `TODO.md.bak` beside
each file before shortening it, and `scripts/backup-todos.sh` takes the same
snapshot from the shell just before launching, driven by the project list in
`~/.config/jc/state.toml` so it covers exactly what jc will sweep. Neither ever
overwrites an existing `.bak`: the first snapshot is the pre-truncation one and
the one worth keeping. The shell copy exists because the in-app one runs on a
path that only executes at startup — take it on the launch that first picks up
the sweep.

**`make.sh` currently requires `--backup-todos` or `--no-backup-todos` and
refuses to launch without one** (checked before the build, so a forgotten flag
costs a second rather than a compile). jc is a live-in tool and weeks can pass
between restarts, which is long enough to forget a flag that only matters once —
so there is no default, because the failure mode of a default here is silent and
permanent. **This is temporary**: once the snapshots exist, delete the marked
block in `make.sh` and `scripts/backup-todos.sh`.

Sessions that have gone quiet are caught by a startup sweep:
`todo::truncate_all_sessions` runs in `ProjectState::create` immediately after
the TODO.md buffer is read, before the session-restore loop. It applies the same bound to every session in the
document and is idempotent. It only reaches sessions the parser sees: `##`
headings inside the `# Claude` section. A `## [DELETED] …` heading is skipped by
`parse`, so such a session's log is never bounded and keeps whatever size it had.

Truncation stops at a message still carrying an undelivered `@jc(...)` marker, so
it can never silently cancel a scheduled send. A send cannot hit this (sends are
gated on `TodoSession::pending_scheduled`), but the startup sweep can, and the
clamp converges: the first sweep drops everything above the marker, leaving it at
index 0, after which every later sweep drops nothing. So a session holding a
pending schedule stays above the bound until that send fires.

### Heading Prefixes

| Prefix | Status | Behavior |
|---|---|---|
| `## Label` | Active | Normal session |
| `## [D] Label` | Disabled | Skipped on startup, visible in picker |
| `## [DELETED] Label` | Deleted | Skipped entirely by parser |

`[X]` was a third prefix, written when Claude had garbage-collected a session's
transcript. Since such a session is relaunchable under its own UUID with
`--session-id`, it is merely dormant.
`parse` still accepts a legacy `## [X] Label` heading and maps it to
`SessionStatus::Disabled`; `toggle_session_disabled_at` rebuilds the heading from the
label, so re-enabling one normalises it to `## Label`. jc never writes `[X]`.

### Session Metadata

Lines starting with `> ` immediately after the `## Label` heading are parsed as session metadata. They may appear in any order; unknown keys are silently ignored (forward compatibility).

| Line | Meaning |
|---|---|
| `> uuid=<id>` | Claude session UUID. Minted by jc when the session is created and written with the heading; rewritten only by `/clear`. |
| `> last=<unix-secs>` | Timestamp of the last `Cmd-Enter` send. Used to sort the Cmd-P / Cmd-Shift-P pickers by recency. Updated automatically. |
| `> dangerous` | When set, jc spawns this session's `claude` process with `--dangerously-skip-permissions`. Add manually; takes effect at next session spawn (relaunch jc, or re-adopt the session via Cmd-Shift-P). |

### Scheduled Messages

Primary use: auto-resume a session after a Claude usage-limit reset. Begin the WAIT
body with a `@jc(HH:MM)` marker (24h) and press Cmd-Enter. The message (text before the
cursor / selection, minus the marker) is moved into a `### Message N @jc(<datetime>)`
heading with the time **resolved to the next absolute occurrence**, e.g.:

```markdown
### Message 3 @jc(2026-07-13 07:30)
finish the parser refactor
### WAIT
```

Delivery to the Claude PTY is deferred to that instant. The message body under the
heading stays editable — jc delivers whatever it *currently* says at fire time, not a
schedule-time snapshot. On delivery the `@jc(...)` marker is dropped, leaving a plain
`### Message N` (that is the "delivered" indication); while pending, the heading is
highlighted in the `@keyword` color.

- **One at a time / all sends blocked.** While a session has a pending scheduled
  message, every Cmd-Enter (immediate or scheduled) is rejected with the system beep.
- **Cancel** by deleting the `@jc(...)` marker or the whole `### Message N` block.
  **Reschedule** by editing the datetime — `Workspace::reconcile_schedules` (and the
  startup scan) re-arms timers from the live markers, so edits in either direction take
  effect, including edits to an *earlier* time that the fire-time re-check alone can't catch.
- **Persistence & catch-up.** The marker lives in TODO.md, so timers re-arm on restart;
  a scheduled time that already passed while jc was closed fires immediately (after a
  brief grace so the resumed terminal is ready).
- `> last=` is stamped at delivery time, not when the message is queued.

## Periodic Work

One recurring task, `Workspace::start_schedule_reconcile_loop`, ticks every 2 seconds and
does three things, none of which touches the filesystem:

1. `Workspace::reconcile_schedules` — arm a timer for every pending `@jc(...)` marker on a
   heading whose session is actually *running*, idempotent through the `armed_schedules` set.
   A marker on a dormant or unbound heading is not dropped: it keeps its pending highlight in
   the file, and adopting the session arms it on the next tick (firing after the catch-up
   grace if its time has passed).
2. `Workspace::step_activity_baselines` — advance each session's `ActivityBaseline` state
   machine, discounting the launch banner and transcript replay from the Cmd-P activity
   marker (see Session Picker).
3. `ProjectState::sync_sessions_from_todo` on each project — match each TODO entry to a
   running session **by UUID only**, and copy the entry's label onto it, so a heading renamed
   by hand is picked up. There is deliberately no label fallback: labels are not unique, and a
   TODO entry for a session that is not running would otherwise claim a same-labelled running
   session and stamp the wrong UUID onto it, silently breaking every hook for it. Returns
   whether anything changed, and only then does the loop re-point the TODO view's active
   label and `cx.notify()`.

All three are in-memory scans: no filesystem access, no git, no subprocesses. Everything else is
event-driven — hooks arrive over the hook server's channel, external file changes over the
per-`CodeView` `notify` watcher, and terminal output over the VTE thread's flume channel.

## Terminal Architecture

The terminal emulator (`jc-terminal/`) uses `alacritty_terminal` for VTE parsing and grid state — not its GPU renderer. GPUI handles rendering via `render.rs`.

### Data Flow (3-Thread Pipeline)

1. **PTY reader thread** — blocking read loop on the PTY fd, 4KB chunks, sends via flume channel
2. **VTE parser thread** (`std::thread`) — receives bytes, coalesces with visibility-aware caps (64KB visible / 256KB hidden), runs `Processor::advance()` under `Mutex<Term>` lock, bumps `output_batches`, answers `PtyWrite` events, signals main thread
3. **Main-thread relay** — async task receives notifications and calls `cx.notify()` for GPUI repaint (skipped when hidden)

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

Of the `notification` payloads, only `idle_prompt` and `permission_prompt` become events; `auth_success` and `elicitation_dialog` are dropped. The server correlates `SessionEnd(clear)` + `SessionStart(clear)` events within a 10-second window to emit a unified `SessionClear` event; all other `session-start`/`session-end` routes are ignored.

Project matching: hook payload includes `cwd`, matched against configured project paths. Session matching: `session_id` in the payload is matched against session UUIDs — and since jc assigns those UUIDs, an event naming an unknown one belongs to a session jc doesn't manage and is discarded.

### Effect of Each Event

`Workspace::apply_hook_to_session` maps every non-`SessionClear` event onto the owning session's `busy` flag: `PromptSubmit` sets `busy`; `Stop`, `StopFailure`, `PermissionPrompt`, and `IdlePrompt` clear it. `SessionClear` is handled before dispatch, in `Workspace::handle_session_clear`.

## Notifications

A desktop notification fires only for a session that is **blocked on you**, and only when the jc window is inactive (`Workspace::window_active`):

| Event | Notification |
|---|---|
| `PermissionPrompt` | "Permission needed" |
| `StopFailure` | "API error" |

`Stop` and `IdlePrompt` are ambient — they update `busy` and nothing else. What moved while you were away is reported by the session picker's activity marker instead, which costs no interruption.

macOS native via `objc2`:

- **Banners:** `UNUserNotificationCenter` with sound. Requires bundled `.app` with bundle ID. Click routes to the session via `session_id` in userInfo (`Workspace::switch_to_session_id`).
- **Dock bounce:** every notification also issues `NSApplication::requestUserAttention(CriticalRequest)`, before the authorization check — so unbundled builds, where banners are unavailable, still get the bounce. Since jc only notifies for a blocked session, a critical request is always the right level.

`notify::beep` is separate: the system alert sound as invalid-action feedback, e.g. a Cmd-Enter rejected because a scheduled send is pending.

## IPC

Unix socket at `~/.config/jc/jc.sock`. Protocol: JSON messages. Primary use: `open_project` command so multiple `jc .` invocations converge to one running instance.
