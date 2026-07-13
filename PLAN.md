# Plan

> **Labels:** **[T]** Trivial, **[E]** Easy, **[H]** Hard (own Claude session), **[D]** Design (needs human input)

### Git Diff View
- [ ] [H] Word-level inline highlighting via `similar`

### Window & Pane Management
- [ ] [H] Multi-window with shared session state

### Remote Workflow (CLI & Hooks)
- [ ] [H] `jc status` — JSON projects/sessions/problems
- [ ] [H] `jc problems` — JSON problem list with ranks
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
      and drop the `@jc(...)` marker. Session looked up by label; skipped if gone.
- [x] [T] jc-app: distinct highlight color (`@keyword`) for a pending scheduled Message
      heading in `apply_session_highlights` (todo_view.rs); plain color once delivered.
- [x] [E] jc-app: startup re-arm (`arm_existing_schedules` in `Workspace::new`) re-scans
      pending `@jc` markers and arms timers (fire-immediately if past). Cancellation and
      later-datetime edits reconcile self-correctingly at fire time (`ScheduledFire`).
- [x] [H] jc-app: pending `@jc` markers are the single source of truth for the timer set.
      `reconcile_schedules` arms timers from the live markers (dedup via an
      `armed_schedules` HashSet keyed by (path, label, when)); it runs at startup and on a
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
