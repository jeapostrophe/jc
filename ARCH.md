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
  src/lib.rs, background.rs, cira.rs, claude.rs, config.rs, model.rs,
      theme.rs, todo.rs, hooks.rs, hooks_settings.rs
  tests/fixtures/cira/                # `cira status --json` captured from a dev daemon (make-status.sh)
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
| Claude sessions | Claude Code background sessions (`claude --bg`), driven through `claude agents --json`, `attach`, `stop` and `rm` by `jc_core::background` — argument vectors, off the main thread |
| Cira | `cira status --json` every 2 s on a thread of its own (`jc_core::cira::poll`): pending counts and session states |
| IPC | Unix socket (`~/.config/jc/jc.sock`) — multiple `jc .` invocations route to one running instance |
| File watching | `notify` 7.x with debouncing, per open `CodeView` (which is also what backs `TodoView`) |
| Desktop notifications | macOS native: `UNUserNotificationCenter` banners (bundled .app) + dock bounce fallback |
| Persistent state | `~/.config/jc/` — project registry, window layout; session state in TODO.md |

## Session Model

### Hierarchy

`Workspace → ProjectState[] → SessionState[]`

Each `ProjectState` owns a TODO view and a `HashMap<SessionId, SessionState>` keyed by numeric ID. Each `SessionState` owns a Claude terminal, a general terminal, a code view, its heading's id (`key`), the background session it attaches to (`bg`), and the conversation uuid hooks report (`uuid`, learned, never written to TODO.md). `ProjectState::code_view` is a convenience accessor for the active session's code view.

The workspace has an active project with an active session. The active session drives which terminals appear in the panes. Switching sessions swaps pane contents without disconnecting terminals.

