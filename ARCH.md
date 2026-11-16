# Architecture

## Project Structure

```
Cargo.toml                          # workspace root
data/
  dark_theme.toml                   # unified dark theme (Tomorrow Night)
  light_theme.toml                  # unified light theme (Tomorrow)
  fonts/                            # bundled Lilex font (Regular, Bold, Italic, BoldItalic)
scripts/
  bundle.sh                         # release build + jc.app and jcc.app bundles + icon + codesign
  update-outline-queries.sh         # fetch outline.scm files from Zed repo
  update-gpui-component.sh          # re-vendor gpui-component from cargo cache + apply patches
  backup-todos.sh                   # one-time TODO.md.bak snapshot before the log-bound sweep
jc-core/                            # data model + config persistence
  src/lib.rs, background.rs, cira.rs, claude.rs, config.rs, flavor.rs,
      lifecycle.rs, model.rs, replace.rs, scratch.rs, shared_file.rs, theme.rs, todo.rs,
      hooks.rs, hooks_settings.rs
  tests/fixtures/cira/                # `cira status --json` captured from a dev daemon (make-status.sh)
jc-terminal/                        # embedded terminal emulator
  src/lib.rs, colors.rs, input.rs, terminal.rs, pty.rs, render.rs, settle.rs, view.rs
  examples/terminal_window.rs
jc-app/                             # binary: CLI + GPUI app
  src/main.rs, app.rs, outline.rs, language.rs, ipc.rs, file_watcher.rs, notify.rs
  src/views/
    workspace/{mod,own,sessions,pickers,render}.rs   # own: jc's lifecycle; sessions: jcc's
    pane.rs, picker.rs, project_state.rs, session_state.rs
    code_view.rs, todo_view.rs, scratch_view.rs
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
| Saving and external edits | `jc_core::shared_file::SharedFile` (three-way merge with `diffy` 0.4) behind every `CodeView`: at each save, and when a watched file changes under unsaved edits (see **Saving** below) |
| Claude sessions | jc: `claude --session-id <uuid>` or `--resume <uuid>` in each Claude pane, uuids minted by jc (`uuid` v4). jcc: Claude Code background sessions owned by Cira, requests through `cira session …` and reads of `cira status --json` (`jc_core::cira`), `claude attach` and the read-only `claude agents --json` (`jc_core::background`) — argument vectors, off the main thread |
| Cira (jcc) | `cira status --json` polled on a thread of its own (`jc_core::cira::poll`): pending counts and session states; the daemon's change feed followed on another, for the scratch pads' saves (`jc_core::scratch::feed`) |
| IPC | Unix socket (`jc.sock` in the config dir) — multiple `jc .` (or `jcc .`) invocations route to one running instance of that app |
| File watching | `notify` 7.x with debouncing, per open `CodeView` (which is also what backs `TodoView`) |
| Desktop notifications | macOS native: `UNUserNotificationCenter` banners (bundled .app) + dock bounce fallback |
| Persistent state | `~/.config/jc/` (jcc: `~/.config/jcc/`, both overridden by `$JC_CONFIG_DIR`) — project registry, window layout, jcc's `sessions.toml`; session state in the app's TODO file (`Flavor::todo_path`) |

## Session Model

### Hierarchy

`Workspace → ProjectState[] → SessionState[]`

Each `ProjectState` owns a TODO view and a `HashMap<SessionId, SessionState>` keyed by numeric ID. Each `SessionState` owns a Claude terminal, a general terminal, a code view, its heading's id (`key`), the Cira id (`cira`), the job short id its attach client runs on (`bg`, empty while none runs), Cira's last view of the session (`cira_session`), and the conversation uuid hooks report (`uuid`, learned, never written to TODO.md). `ProjectState::code_view` is a convenience accessor for the active session's code view.

The workspace has an active project with an active session. The active session drives which terminals appear in the panes. Switching sessions swaps pane contents without disconnecting terminals.

**Active pane:** real keyboard focus is the source of truth for which pane is "active". The pane border and Cmd-[/Cmd-] navigation derive from `focused_pane_index()` (which queries gpui focus); `active_pane_index` is only a cache, kept in sync by an `on_focus_in` subscription per pane (resolved by pane *entity*, not a captured index, so `set_layout` reordering can't make it stale) and resynced before cache-dependent actions like Cmd-Enter/Cmd-S.

### Two Apps: jc and jcc

One binary, two apps (Cira PLAN TMUX-07): `jc_core::flavor::Flavor` is chosen in `main` from `JC_FLAVOR` (`jc`/`jcc`) or else the program's file name (`jcc` → `Flavor::Jcc`, else `Flavor::Jc`; `jcc.app` names its executable `jcc`) and read through `Flavor::current()`. What each app keeps apart (TODO file, config dir, hook URLs, bundle) is tabled once, in README.md ("How jc and jcc stay apart"); in code, `Flavor` names the app and decides those (`config_subdir`, `hook_prefix`, `todo_path`, `id_kind`, `owns`), and `Workspace::flavor` decides who runs a session: jc in `workspace/own.rs` (`Start::Own` carries the `claude` command line, `jc_core::claude::claude_argv`), jcc in `workspace/sessions.rs` (`Workspace::cira_client`, jcc's only `cira` client).

**TODO files.** `Flavor::todo_path` is the only place a TODO path is made: jc's is `<project>/TODO.md`; jcc's is `todo.md` beside the Cira project's scratch pad, `Status::todo_for` (`<home>/projects/<name>/todo.md`, `home` and the project holding the directory both from `cira status --json`), so jcc's `TodoView` starts with no file and `Workspace::open_todos` opens it at the first read that names the project (`ProjectState::open_todo`, which then starts its sessions). `lifecycle::plan_todos` decides per read: open; register (a directory no Cira project holds, once, with `cira project add <name> --path <dir>`, `Cira::add_project`), but only for a project added in this run (`jcc <dir>`, or to `state.toml`: `ProjectState::added`), never for one listed before, so a project removed in Cira does not come back at the next start, and never at the home directory, which Cira refuses (Cira PLAN SES-07); the TODO pane says why (`lifecycle::unregistered_text`, and Cira's own refusal, exit 5, as its answer: `lifecycle::unregistered_why`); close an open todo.md that Cira no longer names for the project (`TodoView::close`), so no save makes it again; or leave closed when another jc project already holds that todo.md (a directory inside it in the same Cira project), so no session starts or attaches twice; that pane says where to work (`TodoView::set_waiting`). Once open, it follows Cira at each read (`Workspace::sync_todos`, `lifecycle::todo_sync`, Cira PLAN TMUX-09): a live session Cira runs for the project that no heading names (one made with the CLI or the page) gets a heading under `# Cira` and a pane, and a heading whose session Cira removed is marked `[DELETED]`, keeping its messages, with a notice in the window's banner; both are recorded in `sessions.toml` as already acted on, so neither becomes a request. Not while the TODO has unsaved edits or a session of it is being asked for. Until a TODO opens, new sessions and transcript adoption are refused (while Cira is down, saying the Cira banner's cause). Refusals beep and say why in a window banner for a few seconds (`Workspace::refuse`). Sessions Jay starts from jcc are dangerous with `--from-jay` (`NewSession::for_jay`, Q-168). Both files hold sessions as `##` headings under a section heading (`Flavor::sections`): jc's `# Claude`; jcc writes `# Cira` in its own file and still reads `# Claude` there (files it made before, blocks pasted from jc); `todo::parse(text, flavor)` returns only the headings the flavor owns (`Flavor::owns`: jc skips a `> cira=` one), `todo::bind_cira_at` writes nothing in a jc document, `todo::update_session_uuid_at` nothing in a jcc one.

