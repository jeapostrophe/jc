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