**Active pane:** real keyboard focus is the source of truth for which pane is "active". The pane border and Cmd-[/Cmd-] navigation derive from `focused_pane_index()` (which queries gpui focus); `active_pane_index` is only a cache, kept in sync by an `on_focus_in` subscription per pane (resolved by pane *entity*, not a captured index, so `set_layout` reordering can't make it stale) and resynced before cache-dependent actions like Cmd-Enter/Cmd-S.

### Session Lifecycle

Every Claude pane is a client of a Claude Code **background session**: the session runs under Claude Code's per-user supervisor, and the pane runs `claude attach <short>`. Quitting jc only detaches; the sessions keep running, and the next start attaches again. The general terminal stays jc's own shell. Facts about the CLI that this rests on (measured on Claude Code 2.1.322, Cira SP-03/SP-06) are recorded where they are used, in `jc-core/src/background.rs`.

- **Addressing.** A heading names its session by `> bg=<short>`, the short id `claude --bg` prints. It is stable across `/clear`, stop, respawn and Claude Code's idle retirement, so it is the address of every TODO.md write (`SessionState::key`) and what `claude attach` takes. A heading written by an older jc carries `> uuid=` instead (`todo::IdKind::Uuid`) until it is migrated. A label is a display name and never an address; a heading an older jc left unbound (empty `> uuid=`) is addressed by *verified position*. **The `todo::SessionKey` doc comment in `jc-core/src/todo.rs` is the canonical statement of why and how — read it there, not here.** (Historically jc kept labels unique instead, via `unique_label` at each creation point plus a `dedupe_labels` startup repair; `DESIGN.md` records why that could not hold.)
- **Stable id.** `> cira=<id>` is a v4 uuid jc mints when it is about to start a session for a heading that has none (`session_state::Heading::new`), so the general terminal starts with it; it is written to the heading when the session is bound. It reaches Claude only through `--settings '{"env":{"CIRA_SESSION":…}}'` (the environment of `claude --bg` never reaches the session) and is exported to the general terminal. Unlike the short id it survives a `> dangerous` change.
- **Project init:** `ProjectState::create` bounds every session's message log (`TodoView::truncate_logs`), then creates a `SessionState` for **every** `SessionStatus::Active` heading with an id, each with an empty Claude pane; `[D]`, `[DELETED]` and unbound headings are skipped, as is any heading repeating an id already adopted this pass (a copy-pasted block would otherwise attach two panes to one session, or migrate one `> uuid=` twice). The active session is the one with the most recent `> last=` timestamp (ties fall back to document order). `Workspace::launch_sessions` then asks `claude agents --json --all` once, off the main thread, and gives each pane what `background::plan_launch` decides:
  - a `> bg=` Claude Code knows (running, stopped or retired) is attached — attach resumes a stopped one;
  - a `> bg=` Claude Code no longer knows gets a fresh session under the same heading (**no expiry**: the heading, message log and WAIT block stay usable behind a new conversation);
  - a legacy `> uuid=` is **migrated once**: `claude --bg --resume <uuid>` when its transcript is on disk (the short id is the uuid's prefix and the conversation continues), a fresh session when it is not, an attach when a background job is already on that conversation — and nothing at all when an interactive `claude` holds it (an older jc still running), because `--resume` would fork a copy. The pane says why, and the session stops counting as adopted (`SessionState::failed`), so picking the heading again retries; so does a start that failed.
  
  The same pass stops the running job of every `[D]` heading and removes (`claude stop` + `claude rm`) the job of every `[DELETED]` heading that still has one — skipping any short id another heading still names as a session (a copied block; `TodoDocument::removable_ids`, `stoppable_ids`). A `[DELETED]` heading that still says `> uuid=` names no job and is left alone.
- **Starting a session** (`Workspace::start_session`, `background::Claude::start`): `claude --bg -n <label> --settings … [--resume <uuid>] [--dangerously-skip-permissions]` in the project directory, then `claude agents --json --all` until the job is live. A start that settles `failed` is removed and retried once. `Workspace::finish_launch` then writes `> bg=` (and `> cira=`) onto the heading — rewriting a `> uuid=` or older `> bg=` line in place, or inserting the heading for a session that had none (`todo::bind_bg_at`, `todo::insert_session_heading`) — and attaches the pane.
- **New session:** `Workspace::create_new_session` (Cmd-Shift-P → "New session", or confirming an empty project in Cmd-P) adds a session whose pane waits, starts it, and writes the heading once `claude --bg` has printed the short id. The label is just `New Session`; it is a display name, not an address, and need not be unique. Until the heading exists the session has no address: Cmd-Enter beeps.
- **Adopt:** `Workspace::adopt_session` starts a TODO.md session that isn't running, through the same plan as project init. If the heading was `[D]`, the prefix is cleared first; attaching resumes its stopped job. An unbound heading gets a fresh session, and its id line is written then — a choice the user made, never made at startup. `Workspace::adopt_transcript` continues an unattached transcript with `--bg --resume` under a new heading.
- **The attach client.** The Claude pane's terminal is made with `respawn`: when `claude attach` exits — Ctrl+Z, Esc from the ← agents view, or a `claude stop` from anywhere — `TerminalView` reaps it and runs it again after a backoff (1 s, doubling to 30 s while it keeps exiting quickly), and attaching resumes a stopped session. Every (re)start marks the session not `live` until `claude agents --json` (`Workspace::watch_live`) or a hook from it says it runs. When `claude agents` no longer lists the job at all (removed elsewhere), the pane stops re-attaching and the heading gets a fresh session, as at startup.
- **Sending.** Cmd-Enter and scheduled sends type into the attach only once the session is `live` and the attach has drawn its first frame (`Workspace::ready_for_input`). A Cmd-Enter before that is recorded as a past-due scheduled send (`### Message N @jc(<now>)`, `todo::defer_message`) rather than as sent, so it survives a quit and is delivered by the scheduled-send path once the session is up; a second one beeps, as for any pending schedule. Scheduled sends are armed only for sessions that are ready, so a past-due one fires as soon as its session is up rather than after a fixed grace.
- **`> dangerous`.** A background job keeps the options it was started with, so `Workspace::reconcile_background` compares each heading's `> dangerous` with the mode its session runs in (`SessionState::dangerous`) and, on a change, recreates it (`Workspace::change_dangerous`, `background::Claude::toggle_dangerous`): detach, `claude stop`, start a copy with `--dangerously-skip-permissions` or `--permission-mode default` (never both) that resumes the conversation when it has a transcript, rewrite `> bg=`, `claude rm` the old job. A change that fails re-attaches the old job and is not retried until the line changes again. A change made while jc is not running is not noticed: jc assumes a session runs in its heading's mode.
- **Transcript buckets:** Claude stores each conversation at `~/.claude/projects/<bucket>/<uuid>.jsonl`, where `<bucket>` is the launch cwd with every non-`[A-Za-z0-9-]` char mapped to `-`. A git worktree at `<root>/.claude/worktrees/<branch>` therefore lands in its **own sibling bucket** `<encoded_root>--claude-worktrees-<branch>`. `jc_core::claude::session_dirs` enumerates all of them, for the Cmd-Shift-P scan for unattached transcripts and for the transcript checks that decide between `--resume` and a fresh start.
- **`/clear` handling:** `SessionEnd(reason=clear)` is stashed by the hook server. When `SessionStart(source=clear)` arrives within 10s for the same project, the pair is emitted as `HookEventKind::SessionClear` and `Workspace::handle_session_clear` moves the session's in-memory uuid. TODO.md does not change: the short id survives a `/clear`.
- **Disable:** Cmd-Shift-Backspace in the session picker carries a `todo::SessionKey` through `SessionPickerResult::ToggleDisabled` to `todo::toggle_session_disabled_at`, flipping the `[D]` prefix on that heading. The key is the heading id, or — for a heading that has none — its position in the parsed document, re-verified against the live document at write time (label unchanged, still unbound) so the write cannot land on a heading that moved since the picker snapshot. Disabling a running session detaches its pane, then runs `claude stop` (detach first, see `SessionState::attach`); it is refused with a beep while the session's start or `> dangerous` change is in flight, which must first bind the heading to the job it starts. Disabled sessions skip auto-attach on startup but remain in the picker.
- **Delete:** Change a heading's prefix to `[DELETED]` by hand. The parser lists such headings in `TodoDocument::deleted`, not as sessions; jc detaches, stops and removes (`claude rm`) the job of one that is running, and removes the job of any other at the next start.
- **Quit:** closing jc detaches every pane (the attach clients are hung up) and leaves the sessions running, so working sessions are no reason for the quit confirm; only unsaved merge conflicts are.

### Session Picker

`SessionPickerDelegate` shows all sessions across all projects as `project / label`.

| Marker | Entry |
|---|---|
| bold red `!` | A permission dialog is waiting, seen or not |
| red `*` | It finished a turn since it was last on screen |
| grey `~` (on a running session) | Its turn is over; background agents are still running |
| grey `x` | The session process has ended |
| green `>` | The active session |
| yellow `~` | In TODO.md but not running — confirming adopts it |
| grey `~` | Same, but disabled (`[D]`) |
| blue `+` | Registered project with no sessions |

Each row's right side also carries its project's open Cira blocks (`4 blocks`, `jc_core::cira::blocks_label`), and the TODO pane header says `TODO · 4 blocks` (`blocks_suffix`).

**The marker comes from session state, not terminal output** (Cira PLAN TMUX-06; `DESIGN.md`, "The Session Marker Comes From State"). `jc_core::cira::marker` decides it from three inputs on `SessionState`:

- `cira_state`: the session's `state` and `state_since` from the last `cira status --json`, matched by `> cira=` id, else short id, else uuid (`cira::Project::session`) within the Cira project that contains the jc project (the deepest, Cira's own rule: `cira::Status::project_for`). `None` while Cira is unreachable or does not know the session.
- `hook_state`: the last state jc's own hooks reported and when it began (`SessionState::note_hook`, `cira::HookState::from_hook`): `PromptSubmit` working, `Stop` idle, `PermissionPrompt` dialog, `StopFailure` failed. `SessionStart` and `IdlePrompt` are not turns and change nothing.
- `seen`: when jc last switched away from the session (`Workspace::switch_to_session`, on the outgoing session — stamping on the way out is what makes it "since you last had it on screen" however the switch was made). Until then the heading's `> last=` stands in.

Cira's state wins; jc's hooks are used when Cira has none, and a `StopFailure` newer than Cira's `state_since` overrides it, since Cira has no state for an API error and still says working. `*` needs an idle time after `seen`; with neither `seen` nor `> last=` a session is not marked. The session on screen shows only `!`.

`Workspace::new` starts the poll: `cira::poll` runs `Cira::status` (`cira status --json`, killed after `Cira::TIMEOUT`, 3 s, because `cira` itself waits 12 s for a daemon that does not accept) on its own thread every 2 s and sends each result; a main-thread task applies it (`Workspace::apply_cira`, `ProjectState::apply_cira`) and notifies only when something shown changed. Any failure — `cira` not on `PATH`, no daemon, a failure exit, unparsable output — is `None`: counts disappear and markers fall back to the hooks. `cira` finds its daemon through `CIRA_HOME` in jc's environment, else `~/.cira`.

**Sort order.** Groups, in order: current project's sessions, other projects' sessions, unadopted sessions (current project first), empty projects, and finally the active session, which always sorts last since you are already there. A session showing a permission dialog, in any project, sorts above all of these. Within both session groups — this project's and the others' — finished sessions come first; "where did the work move while I was elsewhere" is mostly a cross-project question. Every sub-group then sorts by `> last=` descending (`picker::sort_rank`).

The title bar shows `project > session` only. Per-pane headers carry the `[+]` dirty marker.

### Project Actions Picker

`ProjectActionsPickerDelegate` (Cmd-Shift-P) is scoped to the active project and lists, in order:

1. **Dormant** (`*`, cyan) — TODO.md sessions that aren't currently running, most recent `> last=` first. Includes headings an older jc left unbound (empty `> uuid=`); adopting one starts a fresh background session for it.
2. **New session** (`+`, green) — start a background session under the display name `New Session`.
3. **Unattached** (`~`, yellow) — `<uuid>.jsonl` files in any of the project's transcript buckets that no heading accounts for, newest mtime first: not a heading's legacy `> uuid=`, not created by a heading's background session (the uuid starts with its `> bg=` short id), and not the current uuid of a running session. The label is the first informative user message in the transcript (`extract_first_user_summary`), plus a relative age; two transcripts that open with the same prompt therefore get the same display name, which is harmless — the heading is addressed by its `> bg=`.

## TODO.md Format

Each project has a single TODO.md, and jc keeps it open in an editor pane, so you write to it as freely as jc does. External changes are picked up by the `CodeView` file watcher and three-way merged against unsaved edits; `ProjectState::sync_sessions_from_todo` then pulls any renamed label back into the running sessions. It matches on the heading id alone, so hand-editing a `> bg=` line does *not* re-point a running session (see Periodic Work).

```markdown
# TODO
(freeform project notes)

# Claude
## Refactor auth module
> bg=35a13d68
> cira=8e565675-baa4-4f27-8d89-6ae89c563046
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
| `## [DELETED] Label` | Deleted | Not a session; its background job is stopped and removed (`TodoDocument::deleted`) |

`[X]` was a third prefix, written when Claude had garbage-collected a session's
transcript. Since such a session can always be restarted behind its heading, it
is merely dormant.
`parse` still accepts a legacy `## [X] Label` heading and maps it to
`SessionStatus::Disabled`; `toggle_session_disabled_at` rebuilds the heading from the
label, so re-enabling one normalises it to `## Label`. jc never writes `[X]`.

### Session Metadata

Lines starting with `> ` immediately after the `## Label` heading are parsed as session metadata. They may appear in any order; unknown keys are silently ignored (forward compatibility).

| Line | Meaning |
|---|---|
| `> bg=<short>` | The background session's short id, the heading's address. Written by jc when it starts the session; rewritten when a `> dangerous` change recreates it. Must be the first metadata line. |
| `> cira=<id>` | The stable session id, passed to Claude and the general terminal as `CIRA_SESSION`. Minted by jc the first time it starts the session; never changed. |
| `> uuid=<id>` | Legacy: the address an older jc wrote. Replaced by `> bg=` when jc first starts the session. |
| `> last=<unix-secs>` | Timestamp of the last `Cmd-Enter` send. Used to sort the Cmd-P / Cmd-Shift-P pickers by recency. Updated automatically. |
| `> dangerous` | The session runs with `--dangerously-skip-permissions`. Add or remove by hand while jc runs, and jc recreates the session in that mode (a new `> bg=`; the conversation is kept). |

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
does three things:

1. `Workspace::reconcile_schedules` — arm a timer for every pending `@jc(...)` marker on a
   heading whose session is ready for input (`Workspace::ready_for_input`), idempotent through the `armed_schedules` set, which is keyed `(project path, heading id, fire time)`.
   A marker on a dormant, starting or unbound heading is not dropped: it keeps its pending
   highlight in the file, and is armed on the first tick after its session comes up (firing at
   once if its time has passed).
2. `Workspace::reconcile_background` — recreate a session whose `> dangerous` changed, detach,
   stop and remove one whose heading became `[DELETED]`, and deliver a Cmd-Enter that was
   waiting for its session (see Session Lifecycle).
3. `ProjectState::sync_sessions_from_todo` on each project — match each TODO entry to a
   running session **by heading id only**, and copy the entry's label onto it, so a heading renamed
   by hand is picked up. There is deliberately no label fallback: labels are not unique, and a
   TODO entry for a session that is not running would otherwise claim a same-labelled running
   session. Returns whether anything changed, and only then does the loop `cx.notify()`. Nothing
   is re-pointed: only the label moved, and the TODO view's highlight keys on the id.

The scans themselves are in memory; every `claude` command they cause runs on its own thread
(`Workspace::run_blocking`). The Cira poll is a loop of its own on its own thread (see Session
Picker). Everything else is
event-driven — hooks arrive over the hook server's channel, external file changes over the
per-`CodeView` `notify` watcher, and terminal output over the VTE thread's flume channel.

## Terminal Architecture

The terminal emulator (`jc-terminal/`) uses `alacritty_terminal` for VTE parsing and grid state — not its GPU renderer. GPUI handles rendering via `render.rs`.

### Data Flow (3-Thread Pipeline)

1. **PTY reader thread** — blocking read loop on the PTY fd, 4KB chunks, sends via flume channel; at EOF it reaps the child and reports its exit code (`pty::PtyChild`), so an exited child never lingers as a zombie. The thread holds its own dup of the master, so dropping the view does not by itself hang the child up: `PtyHandle`'s `Drop` sends it SIGHUP. Children are argument vectors (`ChildSpec`), never split on whitespace.
2. **VTE parser thread** (`std::thread`) — receives bytes, coalesces with visibility-aware caps (64KB visible / 256KB hidden), runs `Processor::advance()` under `Mutex<Term>` lock, bumps `output_batches` (read only by `has_output_since_spawn`: has the attach drawn its first frame), answers `PtyWrite` events, signals main thread
3. **Main-thread relay** — async task waiting on the batch signal; calls `cx.notify()` for GPUI repaint (skipped when hidden). When the child exits it emits `ChildEvent::Exited` and, for a terminal made with `respawn`, starts the child again after the backoff.

A pane whose child asked for mouse reporting (`claude attach` runs fullscreen on the alternate screen, so jc has no scrollback for it) gets wheel events instead of scrollback movement, from the wheel and from Cmd-Alt scroll-other alike (`view::wheel_bytes`).

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
| `session-start` | Session started (source: clear/startup/resume/compact): the session is live |
| `session-end` | Session ended (reason: clear/logout/prompt_input_exit) |

Of the `notification` payloads, only `idle_prompt` and `permission_prompt` become events; `auth_success` and `elicitation_dialog` are dropped. The server correlates `SessionEnd(clear)` + `SessionStart(clear)` events within a 10-second window to emit a unified `SessionClear` event; every other `session-start` becomes `SessionStart`, and other `session-end` routes are ignored.

Project matching: hook payload includes `cwd`, matched against configured project paths. Session matching (`background::hook_matches`): a background session's conversation uuid starts with its short id until a `/clear`, so `session_id` matches a session when it is `<short>-…` or the uuid jc last learned for it (from a hook, a `/clear` pair, or `claude agents --json`). Hooks keep reaching a session that outlives a jc restart, because each start rewrites the project's hook entries with the new port. A hook from an open project that matches no session — typically one whose uuid changed while jc was not running — triggers a `claude agents --json` refresh (at most one per 30 s, since a `claude` jc does not manage also matches nothing), after which it is tried once more and otherwise dropped.

### Effect of Each Event

`Workspace::apply_hook_to_session` marks the owning session `live`, records the event's uuid as its current one, and records the state the event reports for the marker's fallback (`SessionState::note_hook`; see Session Picker). `SessionClear` is handled before dispatch, in `Workspace::handle_session_clear`.

## Notifications

A desktop notification fires only for a session that is **blocked on you**, and only when the jc window is inactive (`Workspace::window_active`):

| Event | Notification |
|---|---|
| `PermissionPrompt` | "Permission needed" |
| `StopFailure` | "API error" |

`Stop` and `IdlePrompt` are ambient — they raise no notification. What moved while you were away is reported by the session picker's marker instead, which costs no interruption.

macOS native via `objc2`:

- **Banners:** `UNUserNotificationCenter` with sound. Requires bundled `.app` with bundle ID. Click routes to the session via `session_id` in userInfo (`Workspace::switch_to_session_id`).
- **Dock bounce:** every notification also issues `NSApplication::requestUserAttention(CriticalRequest)`, before the authorization check — so unbundled builds, where banners are unavailable, still get the bounce. Since jc only notifies for a blocked session, a critical request is always the right level.

`notify::beep` is separate: the system alert sound as invalid-action feedback, e.g. a Cmd-Enter rejected because a scheduled send is pending.

## IPC

Unix socket at `~/.config/jc/jc.sock` (`$JC_CONFIG_DIR/jc.sock` when that is set; see `config::config_dir`). Protocol: JSON messages. Primary use: `open_project` command so multiple `jc .` invocations converge to one running instance.