`Workspace` dispatches each lifecycle entry point on `Workspace::flavor` (`create_new_session`, `adopt_session`, `adopt_transcript`, `toggle_session_disabled`, hooks); jc builds no Cira poll, no Cira kick and no `sessions.toml` saver (each an `Option`), and observes no pane exits. `hooks_settings` edits `settings.local.json` under an exclusive lock on the project's `.claude` directory, replacing it through `jc_core::replace`, removing only its own prefix's entries, so both apps' hooks coexist; one that does not parse is left alone and the project named in a banner (`Workspace::hook_warning`). jcc's `Workspace::migrate` and `cira_adopt_transcript` refuse while `claude::conversation_running` finds a foreground `claude` on the uuid: a process whose executable (resolved by `proc_pidpath`; the native installer's `…/claude/versions/<v>`), or whose node script (`@anthropic-ai/claude-code`), is Claude Code, by its command line, or by `~/.claude/sessions/<pid>.json` after a `/clear`.

**Signals.** SIGINT/SIGTERM run `on_signal` (in `main.rs`), which only writes the signal to a pipe; a thread of its own reads it, removes this app's hooks from every project and its socket, then re-raises. The removal takes the `.claude` lock, which the interrupted thread may hold.

### Saving

Every editor pane (`CodeView`, and the `TodoView` built on one) keeps its file through `jc_core::shared_file::SharedFile`, which holds the pane's *base*: what it last read from or wrote to the file. The module doc there states the rules; in short:

- `SharedFile::save` re-reads the file under a lock on its directory; if it differs from the base, it three-way merges the pane's text with it (the merge then shows in the pane, and `TodoView::save` re-parses). A conflict writes nothing. The file is replaced through `jc_core::replace::write_replacing_with`, which checks just before the rename that the file still holds what was read, and starts over if not.
- The watcher (`file_watcher::watch_dir`, on the directory of the file a symlink names) reports changes; `SharedFile::disk_changed` ignores one whose text equals the base (the pane's own save), shows anything else, merged with unsaved edits.
- A conflict stops every save to the file from that pane — `TodoView::save`, the automatic saves of sends and schedules, and save-all at quit, which lists the file as unsaved — and each of those writes the pane's text to the conflict's copy, `<file>.conflict-<unix secs>`, instead (`SharedFile::keep`). It ends at Cmd-R (`SharedFile::reload`), at a clean merge, or when the disk returns to the base. `SharedFile::banner` is the pane's banner (`views::file_banner`): the conflict and its copy, or why the last save failed (a lock not got within `LOCK_WAIT`, a read, attempts used up), until a save works.

### Session Lifecycle: jc

- **UUID assignment:** jc mints the UUID. `SessionState::create` takes a `Launch` discriminant: `Launch::New` spawns `claude --session-id <uuid>`, `Launch::Resume` spawns `claude --resume <uuid>`. The two are not interchangeable — `--resume` on a UUID with no transcript exits with "No conversation found", and `--session-id` needs the UUID to be free — so every adoption path picks between them with `ProjectState::launch_for`, which tests `jc_core::claude::transcript_in` over the project's buckets. `SessionState::uuid` is therefore always populated — there is no pending, UUID-less state and no `cwd`-based matching of a first hook event. A `> dangerous` metadata line appends `--dangerously-skip-permissions` to either form.
- **Project init:** `ProjectState::create` bounds every session's message log (`TodoView::truncate_logs`), then adopts **every** `SessionStatus::Active` session in TODO.md, each with the flag `launch_for` picks for it; `[D]` and `[DELETED]` are skipped, as is any heading repeating a `> uuid=` already adopted this pass (a copy-pasted block would otherwise put two `claude` processes on one transcript). The active session is the one with the most recent `> last=` timestamp (ties fall back to document order).
- **New session:** `Workspace::own_new_session` (Cmd-Shift-P → "New session", or confirming an empty project in Cmd-P) generates a UUID, spawns with `Launch::New`, and writes the heading plus `> uuid=` into TODO.md in the same update. The label is just `New Session`; it is a display name, not an address, and need not be unique (see **Session addressing** below).
- **Legacy blank UUIDs:** an older jc created a heading with an empty `> uuid=` and filled it from the first hook event; if that hook never landed, the heading is unbound while its real conversation lives under a UUID Claude chose. jc does **not** mint one at startup — doing so would bind the heading to an empty conversation and orphan the real one, rewriting the file before the user saw anything. Such headings are simply not launched, and both pickers list them as adoptable; `Workspace::own_adopt_session` mints and records the UUID at that point, so it is a choice the user made. (The real transcript, if there is one, is still offered separately under "unattached".)
- **Session addressing:** every operation that *writes* to a heading keys on the UUID; a label is a display name and is never an address. The one exception is a heading an older jc left unbound (empty `> uuid=`), addressed by *verified position*. **The `todo::SessionKey` doc comment in `jc-core/src/todo.rs` is the canonical statement of why and how — read it there, not here.** (Historically jc kept labels unique instead, via `unique_label` at each creation point plus a `dedupe_labels` startup repair; `DESIGN.md` records why that could not hold.)
- **Adopt:** `Workspace::own_adopt_session` starts a TODO.md session that isn't running, with the flag `launch_for` picks for it. If the heading was `[D]`, the prefix is cleared first.
- **No expiry.** A session whose transcript Claude has garbage-collected is relaunched, not retired: `launch_for` sees the missing `<uuid>.jsonl` and claims the UUID with `--session-id`, so the heading, message log, and WAIT block stay usable behind a fresh conversation. There is no `SessionStatus::Expired`. The transcript check still exists, but it now only picks a launch flag — a wrong answer costs one bad spawn, not a permanently retired session.
- **Transcript buckets:** Claude stores each session at `~/.claude/projects/<bucket>/<uuid>.jsonl`, where `<bucket>` is the launch cwd with every non-`[A-Za-z0-9-]` char mapped to `-`. A git worktree at `<root>/.claude/worktrees/<branch>` therefore lands in its **own sibling bucket** `<encoded_root>--claude-worktrees-<branch>`. So a project's sessions can be spread across the root bucket and any worktree buckets. `jc_core::claude::session_dirs` enumerates all of them. Two callers depend on that: the Cmd-Shift-P scan for unattached JSONL sessions (a worktree conversation is invisible there otherwise), and `ProjectState::launch_for`, where missing a bucket makes `transcript_in` answer `false` and jc claims an already-live UUID with `--session-id` — so this is now load-bearing, not just discovery.
- **`/clear` handling:** `SessionEnd(reason=clear)` is stashed by the hook server. When `SessionStart(source=clear)` arrives within 10s for the same project, the pair is emitted as `HookEventKind::SessionClear` and `Workspace::own_session_clear` rewrites the session's UUID in memory and in TODO.md. No terminal relaunch — the same Claude process continues. This is the only post-launch UUID change.
- **Disable:** Cmd-Shift-Backspace in the session picker carries a `todo::SessionKey` through `SessionPickerResult::ToggleDisabled` to `todo::toggle_session_disabled_at`, flipping the `[D]` prefix on that heading. The key is the UUID, or — for a heading that has none — its position in the parsed document, re-verified against the live document at write time (label unchanged, still unbound) so the write cannot land on a heading that moved since the picker snapshot. Disabled sessions skip auto-attach on startup but remain in the picker.
- **Delete:** Manually change `[D]` to `[DELETED]` in TODO.md. jc's sessions ignore these entirely.
- **Quit** asks first when a session is working (`SessionState::busy`), since closing jc ends its `claude` processes.

### Session Lifecycle: jcc

Every Claude pane is a client of a Claude Code **background session that Cira owns** (Cira DESIGN 12): Cira starts, forks, stops and removes the job, and the pane runs `claude attach <short>` to the job Cira names. The job draws at the size the last of its clients sent, and an attach client sends one only when its size changes (measured, Cira PLAN TMUX-08), so a Claude pane that Cira's page also attaches to sends its size again whenever it is focused, shown or its window becomes active (`TerminalConfig::shared_size`, `PtyHandle::reassert_size`: one row less, then its own). Quitting jc only detaches; the sessions keep running, and the next start attaches again. The general terminal runs the `argv` of `cira session shell` (`jc_core::cira::Cira::session_shell`, an argument list, never through a shell), attaching to the session's shell in Cira's tmux server; `jc_core::lifecycle::ShellPane` decides. A pane asks only once it has been laid out (`TerminalView::is_sized`): one never shown would attach at 80x24, and attached clients share the latest one's size, the page's included. When an ask fails, or the shared client fails quickly three times in a row (`lifecycle::SHARED_FAILURES`, saying the last line tmux printed), it runs jc's own shell with `CIRA_SESSION`, headed "Terminal · not shared", until that shell exits. Only a removal holds the pane for good; another exit-5 refusal is asked again. Both panes wait out the same pause after a child exits (`lifecycle::Backoff`). A child's OSC 52 store (a drag in Cira's tmux, which has the mouse on) is written to the macOS clipboard. jc runs no lifecycle verb of `claude` itself: `jc_core::background::Claude` has only `attach_argv`, `agents` and `wait_settled`. Every request is a `cira session …` call (`jc_core::cira::Cira`, which says `--from-jay` where Cira requires it: jc acts for Jay), run in the jc project's directory with `--project` naming the Cira project that lists the session. The decisions are pure functions in `jc_core::lifecycle`, driven by `jc-app/src/views/workspace/sessions.rs`.

- **Addressing.** A heading names its session by `> cira=<id>`, the id Cira minted at `cira session new`, which is also the session's `CIRA_SESSION`. It is the address of every TODO.md write (`SessionState::key`). The job's short id changes on every mode change, so it is never written down; a heading written by an older jc carries `> uuid=` instead (`todo::IdKind::Uuid`) until it is migrated. A label is a display name and never an address; a heading an older jc left unbound (empty `> uuid=`) is addressed by *verified position*. **The `todo::SessionKey` doc comment in `jc-core/src/todo.rs` is the canonical statement of why and how — read it there, not here.** (Historically jc kept labels unique instead, via `unique_label` at each creation point plus a `dedupe_labels` startup repair; `DESIGN.md` records why that could not hold.)
- **The Cira poll.** `cira status --json` is read every 2 s on a thread of its own (`jc_core::cira::poll`), each read stamped with when it began; an attach client's exit, and each answered request, ask for a read at once. `Workspace::on_cira` applies a read: counts and markers (`ProjectState::apply_cira`), then `drive_sessions`, `reconcile_marks` and `sync_labels`. While Cira does not answer, one banner across the window says why and what to do (`cira::Down`), and stderr says it once per cause (`cira::Reach`, kept in the `views::CiraReach` global that every view reads when it renders); the TODO panes add none of their own meanwhile (`todo_view::waiting_banner`), and a refusal that would say the same is not shown twice. No pane attaches anything new; a running attach client is left alone.
- **Project init:** once its todo.md opens (`ProjectState::open_todo`, at the first status read naming the project), `ProjectState::restore_sessions` bounds every session's message log (`TodoView::truncate_logs`), then creates a `SessionState` for **every** `SessionStatus::Active` heading with an id, each with an empty Claude pane; `[D]`, `[DELETED]` and unbound headings are skipped, as is any heading repeating an id already adopted this pass (a copy-pasted block would otherwise attach two panes to one session, or migrate one `> uuid=` twice). The active session is the one with the most recent `> last=` timestamp (ties fall back to document order). The general terminal starts at the first status read after Cira has minted the session's id and the pane has been shown (`SessionState::bind`, `Workspace::drive_shells`), so it always names the session.
- **Attaching** (`Workspace::drive_sessions`, `lifecycle::pane_action`). At each read, a pane with no attach client attaches to the job Cira names (`job.short`), except: while an operation runs or a fork is due it waits (attaching then would resume the job being replaced); a session stopped through Cira after the pane last attached, or after Jay last asked, is held until its job runs again (Cira's listing, or a turn its hooks began after the stop) or Jay asks (adopting it, or Cmd-Shift-P → "Attach this session"); a removed session, a failed start and a job Claude Code no longer lists are held, the pane saying why. A job Claude Code no longer lists is never started by jcc on its own, a session stopped through Cira included: its pane offers Cmd-Shift-P → "Start this session again" and says what Cira will do (`lifecycle::can_start_again`, Cira PLAN TMUX-16), which sends `cira session start` (`Cira::start`, Jay's: Cira records the request again in the session's mode, lists Claude's jobs and starts a new one continuing its conversation, clearing a stop of Jay's) and prints Cira's own answer; the pane then attaches to the job Cira binds. Not offered while another session holds its job or continues it (the pane names that one), nor while an operation or fork is under way. A pane whose client runs on another short id (a fork) is re-attached to the new job. After a client exits, the pane waits for a read begun after the exit and for a backoff (`lifecycle::attach_delay`: 1 s, doubling to 30 s while it keeps exiting quickly). Every attach marks the session not `live` until `claude agents --json` (`Workspace::watch_live`) or a hook from it says it runs.
- **Migration** (`Workspace::migrate`): a legacy `> uuid=U` heading is moved to Cira once: `cira session new --resume=U --wait`, with `--dangerous` when the heading had `> dangerous`, else `--auto` (every `session new` jc sends names its mode: Cira starts dangerous by default and refuses a `--resume` with no mode, Cira PLAN SES-04); a conversation with no transcript gets a fresh session (Cira's exit 5, and jc cannot find the transcript either: Cira's 5 has other causes, and starting fresh drops the heading's only link to the conversation). Only once Cira has a job for it is the heading rewritten to `> cira=`, dropping `> uuid=` and `> dangerous` (`todo::bind_cira_at`); a failed start leaves the heading as it was and the session no longer counts as adopted (`SessionState::failed`), so adopting it again (or the next jc start) asks again, which Cira answers with the same session and retries. A `cira session new` without `--resume` is not idempotent: when it goes unanswered (exit 124, or jc stops waiting), jc looks in the next read for the session Cira may have made under that label (`lifecycle::lost_session`) instead of asking again. A request whose heading went meanwhile drops the pane; a `[DELETED]` one is removed in Cira too.
- **New session:** `Workspace::create_new_session` (Cmd-Shift-P → "New session", or confirming an empty project in Cmd-P) adds a session whose pane waits, asks `cira session new`, and writes the heading (`todo::insert_session_heading`) as soon as Cira answers with the id; the pane then attaches once Cira has bound a job. The label is just `New Session`; it is a display name, not an address, and need not be unique. Until the heading exists the session has no address: Cmd-Enter beeps.
- **Adopt:** `Workspace::adopt_session` gives a TODO.md heading that has no pane one. If the heading was `[D]`, the prefix is cleared first; adopting is Jay asking, so the pane attaches even a session stopped through Cira. An unbound heading gets a fresh session, and its id line is written then — a choice the user made, never made at startup. `Workspace::adopt_transcript` continues an unattached transcript with `cira session new --resume --wait` under a new heading.
- **Sending.** Cmd-Enter and scheduled sends type into the attach only once the session is `live` and the attach has drawn its first frame (`Workspace::ready_for_input`). A Cmd-Enter before that is recorded as a past-due scheduled send (`### Message N @jc(<now>)`, `todo::defer_message`) rather than as sent, so it survives a quit and is delivered by the scheduled-send path once the session is up; a second one beeps, as for any pending schedule. Scheduled sends are armed only for sessions that are ready, so a past-due one fires as soon as its session is up rather than after a fixed grace.
- **The mode** is Cira's (Q-155): Cmd-Shift-P → "Make this session dangerous" / "auto" sends `cira session mode ID MODE --from CURRENT` (`Workspace::toggle_active_dangerous`; a conflict, the mode changed elsewhere, is shown in the pane). Cira forks the job once the session is idle; the pane follows the fork to the new short id. The session picker shows only `bypass (drift)`, when the hooks report bypass Cira did not ask for; the mode itself is the Claude pane's to show (Cira PLAN TMUX-20).
- **Transcript buckets:** Claude stores each conversation at `~/.claude/projects/<bucket>/<uuid>.jsonl`, where `<bucket>` is the launch cwd with every non-`[A-Za-z0-9-]` char mapped to `-`. A git worktree at `<root>/.claude/worktrees/<branch>` therefore lands in its **own sibling bucket** `<encoded_root>--claude-worktrees-<branch>`. `jc_core::claude::session_dirs` enumerates all of them, for the Cmd-Shift-P scan for unattached transcripts. Whether a conversation can be resumed is Cira's check (`cira session new --resume`, exit 5 without a transcript).
- **`/clear` handling:** `SessionEnd(reason=clear)` is stashed by the hook server. When `SessionStart(source=clear)` arrives within 10s for the same project, the pair is emitted as `HookEventKind::SessionClear` and `Workspace::handle_session_clear` moves the session's in-memory uuid. TODO.md does not change: the Cira id survives a `/clear`.
- **Disable:** Cmd-Shift-Backspace in the session picker carries a `todo::SessionKey` through `SessionPickerResult::ToggleDisabled` to `todo::toggle_session_disabled_at`, flipping the `[D]` prefix on that heading. The key is the heading id, or — for a heading that has none — its position in the parsed document, re-verified against the live document at write time (label unchanged, still unbound) so the write cannot land on a heading that moved since the picker snapshot. Disabling a running session detaches its pane; it is refused with a beep while a `cira session new` for it is in flight, which must first write the heading's id. Disabled sessions skip auto-attach on startup but remain in the picker.
- **Markers become requests** (`Workspace::reconcile_marks`, `lifecycle::mark_request`): when a heading's marker differs from the one jc last acted on for its Cira id (`TodoDocument::marks`; a copied block counts its most alive heading), `[D]` sends `cira session stop` and `[DELETED]` (by hand) `cira session rm`. The marker acted on is remembered per id in `sessions.toml` in jc's config dir (`lifecycle::Records`, read and written off the main thread), so an edit made while jc was closed applies at its next start; with no record the marker becomes the record and asks nothing. A `sessions.toml` that does not parse is renamed `sessions.toml.bad-<unix secs>` and jc starts with no records (`config::load_sessions`), so what changed while it was closed is not acted on and is recoverable only from that file; one it cannot read or move leaves jc acting on no markers or labels, so nothing is saved over it. A heading that is `[D]` or `[DELETED]` has no pane. A TODO with unsaved edits waits, so a half-typed marker is no request; a request that recorded nothing (Cira unreachable or unanswering, its store unable to write: `CallError::retry`) is asked again at a later read; one refused for good (a removed session asked to stop) counts as answered.
- **Labels** (`Workspace::sync_labels`, `lifecycle::label_sync`): the label is Cira's (Q-159). A heading renamed in TODO.md is sent with `cira session label`; a label Cira changed (its page) is written into the heading (`todo::rename_heading_at`); when both moved, or jc has no record of them agreeing, Cira's wins. One heading speaks for each id (`TodoDocument::label_headings`), so a copied block does not rename the session back and forth, and a label that would read as a marker (`[DELETED] x`) or split the heading is not written into it. The label both last agreed on is remembered in `sessions.toml`.
- **Quit:** closing jc detaches every pane (the attach clients are hung up) and leaves the sessions running, so working sessions are no reason for the quit confirm; only unsaved merge conflicts are, and a scratch pad that could not be saved.
- **Start** (Cira PLAN TMUX-14). `sessions.toml` also keeps when each pane last attached, while it stays attached (`Record::attached_at`, `lifecycle::note_attach`: an attach sets it, a hold clears it, so quitting, which only hangs up, leaves it set) and what was on screen (`Records::focus`, `Workspace::note_focus`). At a pane's first decision once the records are read, an attached one gets `attach_since` = that time (`lifecycle::since_at_start`), so `pane_action` attaches it again although its job no longer runs (retired idle, which Cira does not record as a stop), and holds it when Cira's `stopped_at` (`cira session stop`, from Jay, the page or the CLI) is later (Q-275). `Workspace::restore_focus` then shows the focused session once a todo.md is open (`lifecycle::start_project`: the focused project, else the first open one), as a switch would; any switch made before settles it.
- **Default panes** (`pane::default_kinds`): Claude, TODO, and on the right jcc's session Terminal (Cira PLAN TMUX-12), jc's Global TODO, which is also jcc's while there is no session.
- **Scratch** (Cira PLAN TMUX-11). Each project's `ScratchView` (`views/scratch_view.rs`) shows the pad of the Cira project holding it (`Workspace::name_scratch_pads` at each read). It talks to Cira's daemon itself, on the store's socket, since the daemon is the only writer of the store and the `cira` CLI has no scratch command; the protocol and the rules (no typing before a read, one request at a time, merge on a newer copy, Cmd-R / Cmd-S on a conflict) are `jc_core::scratch`'s. Cmd-S reaches it through `Workspace::save_file` (the editor's own context binds it). It is read when its project is named, when shown (`show_in_pane`), when the window becomes active, and when Cira's change feed says it was saved elsewhere (Cira PLAN TMUX-15): `Workspace::follow_scratch_feed` follows `GET /v1/events` on the same socket (`jc_core::scratch::feed`, a thread of its own that reconnects after 2 s), a `scratch` event naming the pad's project and a version it does not hold reads it again (`Pad::stale`, `ScratchView::told`: its own save, told back after its answer, reads nothing; one ahead of the answer costs one more read), and `resync`, which begins every connection, reads every pad again; at quit `Workspace::save_dirty_of` saves a pad still owed, waiting for the answer (`ScratchView::flush`).

### Session Picker

`SessionPickerDelegate` shows all sessions across all projects as `project / label`.

| Marker | Entry |
|---|---|
| bold red `!` | A permission dialog is waiting, seen or not |
| red `*` | It finished a turn since it was last on screen |
| grey `…` | Its turn is over; background agents are still running |
| grey `x` | The session process has ended |
| green `>` | The active session |
| yellow `~` | In TODO.md but not running — confirming adopts it |
| grey `~` | Same, but disabled (`[D]`) |
| blue `+` | Registered project with no sessions |

Each row's right side also carries the open Cira blocks its session holds (`4 blocks`, `jc_core::cira::blocks_label`): an attached session's from its Cira session's `open` (`SessionState::open_blocks`), a heading not attached by its Cira id (`ProjectState::cira_blocks`), a project with no sessions its project's. Cira counts a block for the session holding it, the end of a continuation chain (Cira PLAN TMUX-19).

**jc's marker is terminal activity:** red `*` alone (the legend says `activity`), for a running session with unseen Claude terminal output. The rows below this paragraph's table that name `!`, `…` and `x`, the blocks count and the mode badge are jcc's.

**jc: the activity marker.** `TerminalView` keeps two counters: `output_batches`, bumped by the VTE background thread once per coalesced batch it parses (hidden terminals still parse, so a backgrounded session still records activity), and `seen_batches`, the value `clear_output_seen` snapshotted. `TerminalView::has_unseen_output` is just the two being unequal. The counters themselves are private to `jc-terminal`; the app crate sees only `has_unseen_output`, which the picker reads on each session's *Claude* terminal, and `clear_output_seen`. The baseline is taken from two places.

The first is `Workspace::switch_to_session`, on the session being switched *away from*. Clearing on the way out rather than the way in is what makes the marker mean "since you last had it on screen" no matter how the switch was made (picker, Cmd-\`, notification click). The session currently on screen is excluded by the picker itself (`is_active`), since you are looking at it.

The second is the **launch-settle window**. A freshly spawned `claude` prints a banner and, when resuming, replays its transcript; that is jc launching the session, not work done while you were away. `SessionState::create` asks for one by setting `TerminalConfig::launch_settle` on the Claude terminal (the general terminal leaves it `None`), and `TerminalView::new` installs the `SettleWindow` before spawning the relay that drives it — a window poked in afterwards could arrive after the relay had already parked on an unbounded receive, and would then never be serviced.

The relay already blocks on the batch signal to schedule repaints, so it is woken by exactly the event the window needs: there is no second task and no poll. Each pass asks the window what to do, then waits for either the next batch or the window's own deadline, whichever comes first (gpui's `FutureExt::with_timeout`, which polls the receive first so a batch wins the tie). A batch restarts the quiet wait; a deadline means the window is due to decide.

The mechanism is `settle.rs`'s and the numbers are the caller's: `SessionState::CLAUDE_LAUNCH_SETTLE` sets 4 s of quiet after the child has printed *something*, bounded at 30 s. What counts as settled is a fact about the Claude CLI, not about terminal emulation, so the durations live in `jc-app` and travel in as a `LaunchSettle`. `SettleWindow` itself is pure — it observes `Instant`s and answers `Clear`/`Wait`/`Done` — so the policy is unit-tested without a PTY, against fixture durations rather than a second copy of the shipped ones.

Four properties the window holds, each of which was once a bug:

- A child that has printed *nothing* is slow to start, not quiet, so it is never baselined early — otherwise its banner lands above the baseline.
- The quiet wait restarts at every batch, so the pause `claude --resume` takes between banner and transcript replay does not end the window.
- The give-up path *clears* rather than merely stopping. A child still printing re-marks itself on its next batch, whereas a marker left set never retires (only switching away clears one), and `has_activity` is the picker's primary sort key.
- A session is baselined exactly once, so real output arriving after it settles is never silently swallowed.

`SessionState::mark_user_input` — called from `Workspace::send_to_terminal`, `Workspace::fire_scheduled_send` and the `PromptSubmit` hook — ends the window through `TerminalView::cancel_launch_settle` *without* clearing: once you have given a session work, everything it prints is a response to something you asked for. The window is per terminal, so a project restored mid-run by `Workspace::open_project` gets its own.

**jcc: the marker comes from session state, not terminal output** (Cira PLAN TMUX-06; why: `DESIGN.md`, "The Session Marker Comes From State"). `jc_core::cira::marker` holds the rule. Its inputs are three `SessionState` fields: `cira_state` (from the last `cira status --json`, matched by `cira::Project::session` within the Cira project that contains the jc project, `cira::Status::project_for`), `hook_state` (jc's own hooks, `SessionState::note_hook`, stamped with the time the hook server received them), and `seen`, stamped by `Workspace::switch_to_session` on the *outgoing* session, which is what makes it "since you last had it on screen" however the switch was made. Until jc has switched away once, the heading's `> last=` stands in.

The poll: `Workspace::new` starts `cira::poll`, which calls `Cira::status` (`cira status --json`, given `Cira::TIMEOUT`) on its own thread every `CIRA_POLL`; a main-thread task applies each read (`Workspace::on_cira`, `ProjectState::apply_cira`) and notifies only when something shown changed. Any failure reads as unreachable, with its cause (`cira::Down`): counts disappear and markers fall back to the hooks. `cira` is found on `PATH`, like `claude`, and finds its daemon through `CIRA_HOME` in jc's environment, else `~/.cira`.

**Sort order.** Groups, in order: current project's sessions, other projects' sessions, unadopted sessions (current project first), empty projects, and finally the active session, which always sorts last since you are already there. A session showing a permission dialog, in any project, sorts above all of these. Within both session groups — this project's and the others' — finished sessions come first; "where did the work move while I was elsewhere" is mostly a cross-project question. Every sub-group then sorts by `> last=` descending (`picker::sort_rank`).

The title bar shows `project > session`; jcc's is `jcc [X blocks] · project > session [N blocks]` (`jc_core::cira::header`, Cira PLAN TMUX-18): X the open blocks of every attached session (`Workspace::attached_blocks`), N the shown one's, each left out at zero. jcc's Dock badge (`Workspace::update_dock_badge`, `NSDockTile.badgeLabel` through `notify::set_badge`, Cira PLAN TMUX-21) counts each Cira project holding an open project once (`Status::open_blocks_of`). Per-pane headers carry the `[+]` dirty marker.

jcc's keys: Cmd-` rotates only among projects with open blocks (`next_slot`; none: nothing happens), Cmd-\\ toggles the right pane between Scratch and what it showed (`pane::scratch_toggle`, remembered per pane in `Workspace::scratch_before`), Cmd-, opens `Status::page_url` (`web_port`, else 7812; the project `encodeURIComponent`-escaped) with `open -a /Applications/Safari.app` on a thread of its own.

### Project Actions Picker

`ProjectActionsPickerDelegate` (Cmd-Shift-P) is scoped to the active project and lists, in order:

1. **Dormant** (`*`, cyan) — TODO.md sessions that aren't currently running, most recent `> last=` first. Includes headings an older jc left unbound (empty `> uuid=`); adopting one starts a fresh background session for it.
2. **New session** (`+`, green) — ask Cira for a session under the display name `New Session`.
3. **The active session** — "Make this session dangerous" / "auto" (`!`, red: asks Cira for the other mode) and "Attach this session" (`>`, green: attach its pane again, even after a stop made through Cira), and, while Claude Code no longer lists its job, "Start this session again" (`>`, green: `cira session start`, Cira PLAN TMUX-16).
4. **Unattached** (`~`, yellow) — `<uuid>.jsonl` files in any of the project's transcript buckets that no heading accounts for, newest mtime first: not a heading's legacy `> uuid=`, not the current uuid of a running session, and not a session Cira lists (by its uuid, or a job short id the uuid starts with: `ProjectState::cira_known`). The label is the first informative user message in the transcript (`extract_first_user_summary`), plus a relative age; two transcripts that open with the same prompt therefore get the same display name, which is harmless — the heading is addressed by its `> cira=`.

## TODO.md Format

Each project has a TODO for each app (jc's `TODO.md` in the project, jcc's `todo.md` in Cira's store, see **TODO files**), and the app keeps it open in an editor pane, so you write to it as freely as the app does. External changes are picked up by the `CodeView` file watcher and three-way merged against unsaved edits (see **Saving**); `ProjectState::sync_sessions_from_todo` then pulls any renamed label back into the running sessions. It matches on the heading id alone, so hand-editing a `> cira=` line does *not* re-point a running session (see Periodic Work).

```markdown
# TODO
(freeform project notes)

# Cira
## Refactor auth module
> cira=cira-8e565675baa4
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
headings inside the app's section (`Flavor::sections`) that it owns. A `## [DELETED] …` heading is skipped by
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
| `## [DELETED] Label` | Deleted | Not a session; in jcc its background job is stopped and removed (`TodoDocument::deleted`) |

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
| `> cira=<id>` | The id Cira minted for the session, the heading's address and its `CIRA_SESSION` (the general terminal's shell has it, shared or jc's own). Written by jc when Cira answers `cira session new`; never changed. Must be the first metadata line. |
| `> uuid=<id>` | Legacy: the address an older jc wrote. Replaced by `> cira=` when jc moves the session to Cira. |
| `> last=<unix-secs>` | Timestamp of the last `Cmd-Enter` send. Used to sort the Cmd-P / Cmd-Shift-P pickers by recency. Updated automatically. |
| `> dangerous` | Legacy: read once, to start the migrated session in that mode, then removed. The mode is Cira's (Cmd-Shift-P). |

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
does two things:

1. `Workspace::reconcile_schedules` — arm a timer for every pending `@jc(...)` marker on a
   heading whose session is ready for input (`Workspace::ready_for_input`), idempotent through the `armed_schedules` set, which is keyed `(project path, heading id, fire time)`.
   A marker on a dormant, starting or unbound heading is not dropped: it keeps its pending
   highlight in the file, and is armed on the first tick after its session comes up (firing at
   once if its time has passed).
2. `ProjectState::sync_sessions_from_todo` on each project — match each TODO entry to a
   running session **by heading id only**, and copy the entry's label onto it, so a heading renamed
   by hand is picked up. There is deliberately no label fallback: labels are not unique, and a
   TODO entry for a session that is not running would otherwise claim a same-labelled running
   session. Returns whether anything changed, and only then does the loop `cx.notify()`. Nothing
   is re-pointed: only the label moved, and the TODO view's highlight keys on the id.

The scans themselves are in memory. The Cira poll is a loop of its own on its own thread, and
what jc does about sessions follows each read (see Session Lifecycle); every `cira` and `claude`
command runs on its own thread (`Workspace::run_blocking`). Everything else is
event-driven — hooks arrive over the hook server's channel, external file changes over the
per-`CodeView` `notify` watcher, and terminal output over the VTE thread's flume channel.

## Terminal Architecture

The terminal emulator (`jc-terminal/`) uses `alacritty_terminal` for VTE parsing and grid state — not its GPU renderer. GPUI handles rendering via `render.rs`.

### Data Flow (3-Thread Pipeline)

1. **PTY reader thread** — blocking read loop on the PTY fd, 4KB chunks, sends via flume channel; at EOF it reaps the child and reports its exit code (`pty::PtyChild`), so an exited child never lingers as a zombie. The thread holds its own dup of the master, so dropping the view does not by itself hang the child up: `PtyHandle`'s `Drop` sends it SIGHUP. Children are argument vectors (`ChildSpec`), never split on whitespace.
2. **VTE parser thread** (`std::thread`) — receives bytes, coalesces with visibility-aware caps (64KB visible / 256KB hidden), runs `Processor::advance()` under `Mutex<Term>` lock, bumps `output_batches` (read only by `has_output_since_spawn`: has the attach drawn its first frame), answers `PtyWrite` events, signals main thread
3. **Main-thread relay** — async task waiting on the batch signal; calls `cx.notify()` for GPUI repaint (skipped when hidden). When the child exits it emits `ChildEvent::Exited` with how long it ran; nothing restarts it (for `claude attach`, the workspace asks Cira what to attach to next).

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

Unix socket at `~/.config/jc/jc.sock` (`$JC_CONFIG_DIR/jc.sock` when that is set; see `config::config_dir`). Protocol: JSON messages. Primary use: `open_project` command so multiple `jc .` invocations converge to one running instance. jc switches to the project and raises its window (`Workspace::open_project`); jcc only lists it (`Flavor::ipc_switches`, `Workspace::list_project`, Cira PLAN TMUX-13), saying so in the window's banner and on the second invocation's stdout.

The app also follows its own `state.toml` (`workspace/state_file.rs`, Cira PLAN TMUX-10): `file_watcher::watch_dir` on the config directory; once the file holds still for `STATE_SETTLE` it is read and its paths made canonical off the main thread (`AppState::resolve`; one that does not exist is neither opened nor a reason to close, and the window says so), and only the newest read is applied (`config::Reads`). `Resolved::change_from` compares it with the open projects. A project it adds opens without being switched to (`Workspace::add_project`), its hooks served and installed as for one open at start (`Workspace::install_project_hooks`, also for `jc .`). One it drops has its buffers saved, then `config::leaving` decides: it closes (`Workspace::close_project`: panes and sessions go, its hooks stop being served and are removed), or Jay is asked first (`Closing::Project`) when a buffer could not be saved, or, in jc, when it has sessions, since closing ends their `claude`; Cancel lists it in the file again, and a file that lists it again while the dialog is up dismisses it. A removal that needs asking while another dialog is up waits, and is asked when that dialog closes; jcc's other closes do not wait. The last project open stays. Hook installs and removals run in order on one thread (`hooks_settings::hook_worker`), so a project opened and closed in quick succession ends without hooks; the SIGINT/SIGTERM cleanup reads `state.toml` too. An app's own write of `state.toml` lists what it has open, so following it changes nothing.
