# Plan

> **Labels:** **[T]** Trivial, **[E]** Easy, **[H]** Hard (own Claude session), **[D]** Design (needs human input)

### Window & Pane Management
- [ ] [H] Multi-window with shared session state

### Remote Workflow (CLI & Hooks)
- [ ] [H] `jc status` — JSON projects/sessions
- [ ] [E] `jc note` — append text below WAIT
- [ ] [E] External notification hook (ntfy/Pushover)

### Git Worktrees
- [ ] [H] Worktree creation/deletion via `git2`

### Energy / Performance
- [ ] [H] Thermal state throttling — read macOS `ProcessInfo.thermalState`, cap at 60fps under Serious/Critical (backport Zed PR #45638, needs ObjC FFI)

### Polish
- [ ] [H] End-to-end test: full workflow cycle
- [ ] [H] Graceful recovery from Claude crashes, terminal failures

### Automation
- [ ] [D] Auto-creating and running sessions

### Simplification: features removed  ✅ shipped (2026-08-28)
One item per thing that was asked for, so each can be checked off independently.
All landed in a single commit (`dadfc34`) rather than one commit each — that was a
mistake in how the work was sequenced, not a sign the items are entangled; each can
be reviewed on its own.

- [x] **[SIMP-01]** Remove problem tracking and navigation entirely. Deleted
      `jc-core/src/problem.rs` (`ProblemLayer` L0–L3, `ProblemTarget`, `SessionProblem`,
      `ProjectProblem`, ranks) and `jc-app/src/views/workspace/problems.rs` (the Cmd-;
      rotation); dropped the `cmd-;` binding, the title-bar problem count and its
      per-layer tooltip, per-session problem lists, `todo::validate`/`TodoProblem`, and
      the 2 s poll task that recomputed them.
- [x] **[SIMP-02]** Keep app notifications for permission prompts and API errors only.
      `Workspace::handle_hook_event` notifies on `PermissionPrompt` and `StopFailure`
      and nothing else; `Stop` and `IdlePrompt` still clear `busy` but never interrupt.
      `notify::notify` lost its `critical` parameter — every surviving notification is.
- [x] **[SIMP-03]** Remove `status.sh` support. Deleted `jc-core/src/status_script.rs`,
      `ScriptProblem`, the 10 s run interval, and the repo's own stub `status.sh`.
      (Answer to a question asked at the time: it existed only to feed the problem list.)
- [x] **[SIMP-04]** Remove the Git Diff view. Deleted `jc-app/src/views/diff_view.rs`,
      `PaneContentKind::GitDiff` (`ALL` is 5 wide now), the diff drill-down picker,
      git-log/commit browsing, and per-file "mark reviewed" (the Diff meaning of Cmd-R;
      Cmd-R still reloads in the Code view). Dropped the `similar` dependency.
- [x] **[SIMP-05]** Remove Cmd-D. Went with the diff view — `ToggleCodeDiff` and both its
      bindings are gone.
- [x] **[SIMP-06]** Remove the Cmd-K comment system. Deleted
      `jc-app/src/views/comment_panel.rs`, `CodeView::comment_context`, and
      `TodoDocument::comment_insert_offset`.
- [x] **[SIMP-07]** Remove the Cmd-Shift-K snippet picker. Deleted
      `jc-core/src/snippets.rs`, `SnippetPickerDelegate`, and the `~/.claude/jc.md`
      watcher.
- [x] **[SIMP-08]** Remove Cmd-Shift-C (copy Claude's reply). Deleted `CopyReply`, the
      `/copy` clipboard poll, and the `.jc/replies/` write path. Dropped `arboard`.
- [x] **[SIMP-09]** Remove Cmd-Shift-E (open in external editor). Deleted
      `OpenInExternalEditor` and its `zed path:line` spawn.
- [x] **[SIMP-10]** Cmd-P indicator becomes a boolean `*` for "this session has had
      activity". Defined as *any terminal output since the session was last on screen*:
      `TerminalView` counts parsed output batches (`output_batches`) against a baseline
      (`seen_batches`), `has_unseen_output` compares them, and
      `Workspace::switch_to_session` rebaselines on switch-**away** so the marker means
      the same thing however you left. A session's own launch output is discounted by
      `jc_terminal::SettleWindow`, driven by the terminal's own batch signal. The picker
      sorts activity-first in both the current project and the others.
- [x] **[SIMP-11]** Assign session UUIDs instead of detecting them. `create_new_session`
      mints a v4 UUID and spawns `claude --session-id <uuid>`, writing the heading and the
      UUID together; the "launch bare `claude`, fill the UUID in from the first hook"
      path and its `cwd`-matched fallback are gone, and `SessionState::uuid` is a plain
      `String`. `/clear` handling is unchanged (kept deliberately).
- [x] **[SIMP-12]** Drop the `[X]` expired state. `SessionStatus::Expired`,
      `todo::mark_session_expired` and the startup expiry pass are gone; a legacy
      `## [X] Label` heading reads as `[D]` and normalises to `## Label` when re-enabled.
      **Deviation worth checking:** the premise that `claude --resume` works on a deleted
      UUID is false — measured 2026-08-27, it prints `No conversation found with session
      ID: <uuid>` and exits, for a deleted *and* a never-used UUID. What does work is
      `--session-id` on that same UUID, which starts a fresh conversation under it. So the
      conclusion holds by a different route: a session whose transcript is gone is
      *relaunched*, not retired, and `ProjectState::launch_for` picks the flag from
      transcript presence. The transcript check therefore survives — it chooses a flag
      instead of deciding a session's fate.
- [x] **[SIMP-13]** Legacy headings with a blank `> uuid=` (written by the old
      detect-the-UUID flow) are **not** rewritten at startup: minting there would bind the
      heading to an empty conversation and orphan whatever real transcript it had. They
      are listed as adoptable in both pickers, and `adopt_session` mints and records the
      UUID at that point.

### Follow-ups from the simplify review  ✅ shipped (2026-08-28)
Both were flagged by the four-agent simplify pass over SIMP-01..SIMP-13 and done as
independent units in worktrees.

- [x] **[SIMP-14]** Address TODO.md headings by UUID, not by label. Every text operation
      that resolved a session by label — `insert_wait_section`, `send_from_wait`,
      `fire_scheduled`, `TodoView::{wait_line, send_selection, has_pending_schedule,
      ensure_wait, deliver_scheduled}`, the active-heading highlight, and
      `Workspace::armed_schedules` — took the FIRST heading with that label, and label
      uniqueness was only *repaired at startup* over a file you edit by hand. Pasting a
      duplicate heading mid-session silently misrouted sends until restart. Now keyed on
      the UUID, with `TodoDocument::session_by_uuid` rejecting the empty string so the
      family is safe by construction rather than by a repair pass. An unbound legacy
      heading is addressed by `SessionKey::Unbound { index, label }`, re-verified against
      the live document at use. Deleted: `dedupe_labels`, `rename_session_at`,
      `session_by_label` as an address, and `TodoDocument::wait_body_end_line` (a
      duplicate of `TodoWait::body_end_line` that was off by one line).
      `todo::unique_label` survives, explicitly cosmetic — it keeps the picker from
      showing identical rows and carries no invariant.
- [x] **[SIMP-15]** Move launch-settle detection into the terminal that sees the batches.
      The workspace was polling `output_batches` on the 2 s reconcile tick to decide when
      a freshly-spawned `claude` had stopped printing, with the settle policy expressed in
      tick counts whose meaning lived in another symbol. `jc_terminal::SettleWindow` now
      holds that policy against `Instant`s and is driven by the relay's own per-batch
      wake; the claude-specific durations live in `SessionState::CLAUDE_LAUNCH_SETTLE` and
      are passed in via `TerminalConfig`, so the general terminal opts out by passing
      `None`. `SessionState::ActivityBaseline` and `Workspace::step_activity_baselines`
      are gone, and the policy is unit-tested (7 tests) where before it had none.

### Scheduled Messages  ✅ shipped & verified working end-to-end (2026-07-13)
**Purpose:** auto-resume a session after a Claude usage-limit reset — queue the message
now, have jc submit it when the limit lifts. The `claude` process stays alive in the PTY
(limit-blocked, not exited), so delivery is the same type-into-terminal path as a normal
send.

Deliver a WAIT draft at a future time. Input syntax: begin the WAIT body with
`@jc(HH:MM)` (24h). On Cmd-Enter the message (text-before-cursor / selection, minus the
`@jc(HH:MM)` token) is moved out of WAIT into a `### Message N` heading — with the token
**resolved to an absolute datetime** stored on the heading, e.g.
`### Message 3 @jc(2026-07-13 07:30)`. The message body lives under that heading and is
freely editable afterward. TODO.md is the persistence layer (consistent with "app is
sole writer"); the pending marker re-arms a timer on restart.

Interaction model (decided):
- **Live body, delivered at fire time.** At the scheduled instant jc delivers whatever
  the `### Message N` block *currently* contains (not a schedule-time snapshot) — so you
  edit the block to change what will be sent.
- **Delivered = marker dropped.** On fire the `@jc(...)` marker is removed, leaving a
  plain `### Message N` identical to any normal sent message. That is the "it happened"
  indication; the distinct highlight color applies only while pending.
- **Cancel = delete the marker or the block.** Removing `@jc(...)` (or the whole heading)
  before fire cancels delivery. Editing the datetime re-arms.
- **All sends blocked while one is pending.** Any Cmd-Enter (immediate or scheduled) while
  the session has an undelivered scheduled Message is rejected with an invalid-action beep
  (no-op) — the same feedback as non-typable input. Editing the block text is not a send,
  so tweaking the queued message stays free. (One pending scheduled message per session.)
- **Absolute-time storage** removes occurrence ambiguity and makes restart re-arming
  exact. On launch, if a pending marker's time is already in the past (jc was closed
  through the reset), deliver immediately once — catch-up, which is the whole point for
  the limit-reset use case.

- [x] [E] jc-core: parse a leading `@jc(HH:MM)` on the WAIT body, and `@jc(<datetime>)`
      on `### Message N` headings; add `schedule` + `body_byte_range` to `TodoMessage`
      (absolute pending time; delivered state = marker absent). Unit-tested.
- [x] [E] jc-core: `resolve_schedule(HH:MM, now) -> DateTime` (next occurrence: today if
      not yet passed, else tomorrow) using chrono; reject malformed times. Unit-tested.
- [x] [H] jc-core: extend `send_from_wait` so a leading `@jc(HH:MM)` produces a pending
      `### Message N @jc(<resolved datetime>)` (strip token from message text; do NOT set
      `> last=`/deliver). Plus `TodoSession::pending_scheduled()` for the block-all-sends
      guard, and `drop_schedule_marker()` to deliver. Unit-tested.
- [x] [T] jc-app: `beep()` invalid-action helper (macOS `NSBeep` via objc2-app-kit).
      In `send_to_terminal`, if the session has a pending scheduled Message, beep and
      return (blocks all sends).
- [x] [H] jc-app: on scheduled send, arm `cx.spawn`+`Timer::after` in workspace; at fire
      time re-read the block's current text (`TodoView::deliver_scheduled`), deliver via
      the session's `claude_terminal` (`deliver_to_terminal` helper), set busy + `> last=`,
      and drop the `@jc(...)` marker. Session looked up by UUID; skipped if gone.
- [x] [T] jc-app: distinct highlight color (`@keyword`) for a pending scheduled Message
      heading in `apply_session_highlights` (todo_view.rs); plain color once delivered.
- [x] [E] jc-app: startup re-arm (`arm_existing_schedules` in `Workspace::new`) re-scans
      pending `@jc` markers and arms timers (fire-immediately if past). Cancellation and
      later-datetime edits reconcile self-correctingly at fire time (`ScheduledFire`).
- [x] [H] jc-app: pending `@jc` markers are the single source of truth for the timer set.
      `reconcile_schedules` arms timers from the live markers (dedup via an
      `armed_schedules` HashSet keyed by (path, uuid, when)); it runs at startup and on a
      2s windowed loop (`start_schedule_reconcile_loop`), plus immediately on send and on
      fire-time reschedule. This closes the *earlier* mid-session datetime-edit gap — an
      edit in either direction re-arms within ~2s; the stale timer harmlessly no-ops at
      fire since `fire_scheduled` re-reads the live marker. Flagged by /simplify (altitude).
- [ ] [T] jc-app: a scheduled send defers `> last=` to delivery time, so if jc restarts
      before the fire, `ProjectState::create` may focus a different session as active
      (last-active picks the stale max). Minor cosmetic focus quirk; revisit if annoying.
- [x] [T] Docs: ARCH.md TODO.md-format (input vs stored marker, block-all-sends, delivered
      = marker dropped, catch-up) + DESIGN.md rationale.
- [x] [H] Post-review fixes (high-effort /code-review): fixed silent message loss when the
      target session is gone (verify terminal before consuming marker), empty-body
      permanent-lock (drop marker even on empty body), same-minute `@jc` deferring ~24h
      (minute-granularity `resolve_schedule`), startup catch-up firing into a not-ready
      terminal (5s grace), Disabled/Expired markers arming (filter to Active), plus
      cleanups (guarded `cx.notify`, single-parse `deliver_scheduled`, `now_unix_secs`
      helper, dropped dead `message_index` param).
- [x] [T] Post-simplify cleanups: moved scheduling policy + text rewrite into pure
      jc-core `fire_scheduled`/`FireOutcome` (view just applies text + saves; killed the
      fragile stale-byte-range reasoning), `has_pending_schedule` reads cached
      `self.document` instead of re-parsing on every send, `send_selection` drops the
      unused index, `copy_reply` reuses `deliver_to_terminal`. Unit-tested.

### Session Restore Reliability  ✅ shipped (2026-07-13)
Restore ALL active sessions per project, not just one. Root cause: `ProjectState::create`
(project_state.rs:67-99) picks a single "best" session via `.find().or_else()` + one
`sessions.insert`. state.toml stores only project paths; the session set comes from TODO.md.
- [x] [H] jc-app: in `ProjectState::create`, loop over every `SessionStatus::Active`
      session (skip [D]/[X]/[DELETED]) and adopt each (allocate id, spawn claude terminal,
      insert). Uses the existing `SessionState::create` primitive; bells auto-subscribed
      by `subscribe_bells` which already iterates all sessions.
- [x] [T] jc-app: select active session by most-recent `> last=` (fallback: first).
- [x] [T] Docs: ARCH.md "Session Lifecycle / Project init" — describe all-active adoption
      + last-active selection (current text is aspirational; code only did one).
- [x] [H] Worktree transcript buckets: a session run in a git worktree
      (`<root>/.claude/worktrees/<b>`) lands in a *sibling* `~/.claude/projects` bucket
      `<encoded_root>--claude-worktrees-<b>`, not the root bucket. jc scanned only the root
      bucket, so worktree sessions were falsely marked `[X]` expired on restart and hidden
      from unattached-session discovery. Fix: `jc_core::claude` (new module) enumerates root +
      worktree buckets (`session_dirs`); rewired expiry checks (project_state.rs, workspace/mod.rs)
      and picker unattached-discovery through it; unified the picker's ad-hoc `/`-only encoding
      onto the correct non-`[A-Za-z0-9-]`→`-` encoder. Prefix-scan only (no `git worktree list`),
      so out-of-tree `git worktree add ../elsewhere` buckets are not covered — see below.
- [ ] [E] Out-of-tree worktree buckets: `git worktree add ../pgm-feat` produces an
      unrelated-looking bucket (`-Users-jay-Dev-pgm-feat`) the prefix-scan can't associate with
      the project. Harm is now narrower than when this was filed — expiry is gone, so the only
      loss is that such a session is not offered for adoption in the Cmd-Shift-P picker. If
      needed, enumerate real worktree paths via `git worktree list --porcelain` (off the main
      thread) and add their encoded buckets to `session_dirs`.
- [ ] [E] Move session-bucket scanning off the main thread. `session_dirs` does a synchronous
      `read_dir` of `~/.claude/projects`, which grows a bucket per project ever opened. Callers:
      `ProjectState::create` (once per project, at startup and on `open_project`),
      `ProjectState::launch_for` (only when a transcript is *missing* from the cached bucket list
      — the common path is a few `stat`s), and `ProjectActionsPickerDelegate::new` (one scan per
      Cmd-Shift-P press). Per the gpui rule all three should run on `cx.background_executor()`;
      the picker is the easiest and the constructor the hardest, since it is synchronous.
