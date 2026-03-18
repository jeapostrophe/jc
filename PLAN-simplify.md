# PLAN: Simplify Session Management (Drop JSONL Dependency)

## Motivation

The current session model relies on scanning Claude Code's JSONL files to discover slugs, detect /clear events, and populate the reply viewer. This is fragile — slug extraction requires scanning file heads/tails, fork detection races against Claude's file writes, and /clear rekeying involves multi-step polling. The complexity makes the app feel unreliable.

**New model:** Sessions are identified by Claude's `session_id` UUID, stored directly in TODO.md. Hooks (which already carry `session_id`) provide all the event correlation we need. JSONL files are never read.

## Design

### TODO.md Format Change

Before:
```markdown
# Claude
## Session encapsulated-swimming-firefly: Refactor auth module
### Message 0
...
### WAIT
```

After:
```markdown
# Claude
## Refactor auth module
> uuid=a1b2c3d4-e5f6-7890-abcd-ef1234567890

### Message 0
...
### WAIT
```

- Heading is just `## Label` (no "Session", no slug)
- UUID stored in a blockquote line immediately after the heading
- Blank UUID (`> uuid=`) means session is pending first hook
- Deleted sessions: `## DELETED Refactor auth module` (same as before but without "Session")

### Internal Session Identity

Sessions are keyed by an **internal stable ID** (incrementing `usize`), not by UUID or slug. The UUID is a mutable field on `SessionState` that updates on /clear without any map rekeying.

```rust
type SessionId = usize;

struct SessionState {
    id: SessionId,
    uuid: Option<String>,   // None while pending first hook
    label: String,
    claude_terminal: TerminalView,
    general_terminal: TerminalView,
    pending_events: HashSet<PendingEvent>,
}

struct ProjectState {
    sessions: HashMap<SessionId, SessionState>,
    active_session: Option<SessionId>,
    next_session_id: SessionId,
    // no more pending_rekeys
}
```

### Hook Correlation (No JSONL)

Current: hook arrives with `session_id` → open JSONL file → extract slug → match to session.

New: hook arrives with `session_id` + `cwd` → match `cwd` to project → scan project's sessions for matching `uuid` field → found.

Special case: if no session matches the UUID but one session has `uuid: None` (pending), assign it. This handles new session creation.

### /clear Handling (Simplified)

Current: SessionEnd(clear) → stash old_session_id → SessionStart(clear) → poll JSONL for new slug → rekey session map → update TODO heading. Multiple failure modes.

New:
1. `SessionEnd(reason="clear", session_id="OLD")` → find session by UUID="OLD", note it
2. `SessionStart(source="clear", session_id="NEW")` → same project within 10s window → update session's UUID to "NEW"
3. Update TODO.md: change `> uuid=OLD` to `> uuid=NEW`
4. Done. The same Claude process continues in the same terminal — `/clear` doesn't exit the process, so no terminal relaunch needed.

### Session Creation

1. User selects "NEW" from picker
2. App writes `## New Session\n> uuid=\n\n### WAIT\n` into TODO.md
3. App creates SessionState with `uuid: None`, launches `claude` (no --resume) in terminal
4. First hook event arrives with `session_id` for this project → matches the `uuid: None` session → assigns UUID
5. App updates TODO.md: `> uuid=<session_id>`
6. Constraint: only one `uuid: None` session per project at a time

### Session Resume (App Restart)

1. Parse TODO.md, find `## Refactor auth module\n> uuid=abc123`
2. Create SessionState with `uuid: Some("abc123")`
3. Launch `claude --resume abc123` in terminal
4. Hooks arrive with `session_id=abc123`, matched directly

### Reply Viewer Replacement

Delete `reply_view.rs` entirely. The reply view pane type is removed from the view picker and pane management. Replace with `/copy` automation that targets the code viewer:

1. Keybinding (e.g., Cmd-Shift-C) sends `/copy` to the active session's Claude terminal
2. Poll clipboard for ~2s for change
3. Write clipboard contents to `.jc/replies/<uuid>.md`
4. Open the file in the code viewer pane (same as Cmd-O opening any file)

There is no "reply view" — it's just a markdown file in the code viewer. Re-reading a reply is Cmd-O → pick from `.jc/replies/`. Each session gets one reply file, overwritten on each `/copy`. Old UUIDs' files remain on disk as informal history after /clear. The code viewer already provides syntax highlighting, Cmd-T heading navigation, and comment annotation.

### Pickers (Simplified)

**Session picker (Cmd-P):** Same as before but entries show label instead of slug. Format: `project / label`. No JSONL discovery needed — sessions come from TODO.md only.

**Slug picker (Cmd-Shift-P) → removed.** Its purpose was managing JSONL-discovered sessions vs TODO-adopted sessions. With no JSONL discovery, there are no orphaned sessions to attach. Replace with a simpler "New Session" action (could be an entry in the session picker or a standalone keybinding).

### What's Deleted

| Component | Lines (approx) | Reason |
|---|---|---|
| `session.rs`: slug scanning, JSONL parsing, session groups, turn parsing | ~400 | No JSONL access |
| `reply_view.rs` | ~250 | Replaced by /copy + code viewer |
| `hooks.rs`: `session_id_to_slug()` call | ~5 | UUID matched directly |
| `workspace/mod.rs`: `drain_pending_rekeys()`, `rekey_session()`, polling loops | ~200 | /clear is synchronous |
| `workspace/pickers.rs`: `SlugPickerDelegate` | ~120 | No slug picker |
| `todo.rs`: slug validation, `has_valid_sessions()` JSONL cross-ref | ~30 | Validation is just "is uuid non-empty" |
| Total deleted | **~1,000** | |

### What's Modified

| Component | Change |
|---|---|
| `todo.rs` parser | New heading format: `## Label` + `> uuid=...` line. Drop slug extraction, add uuid extraction. |
| `todo.rs` manipulation | `insert_session_heading()` writes new format. `mark_session_deleted()` uses `## DELETED Label`. |
| `hooks.rs` accept_loop | Drop slug resolution. Pass `session_id` through directly. HookEvent drops `slug` field. |
| `hooks.rs` /clear handling | Same stash logic, but emit both old and new `session_id` without slug resolution. |
| `session_state.rs` | `slug` field → `uuid: Option<String>`. Terminal command uses `--resume <uuid>` directly. |
| `project_state.rs` | Map key changes from slug to SessionId. Drop `pending_rekeys`. |
| `workspace/mod.rs` | `handle_session_clear()` becomes ~10 lines: find session by old UUID, update to new UUID, update TODO, relaunch terminal. |
| `workspace/pickers.rs` | Session picker builds from TODO.md sessions directly. Add /copy keybinding handler. |
| `README.md` | Update session docs, TODO.md format, reply viewer section, keybindings. |

## Task Checklist

### Phase 1: Core Model Changes
- [x] Rewrite `todo.rs` parser for new heading format (`## Label` + `> uuid=...`)
- [x] Rewrite `todo.rs` manipulation functions (insert, delete, mark deleted, send)
- [x] Update `todo.rs` tests for new format
- [x] Change `SessionState`: replace `slug: String` with `id: SessionId` + `uuid: Option<String>`
- [x] Change `ProjectState`: key sessions by `SessionId`, add `next_session_id` counter
- [x] Update `session_state.rs` terminal launch to use `--resume <uuid>` directly

### Phase 2: Hook Simplification
- [x] Remove `slug` field from `HookEvent`
- [x] Remove `session_id_to_slug()` call from hook accept loop
- [x] Keep /clear stash logic but drop slug resolution from the emit path
- [x] Update `workspace/mod.rs` hook handler: match by UUID instead of slug

### Phase 3: /clear + Session Lifecycle
- [x] Rewrite `handle_session_clear()`: find session by old UUID → update uuid → update TODO (no terminal relaunch needed)
- [x] Delete `drain_pending_rekeys()`, `rekey_session()`, `PendingRekey` struct
- [x] Rewrite `create_new_session()`: write blank-UUID heading, launch terminal, wait for first hook
- [x] Update session resume logic on app startup

### Phase 4: Delete JSONL Code
- [x] Delete `reply_view.rs`
- [x] Delete JSONL parsing from `session.rs` (Turn, UserMessage, AssistantResponse, parse_session_group, SessionAccumulator, etc.)
- [x] Delete `SlugPickerDelegate` from pickers
- [x] Delete `ReplyTurnPickerDelegate` and `ReplyHeadingPickerDelegate` from pickers
- [x] Remove reply view from view picker and pane management
- [x] Remove `ReplyViewer` variant from `PaneContentKind` enum
- [x] Remove `ShowSlugPicker` action and keybinding
- [x] Remove `ShowReplyViewer` action
- [x] Clean up imports and dead code
- Note: Kept JSONL slug discovery functions (`discover_session_groups`, `session_id_to_slug`, etc.) as they're still used by `SessionPickerDelegate` and `ProjectState` init

### Phase 5: /copy Automation + Polish
- [x] Add keybinding that sends `/copy` to Claude terminal
- [x] Implement clipboard polling → write to `.jc/replies/<uuid>.md`
- [x] Open result in code viewer pane
- [x] Update session picker to work with new model (label-based entries)
- [x] Update README.md to reflect all changes

## Risks & Open Questions

1. **First hook timing:** If Claude errors before sending any hook, the UUID stays blank. Mitigation: user can manually paste UUID into TODO.md, or retry by creating another session.

2. **Multiple blank UUIDs:** Two "NEW" sessions at once → ambiguous hook assignment. Mitigation: enforce one pending session per project in the UI.

3. **`--resume` with UUID:** The current code already does `claude --resume <uuid>` (extracted from JSONL filenames). This is a supported Claude Code flag. No change needed.

4. **Clipboard access for /copy:** Need `arboard` or similar crate for cross-platform clipboard. macOS-only so `pbpaste` via shell is also fine.

5. **Migration:** Not needed. Old `## Session slug: label` headings won't parse under the new format — they'll just be inert markdown. User can re-add sessions as needed.
